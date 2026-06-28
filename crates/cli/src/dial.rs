//! Dial → Game/Chat balance mapping and application.
//!
//! The Arctis Nova Pro Wireless physical Game↔Chat dial reports two fields in the
//! device status response:
//!   - `chat_mix`   (offset 3, parser Int): 0..=100, 100 = full Chat
//!   - `media_mix`  (offset 2, parser Int): 0..=100, 100 = full Game
//!
//! Both fields change together from the same [0x07, 0x45] response frame; they are
//! complementary (chat_mix + media_mix ≈ 100 at any dial position, though protocol
//! details are still to be validated on real hardware).
//!
//! ## Mapping curve: `dial_to_channel_volumes`
//!
//! We use `chat_mix` as the single "dial reading" (0 = full Game side, 100 = full Chat
//! side).  Center is 50 (the physical midpoint of the 0..=100 range).
//!
//!   - `chat_mix` = 50 (center):   game = 100%, chat = 100%
//!   - `chat_mix` = 0 (full Game): game = 100%, chat = 0%
//!   - `chat_mix` = 100 (full Chat): game = 0%, chat = 100%
//!
//! Linear interpolation in percent:
//!   - When reading ≤ center: game = 100%, chat = reading / center * 100%
//!   - When reading ≥ center: chat = 100%, game = (max - reading) / (max - center) * 100%
//!
//! Readings outside 0..=100 are clamped.
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

/// The center of the 0..=100 dial range.
const DIAL_CENTER: f32 = 50.0;

/// The minimum (full-game) position.
const DIAL_MIN: f32 = 0.0;

/// The maximum (full-chat) position.
const DIAL_MAX: f32 = 100.0;

