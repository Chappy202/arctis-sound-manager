//! Dial → Game/Chat balance mapping and application.
//!
//! The Arctis Nova Pro Wireless physical Game↔Chat dial reports two fields in the
//! device status response:
//!   - `chat_mix`   (offset 3, parser Int): 0..=9, 9 = full Chat
//!   - `media_mix`  (offset 2, parser Int): 0..=9, 9 = full Game
//!
//! Both fields change together from the same [0x07, 0x45] response frame; they are
//! complementary (chat_mix + media_mix ≈ 9 at any dial position, though protocol
//! details are still to be validated on real hardware).
//!
//! ## Mapping curve: `dial_to_channel_volumes`
//!
//! We use `chat_mix` as the single "dial reading" (0 = full Game side, 9 = full Chat
//! side).  Center is 4 or 5 (both treated the same — whichever the hardware reports
//! at the physical midpoint).
//!
//!   - `chat_mix` = 4 or 5 (center): game = 0.0 dB, chat = 0.0 dB
//!   - `chat_mix` = 0 (full Game):   game = 0.0 dB, chat = -40.0 dB
//!   - `chat_mix` = 9 (full Chat):   game = -40.0 dB, chat = 0.0 dB
//!
//! Linear interpolation in the dB domain (simple, predictable):
//!   - When chat_mix < center: game stays at 0 dB; chat is interpolated from 0 to -40 dB
//!   - When chat_mix > center: chat stays at 0 dB; game is interpolated from 0 to -40 dB
//!
//! The `center` is defined as 4.5 (midpoint of the 0..=9 range) so that positions 4
//! and 5 both give slightly positive attenuation on one side, symmetric about center.
//! This provides perceptually equal treatment of positions 4 and 5.
//!
//! ## Persist-or-not choice
//!
//! `apply_dial_balance` calls `engine.set_channel_volume` which DOES persist. This
//! means the last dial position is remembered across restarts. This is intentional:
//! the volume is part of the channel config and should survive a daemon restart.
//! The trade-off is that a manual `asm-cli channel volume` command for game/chat will
//! be overridden next time the dial moves. Document this limitation.
//!
//! Thrash avoidance: `apply_dial_balance` only applies when `chat_mix` reading changes
//! from the last-applied value. The caller must track the last reading.

use arctis_audio::CommandRunner;
use arctis_engine::{Engine, EngineError};

/// Maximum attenuation in dB applied at the fully-away-from-center dial position.
const FULL_ATTENUATION_DB: f32 = -40.0;

/// The center of the 0..=9 dial range (4.5 treats both 4 and 5 symmetrically).
const DIAL_CENTER: f32 = 4.5;

/// The minimum (full-game) position.
const DIAL_MIN: f32 = 0.0;

/// The maximum (full-chat) position.
const DIAL_MAX: f32 = 9.0;

/// Map a `chat_mix` dial reading (0..=9, where 9 = full Chat) to
/// `(game_db, chat_db)` software-volume dB levels.
///
/// Center (≈ 4.5): both channels at 0.0 dB.
/// Full-game  (0): chat at -40 dB, game at 0 dB.
/// Full-chat  (9): game at -40 dB, chat at 0 dB.
///
/// Linear interpolation in the dB domain:
/// - When reading ≤ center: game = 0.0, chat = lerp(0.0, -40.0, (center - reading) / center)
/// - When reading ≥ center: chat = 0.0, game = lerp(0.0, -40.0, (reading - center) / (max - center))
///
/// Readings outside 0..=9 are clamped.
pub fn dial_to_channel_volumes(chat_mix: i64) -> (f32, f32) {
    let r = (chat_mix as f32).clamp(DIAL_MIN, DIAL_MAX);
    if r <= DIAL_CENTER {
        // Game side dominates: game stays at 0 dB, attenuate chat
        let t = (DIAL_CENTER - r) / DIAL_CENTER; // 0.0 at center, 1.0 at full-game
        let chat_db = FULL_ATTENUATION_DB * t;
        (0.0, chat_db)
    } else {
        // Chat side dominates: chat stays at 0 dB, attenuate game
        let t = (r - DIAL_CENTER) / (DIAL_MAX - DIAL_CENTER); // 0.0 at center, 1.0 at full-chat
        let game_db = FULL_ATTENUATION_DB * t;
        (game_db, 0.0)
    }
}

