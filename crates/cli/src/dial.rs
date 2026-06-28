//! Dial → Game/Chat balance application.
//!
//! The Arctis Nova Pro Wireless physical Game↔Chat dial reports two independent
//! 0–100 values in the `[0x07,0x45]` status frame:
//!   - `media_mix` (offset 2, parser Int): 0..=100, 100 = full Game
//!   - `chat_mix`  (offset 3, parser Int): 0..=100, 100 = full Chat
//!
//! Both are applied directly: game sink = media_mix%, chat sink = chat_mix%.
//! This matches the reference app (pactl.py set_mix) and the hardware protocol.
//!
//! ## Thrash avoidance
//!
//! `apply_dial_balance` only applies when the (media_mix, chat_mix) pair differs
//! from the last-applied pair. The caller tracks the last pair.
//!
//! ## Persist-or-not choice
//!
//! The dial path calls `engine.apply_dial_mix` which does NOT persist to disk
//! (transient dial state; no per-tick disk I/O). The last dial position is NOT
//! remembered across daemon restarts — this is intentional: the dial position
//! is re-read from hardware on every poll anyway. This also means a manual
//! `asm-cli channel volume` command is NOT overridden by the next dial tick.

use arctis_audio::CommandRunner;
use arctis_engine::{Engine, EngineError};

/// Apply dial balance to the engine's "game" and "chat" channels.
///
/// Only applied when:
/// - `dial_controls_balance` is true in the config
/// - `(media_mix, chat_mix)` pair differs from `last_mix` (avoids thrash)
///
/// On change, calls `engine.apply_dial_mix(media_mix as u8, chat_mix as u8)`,
/// which applies both values directly (game sink = media_mix%, chat sink =
/// chat_mix%), updates `chatmix_position`, and stamps `last_volume_write` —
/// WITHOUT persisting to disk. Absent channels are skipped gracefully by the
/// engine.
///
/// Returns `Ok(true)` when balance was applied, `Ok(false)` when skipped (no
/// change or flag off).
pub fn apply_dial_balance<R: CommandRunner>(
    engine: &mut Engine<R>,
    media_mix: i64,
    chat_mix: i64,
    last_mix: &mut Option<(i64, i64)>,
    dial_controls_balance: bool,
) -> Result<bool, EngineError> {
    if !dial_controls_balance {
        return Ok(false);
    }

    // Skip if reading hasn't changed (avoids thrash on every poll tick).
    let pair = (media_mix, chat_mix);
    if *last_mix == Some(pair) {
        return Ok(false);
    }

    engine.apply_dial_mix(media_mix as u8, chat_mix as u8)?;

    *last_mix = Some(pair);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn queue_volume_mute_calls(
        runner: arctis_audio::MockRunner,
        n: usize,
    ) -> arctis_audio::MockRunner {
        // apply_dial_mix calls: ls (find node) + set Props (apply_volume_mute_pct)
        // — no save_config on this path (no ASM_CONFIG_HOME needed).
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
        // 2 channels × (1 ls + 1 Props set) = 4 runner calls; no save_config.
        let runner = queue_volume_mute_calls(arctis_audio::MockRunner::new(), 2);
        let cfg = make_config_with_game_chat();
        let mut engine = Engine::new(runner, cfg);
        let mut last: Option<(i64, i64)> = None;

        let result = apply_dial_balance(&mut engine, 80, 20, &mut last, true);
        assert!(result.is_ok(), "apply must not error: {:?}", result);
        assert!(
            result.unwrap(),
            "apply must return true when volumes are changed"
        );
        assert_eq!(last, Some((80, 20)), "last pair must be updated");
    }

    /// When dial_controls_balance is false, no runner calls are made.
    #[test]
    fn apply_dial_balance_skips_when_flag_off() {
        let cfg = make_config_with_game_chat();
        let mut engine = Engine::new(arctis_audio::MockRunner::new(), cfg);
        let mut last: Option<(i64, i64)> = None;

        let result = apply_dial_balance(&mut engine, 80, 20, &mut last, false);
        assert!(result.is_ok(), "apply must not error when flag off");
        assert!(!result.unwrap(), "apply must return false when flag off");
        assert_eq!(last, None, "last must not be updated when flag off");
    }

    /// When the same pair is sent twice, the second call is a no-op.
    #[test]
    fn apply_dial_balance_no_op_on_same_reading() {
        // First call: 2 volume applies (ls+Props ×2)
        let runner = queue_volume_mute_calls(arctis_audio::MockRunner::new(), 2);
        let cfg = make_config_with_game_chat();
        let mut engine = Engine::new(runner, cfg);
        let mut last: Option<(i64, i64)> = None;

        // First call applies
        let _ = apply_dial_balance(&mut engine, 60, 80, &mut last, true);
        assert_eq!(last, Some((60, 80)));

        // Second call with same pair must be a no-op (returns false, last unchanged)
        let result = apply_dial_balance(&mut engine, 60, 80, &mut last, true);
        assert!(result.is_ok(), "no-op must not error");
        assert!(!result.unwrap(), "no-op must return false");
        assert_eq!(last, Some((60, 80)), "last must remain unchanged on no-op");
    }

    /// Serialize tests that mutate ASM_CONFIG_HOME to avoid parallel env-var races.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// When "game" or "chat" channels are absent, apply returns Ok(true) without panic.
    /// (apply_dial_mix handles absent channels gracefully as no-ops.)
    #[test]
    fn apply_dial_balance_graceful_when_channels_absent() {
        // remove_channel calls save_config → needs a real config dir.
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("asm_dial_absent_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Build config with only "media" channel (no game/chat).
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
        // Remove any standard channels seeded by Engine::new.
        engine.remove_channel("game").expect("remove game must succeed");
        engine.remove_channel("chat").expect("remove chat must succeed");
        engine.remove_channel("aux").expect("remove aux must succeed");

        let mut last: Option<(i64, i64)> = None;

        let result = apply_dial_balance(&mut engine, 80, 20, &mut last, true);
        assert!(
            result.is_ok(),
            "absent channels must not panic or error: {result:?}"
        );
        // apply_dial_mix gracefully no-ops absent channel applies;
        // apply_dial_balance returns Ok(true) since the pair changed from None.
        assert!(
            result.unwrap(),
            "apply_dial_balance returns true when reading changed (channels absent is a no-op, not a skip)"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }
}