/// Map a `chat_mix` dial reading (0..=100, where 100 = full Chat) to
/// `(game_pct, chat_pct)` software-volume percent levels (0–100).
///
/// Center (50): both channels at 100%.
/// Full-game  (0):   chat = 0%, game = 100%.
/// Full-chat  (100): game = 0%, chat = 100%.
///
/// Linear interpolation in percent:
/// - When reading ≤ center: game = 100%, chat = reading / center * 100%
/// - When reading ≥ center: chat = 100%, game = (max - reading) / (max - center) * 100%
///
/// Readings outside 0..=100 are clamped.
pub fn dial_to_channel_volumes(chat_mix: i64) -> (u8, u8) {
    let r = (chat_mix as f32).clamp(DIAL_MIN, DIAL_MAX);
    if r <= DIAL_CENTER {
        // Game side dominates: game stays at 100%, lerp chat from 0% to 100%
        let chat_pct = (r / DIAL_CENTER * 100.0).round() as u8;
        (100, chat_pct)
    } else {
        // Chat side dominates: chat stays at 100%, lerp game from 100% to 0%
        let game_pct = ((DIAL_MAX - r) / (DIAL_MAX - DIAL_CENTER) * 100.0).round() as u8;
        (game_pct, 100)
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

    let (game_pct, chat_pct) = dial_to_channel_volumes(new_chat_mix);

    // Check that both channels exist in the active profile; skip gracefully if absent.
    let active = engine.config().active()?;
    let has_game = active.channels.iter().any(|ch| ch.id == "game");
    let has_chat = active.channels.iter().any(|ch| ch.id == "chat");

    if !has_game || !has_chat {
        // Channels absent — skip without error (graceful degradation as spec requires)
        return Ok(false);
    }

    engine.set_channel_volume("game", game_pct)?;
    engine.set_channel_volume("chat", chat_pct)?;

    *last_chat_mix = Some(new_chat_mix);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── dial_to_channel_volumes pure mapping tests ────────────────────────────

    #[test]
    fn dial_center_50_gives_both_100_pct() {
        let (game, chat) = dial_to_channel_volumes(50);
        assert_eq!(game, 100, "game must be 100% at center (position 50), got {game}");
        assert_eq!(chat, 100, "chat must be 100% at center (position 50), got {chat}");
    }

    #[test]
    fn dial_full_game_gives_chat_0() {
        let (game, chat) = dial_to_channel_volumes(0);
        assert_eq!(game, 100, "game must be 100% at full-game (position 0), got {game}");
        assert_eq!(chat, 0, "chat must be 0% at full-game (position 0), got {chat}");
    }

    #[test]
    fn dial_full_chat_gives_game_0() {
        let (game, chat) = dial_to_channel_volumes(100);
        assert_eq!(chat, 100, "chat must be 100% at full-chat (position 100), got {chat}");
        assert_eq!(game, 0, "game must be 0% at full-chat (position 100), got {game}");
    }

    #[test]
    fn dial_midpoint_25_attenuates_chat_proportionally() {
        let (game, chat) = dial_to_channel_volumes(25);
        // position 25: game side dominates; chat = 25/50 * 100 = 50%
        assert_eq!(game, 100, "game must be 100% at position 25, got {game}");
        assert_eq!(chat, 50, "chat must be 50% at position 25, got {chat}");
    }

    #[test]
    fn dial_midpoint_75_attenuates_game_proportionally() {
        let (game, chat) = dial_to_channel_volumes(75);
        // position 75: chat side dominates; game = (100-75)/(100-50) * 100 = 50%
        assert_eq!(chat, 100, "chat must be 100% at position 75, got {chat}");
        assert_eq!(game, 50, "game must be 50% at position 75, got {game}");
    }

    #[test]
    fn dial_out_of_range_negative_clamped_to_zero() {
        // Dial reading of -1 must be treated as 0 (full game)
        let (game, chat) = dial_to_channel_volumes(-1);
        let (game0, chat0) = dial_to_channel_volumes(0);
        assert_eq!(game, game0, "negative reading must clamp to 0");
        assert_eq!(chat, chat0, "negative reading must clamp to 0");
    }

    #[test]
    fn dial_out_of_range_above_max_clamped_to_100() {
        // Dial reading of 101 must be treated as 100 (full chat)
        let (game, chat) = dial_to_channel_volumes(101);
        let (game100, chat100) = dial_to_channel_volumes(100);
        assert_eq!(game, game100, "reading > 100 must clamp to 100");
        assert_eq!(chat, chat100, "reading > 100 must clamp to 100");
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
                volume_pct: 100,
                muted: false,
            },
            arctis_config::ChannelConfig {
                id: "chat".into(),
                node_name: "Arctis_Chat".into(),
                description: "Chat".into(),
                output_device: None,
                eq: vec![],
                volume_db: 0.0,
                volume_pct: 100,
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
        // Early-return is proven by `last` remaining None and zero runner calls.
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
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("asm_dial_absent_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Build config with only "media" channel, then let Engine::new seed the standards.
        // Engine::new calls ensure_standard_channels() which adds game/chat/aux, so we must
        // explicitly remove game and chat after construction to re-establish the intended
        // "channels absent" precondition. This faithfully exercises the graceful-skip path
        // that fires at runtime when a user removes a channel within a session.
        let mut cfg = arctis_config::Config::default_config();
        cfg.profiles[0].channels = vec![arctis_config::ChannelConfig {
            id: "media".into(),
            node_name: "Arctis_Media".into(),
            description: "Media".into(),
            output_device: None,
            eq: vec![],
            volume_db: 0.0,
            volume_pct: 100,
            muted: false,
        }];

        let mut engine = Engine::new(arctis_audio::MockRunner::new(), cfg);

        // Remove the seeded channels. MockRunner default empty response causes
        // AudioBackend::sink_exists() to return false → teardown is a no-op.
        engine.remove_channel("game").expect("remove game must succeed");
        engine.remove_channel("chat").expect("remove chat must succeed");

        let mut last: Option<i64> = None;

        let result = apply_dial_balance(&mut engine, 0, &mut last, true);
        assert!(result.is_ok(), "absent channels must not panic or error");
        assert!(
            !result.unwrap(),
            "absent channels must return false (graceful skip)"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }
}