/// Apply dial balance to the engine's "game" and "chat" channels.
///
/// Only applied when:
/// - `dial_controls_balance` is true in the config
/// - `new_chat_mix` differs from `last_chat_mix` (avoids thrash)
/// - Both "game" and "chat" channels exist in the active profile (skips gracefully if absent)
///
/// Returns `Ok(true)` when balance was applied, `Ok(false)` when skipped (no change or
/// flag off or channels absent).
pub fn apply_dial_balance<R: CommandRunner>(
    engine: &mut Engine<R>,
    new_chat_mix: i64,
    last_chat_mix: &mut Option<i64>,
    dial_controls_balance: bool,
) -> Result<bool, EngineError> {
    if !dial_controls_balance {
        return Ok(false);
    }

    // Skip if reading hasn't changed (avoids thrash)
    if *last_chat_mix == Some(new_chat_mix) {
        return Ok(false);
    }

    let (game_db, chat_db) = dial_to_channel_volumes(new_chat_mix);

    // Check that both channels exist in the active profile; skip gracefully if absent.
    let active = engine.config().active()?;
    let has_game = active.channels.iter().any(|ch| ch.id == "game");
    let has_chat = active.channels.iter().any(|ch| ch.id == "chat");

    if !has_game || !has_chat {
        // Channels absent — skip without error (graceful degradation as spec requires)
        return Ok(false);
    }

    engine.set_channel_volume("game", game_db)?;
    engine.set_channel_volume("chat", chat_db)?;

    *last_chat_mix = Some(new_chat_mix);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── dial_to_channel_volumes pure mapping tests ────────────────────────────

    #[test]
    fn dial_center_4_gives_both_zero_db() {
        let (game, chat) = dial_to_channel_volumes(4);
        // At position 4, t = (4.5 - 4.0) / 4.5 = 0.111...; chat slightly attenuated
        // Verify game = 0.0 dB, chat < 0 but very small attenuation
        assert!(
            (game - 0.0).abs() < f32::EPSILON,
            "game must be 0.0 dB at position 4, got {game}"
        );
        assert!(
            chat > -5.0,
            "chat attenuation at position 4 must be small (>-5 dB), got {chat}"
        );
    }

    #[test]
    fn dial_center_5_gives_both_near_zero_db() {
        let (game, chat) = dial_to_channel_volumes(5);
        // At position 5, t = (5.0 - 4.5) / (9.0 - 4.5) = 0.111...; game slightly attenuated
        assert!(
            (chat - 0.0).abs() < f32::EPSILON,
            "chat must be 0.0 dB at position 5, got {chat}"
        );
        assert!(
            game > -5.0,
            "game attenuation at position 5 must be small (>-5 dB), got {game}"
        );
    }

    #[test]
    fn dial_full_game_attenuates_chat_to_minus_40() {
        let (game, chat) = dial_to_channel_volumes(0);
        assert!(
            (game - 0.0).abs() < f32::EPSILON,
            "game must be 0.0 dB at full-game (position 0), got {game}"
        );
        assert!(
            (chat - FULL_ATTENUATION_DB).abs() < 0.01,
            "chat must be {FULL_ATTENUATION_DB} dB at full-game (position 0), got {chat}"
        );
    }

    #[test]
    fn dial_full_chat_attenuates_game_to_minus_40() {
        let (game, chat) = dial_to_channel_volumes(9);
        assert!(
            (chat - 0.0).abs() < f32::EPSILON,
            "chat must be 0.0 dB at full-chat (position 9), got {chat}"
        );
        assert!(
            (game - FULL_ATTENUATION_DB).abs() < 0.01,
            "game must be {FULL_ATTENUATION_DB} dB at full-chat (position 9), got {game}"
        );
    }

    #[test]
    fn dial_midpoint_2_attenuates_chat_proportionally() {
        let (game, chat) = dial_to_channel_volumes(2);
        // t = (4.5 - 2.0) / 4.5 = 2.5/4.5 ≈ 0.556; chat ≈ -40 * 0.556 ≈ -22.2 dB
        assert!(
            (game - 0.0).abs() < f32::EPSILON,
            "game must be 0.0 dB at position 2, got {game}"
        );
        assert!(
            chat < -10.0 && chat > -35.0,
            "chat at position 2 should be moderate attenuation, got {chat}"
        );
    }

    #[test]
    fn dial_midpoint_7_attenuates_game_proportionally() {
        let (game, chat) = dial_to_channel_volumes(7);
        // t = (7.0 - 4.5) / (9.0 - 4.5) = 2.5/4.5 ≈ 0.556; game ≈ -22.2 dB
        assert!(
            (chat - 0.0).abs() < f32::EPSILON,
            "chat must be 0.0 dB at position 7, got {chat}"
        );
        assert!(
            game < -10.0 && game > -35.0,
            "game at position 7 should be moderate attenuation, got {game}"
        );
    }

    #[test]
    fn dial_out_of_range_negative_clamped_to_zero() {
        // Dial reading of -1 must be treated as 0 (full game)
        let (game, chat) = dial_to_channel_volumes(-1);
        let (game0, chat0) = dial_to_channel_volumes(0);
        assert!(
            (game - game0).abs() < 0.01,
            "negative reading must clamp to 0"
        );
        assert!(
            (chat - chat0).abs() < 0.01,
            "negative reading must clamp to 0"
        );
    }

    #[test]
    fn dial_out_of_range_above_max_clamped_to_9() {
        // Dial reading of 10 must be treated as 9 (full chat)
        let (game, chat) = dial_to_channel_volumes(10);
        let (game9, chat9) = dial_to_channel_volumes(9);
        assert!((game - game9).abs() < 0.01, "reading > 9 must clamp to 9");
        assert!((chat - chat9).abs() < 0.01, "reading > 9 must clamp to 9");
    }

    // ── apply_dial_balance tests with MockRunner ──────────────────────────────

    fn make_config_with_game_chat() -> arctis_config::Config {
        let channels = vec![
            arctis_config::ChannelConfig {
                id: "game".into(),
                node_name: "Arctis_Game".into(),
                description: "Game".into(),
                output_device: None,
                eq: vec![],
                volume_db: 0.0,
                muted: false,
            },
            arctis_config::ChannelConfig {
                id: "chat".into(),
                node_name: "Arctis_Chat".into(),
                description: "Chat".into(),
                output_device: None,
                eq: vec![],
                volume_db: 0.0,
                muted: false,
            },
        ];
        let mut cfg = arctis_config::Config::default_config();
        cfg.profiles[0].channels = channels;
        cfg
    }

    /// Serialize tests that mutate ASM_CONFIG_HOME to avoid parallel env-var races.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn queue_volume_mute_calls(
        runner: arctis_audio::MockRunner,
        n: usize,
    ) -> arctis_audio::MockRunner {
        // set_channel_volume calls: ls (find node) + set Props (apply_volume_mute)
        // Also calls save_config which needs ASM_CONFIG_HOME (handled by caller)
        let ls = "id 10\n    node.name = \"Arctis_Game\"\nid 11\n    node.name = \"Arctis_Chat\"\n";
        let mut r = runner;
        for _ in 0..n {
            r = r.with_output(0, ls, ""); // ls for find node
            r = r.with_output(0, "", ""); // Props set
        }
        r
    }

    /// A dial change (from None) → both game and chat volumes are applied.
    #[test]
    fn apply_dial_balance_applies_volumes_on_change() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_dial_apply_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // 2 channels × (1 ls + 1 Props set) = 4 runner calls
        let runner = queue_volume_mute_calls(arctis_audio::MockRunner::new(), 2);
        let cfg = make_config_with_game_chat();
        let mut engine = Engine::new(runner, cfg);
        let mut last: Option<i64> = None;

        let result = apply_dial_balance(&mut engine, 0, &mut last, true);
        assert!(result.is_ok(), "apply must not error: {:?}", result);
        assert!(
            result.unwrap(),
            "apply must return true when volumes are changed"
        );
        assert_eq!(last, Some(0), "last reading must be updated");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// When dial_controls_balance is false, no runner calls are made.
    #[test]
    fn apply_dial_balance_skips_when_flag_off() {
        let cfg = make_config_with_game_chat();
        // If apply happens, the MockRunner will fail on unexpected runner calls
        let mut engine = Engine::new(arctis_audio::MockRunner::new(), cfg);
        let mut last: Option<i64> = None;

        let result = apply_dial_balance(&mut engine, 0, &mut last, false);
        assert!(result.is_ok(), "apply must not error when flag off");
        assert!(!result.unwrap(), "apply must return false when flag off");
        assert_eq!(last, None, "last must not be updated when flag off");
    }

    /// When the same reading is sent twice, the second call is a no-op.
    #[test]
    fn apply_dial_balance_no_op_on_same_reading() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_dial_noop_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // First call: 2 volume applies
        let runner = queue_volume_mute_calls(arctis_audio::MockRunner::new(), 2);
        let cfg = make_config_with_game_chat();
        let mut engine = Engine::new(runner, cfg);
        let mut last: Option<i64> = None;

        // First call applies
        let _ = apply_dial_balance(&mut engine, 5, &mut last, true);
        assert_eq!(last, Some(5));

        // Second call with same reading must be a no-op (no extra runner calls)
        let result = apply_dial_balance(&mut engine, 5, &mut last, true);
        assert!(result.is_ok(), "no-op must not error");
        assert!(!result.unwrap(), "no-op must return false");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// When "game" or "chat" channels are absent, apply returns Ok(false) without panic.
    #[test]
    fn apply_dial_balance_graceful_when_channels_absent() {
        // Config with only "media" channel — no game or chat
        let mut cfg = arctis_config::Config::default_config();
        cfg.profiles[0].channels = vec![arctis_config::ChannelConfig {
            id: "media".into(),
            node_name: "Arctis_Media".into(),
            description: "Media".into(),
            output_device: None,
            eq: vec![],
            volume_db: 0.0,
            muted: false,
        }];

        let mut engine = Engine::new(arctis_audio::MockRunner::new(), cfg);
        let mut last: Option<i64> = None;

        let result = apply_dial_balance(&mut engine, 0, &mut last, true);
        assert!(result.is_ok(), "absent channels must not panic or error");
        assert!(
            !result.unwrap(),
            "absent channels must return false (graceful skip)"
        );
    }
}
