    use super::*;
    use crate::engine::test_support::*;

    // ─────────────────────────────────────────────
    // F2.1: set_channel_volume / set_channel_mute tests
    // ─────────────────────────────────────────────

    #[test]
    fn set_channel_volume_persists_and_applies_live() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("ch_vol");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        // set_channel_volume: find_node_id (ls) + apply_volume_mute (Props set)
        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // find_node_id
            .with_output(0, "", ""); // Props set
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        engine
            .set_channel_volume("game", 50)
            .expect("set_channel_volume should succeed");

        // Persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config must be persisted");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("volume_pct = 50"),
            "config.toml must contain volume_pct = 50, got: {saved_str}"
        );

        // In-memory state updated
        let state = engine.state();
        let ch = state.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(ch.volume_pct, 50, "volume_pct must be 50");

        // Event emitted
        let event = rx
            .try_recv()
            .expect("ChannelVolumeSet event must be emitted");
        assert!(
            matches!(
                event,
                crate::state::Event::ChannelVolumeSet {
                    ref channel_id,
                    volume_pct,
                } if channel_id == "game" && volume_pct == 50
            ),
            "event must be ChannelVolumeSet{{channel_id: game, volume_pct: 50}}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_channel_volume_rejects_out_of_range() {
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let err = engine
            .set_channel_volume("game", 101)
            .expect_err("101 pct should be rejected");
        assert!(
            matches!(err, EngineError::BadRequest(_)),
            "expected BadRequest"
        );
    }

    #[test]
    fn set_channel_mute_persists_and_applies_live() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("ch_mute");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // find_node_id
            .with_output(0, "", ""); // Props set
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        engine
            .set_channel_mute("chat", true)
            .expect("set_channel_mute should succeed");

        // Persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config must be persisted");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("muted = true"),
            "config.toml must contain muted = true, got: {saved_str}"
        );

        // In-memory state updated
        let state = engine.state();
        let ch = state.channels.iter().find(|c| c.id == "chat").unwrap();
        assert!(ch.muted, "muted must be true");

        // Event emitted
        let event = rx.try_recv().expect("ChannelMuteSet event must be emitted");
        assert!(
            matches!(
                event,
                crate::state::Event::ChannelMuteSet {
                    ref channel_id,
                    muted,
                } if channel_id == "chat" && muted
            ),
            "event must be ChannelMuteSet{{channel_id: chat, muted: true}}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_channel_volume_rejects_unknown_channel() {
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let err = engine
            .set_channel_volume("nonexistent", 50)
            .expect_err("unknown channel should fail");
        assert!(matches!(err, EngineError::BadRequest(_)));
    }

    #[test]
    fn set_channel_mute_rejects_unknown_channel() {
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let err = engine
            .set_channel_mute("nonexistent", true)
            .expect_err("unknown channel should fail");
        assert!(matches!(err, EngineError::BadRequest(_)));
    }

    // ── A2 TDD: wpctl argv assertions + pw-dump live read ──────────────────

    #[test]
    fn set_channel_volume_pct_emits_correct_pwcli_props_argv() {
        // I2: set_channel_volume must call pw-cli s <node_id> Props {channelVolumes:[(pct/100)^3,…]}
        // (perceptual/cubic, same scale as wpctl/PipeWire/pavucontrol and inverse of the
        //  parse_node_volume cbrt read): 50% → channelVolumes 0.125 (=0.5^3).
        // ls_all_present() maps Arctis_Game → id "10"
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_vol_pct_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // [0] pw-cli ls Node → id "10" for Arctis_Game
            .with_output(0, "", ""); // [1] pw-cli s Props (success)
        let mut engine = Engine::new(runner, cfg);
        engine
            .set_channel_volume("game", 50)
            .expect("set_channel_volume should succeed");
        let calls = &engine.runner.calls;
        assert_eq!(
            calls[0],
            vec!["pw-cli", "ls", "Node"],
            "first call must be pw-cli ls Node (find_node_id)"
        );
        assert_eq!(
            calls[1],
            vec![
                "pw-cli",
                "s",
                "10",
                "Props",
                "{ channelVolumes = [ 0.125 0.125 ] mute = false }",
            ],
            "second call must be pw-cli s <id> Props {{channelVolumes:[0.125,0.125], mute:false}}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn state_channel_volume_pct_from_pw_dump() {
        // TDD A2: state() must report live volume_pct from pw-dump, falling back to config
        let mut cfg = make_config_no_eq_no_routes();
        // Config has game at 80%, but pw-dump reports 50% — live read should win
        cfg.profiles[0].channels[0].volume_pct = 80;
        let pw_dump = concat!(
            r#"[{"type":"PipeWire:Interface:Node","id":10,"info":{"props":{"node.name":"Arctis_Game"},"#,
            r#""params":{"Props":[{"channelVolumes":[0.125,0.125],"mute":false}]}}}]"#
        );
        let runner = MockRunner::new().with_output(0, pw_dump, ""); // [0] pw-dump
        let mut engine = Engine::new(runner, cfg);
        let state = engine.state();
        let game = state.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(
            game.volume_pct, 50,
            "state() must report live volume_pct 50 from pw-dump, not config value 80"
        );
    }

    #[test]
    fn state_volume_dump_cached_within_ttl() {
        // TDD A4: two back-to-back state() calls within the TTL must share one pw-dump subprocess.
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].channels[0].volume_pct = 80;
        let pw_dump = concat!(
            r#"[{"type":"PipeWire:Interface:Node","id":10,"info":{"props":{"node.name":"Arctis_Game"},"#,
            r#""params":{"Props":[{"channelVolumes":[0.125,0.125],"mute":false}]}}}]"#
        );
        // Queue only ONE pw-dump output; if state() calls pw-dump twice, the second
        // call returns an empty response and volume_pct would diverge.
        let runner = MockRunner::new().with_output(0, pw_dump, ""); // [0] pw-dump (only one queued)
        let mut engine = Engine::new(runner, cfg);

        let state1 = engine.state();
        let state2 = engine.state(); // must hit the cache, not spawn a second pw-dump

        let game1 = state1.channels.iter().find(|c| c.id == "game").unwrap();
        let game2 = state2.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(
            game1.volume_pct, 50,
            "first state() must report live volume_pct 50 from pw-dump"
        );
        assert_eq!(
            game2.volume_pct, game1.volume_pct,
            "second state() must return same volume_pct as first (cache hit)"
        );

        let pw_dump_calls = engine.runner.calls.iter().filter(|c| c[0] == "pw-dump").count();
        assert_eq!(
            pw_dump_calls, 1,
            "exactly one pw-dump must be spawned for two back-to-back state() calls within TTL; got {pw_dump_calls}"
        );
    }

    #[test]
    fn state_volume_dump_re_reads_after_cache_cleared() {
        // TDD A4: after expire_volume_cache(), state() must spawn a fresh pw-dump subprocess.
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].channels[0].volume_pct = 80;
        let pw_dump = concat!(
            r#"[{"type":"PipeWire:Interface:Node","id":10,"info":{"props":{"node.name":"Arctis_Game"},"#,
            r#""params":{"Props":[{"channelVolumes":[0.125,0.125],"mute":false}]}}}]"#
        );
        // Queue two pw-dump outputs: one for the first state(), one for after cache expiry.
        let runner = MockRunner::new()
            .with_output(0, pw_dump, "") // [0] first pw-dump
            .with_output(0, pw_dump, ""); // [1] second pw-dump after cache cleared
        let mut engine = Engine::new(runner, cfg);

        let _state1 = engine.state(); // consumes queued output [0]
        engine.expire_volume_cache(); // force cache miss
        let _state2 = engine.state(); // must spawn a new pw-dump, consuming output [1]

        let pw_dump_calls = engine.runner.calls.iter().filter(|c| c[0] == "pw-dump").count();
        assert_eq!(
            pw_dump_calls, 2,
            "two pw-dump calls expected (one per state() after cache cleared); got {pw_dump_calls}"
        );
    }

    // ── I1 TDD: no snap-back after write; live read after cache refresh ──────

    #[test]
    fn set_channel_volume_state_uses_config_not_stale_cache() {
        // I1 no-snap-back: after a volume write, state() must report the just-written config value,
        // NOT the stale cached pw-dump value that was snapshotted before the write.
        // Structure: seed cache with pre-write pw-dump (100%), write 50%, state() must report 50%.
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("ch_vol_snapback");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].channels[0].volume_pct = 100; // initial config value

        // pw-dump fixture: Arctis_Game at 100% (channelVolumes 1.0 → parse → 100)
        let pw_dump_old = concat!(
            r#"[{"type":"PipeWire:Interface:Node","id":10,"info":{"props":{"node.name":"Arctis_Game"},"#,
            r#""params":{"Props":[{"channelVolumes":[1.0,1.0],"mute":false}]}}}]"#
        );
        let ls = ls_all_present();
        // Queue: [0] pw-dump (seeds cache before write), [1] find_node_id, [2] Props set.
        // Second state() hits cache (TTL not expired) → no new pw-dump queued.
        let runner = MockRunner::new()
            .with_output(0, pw_dump_old, "") // [0] first pw-dump (seeds cache, taken_before)
            .with_output(0, &ls, "")         // [1] find_node_id for set_channel_volume
            .with_output(0, "", "");         // [2] pw-cli Props set
        let mut engine = Engine::new(runner, cfg);

        // Seed the cache — taken_before is stamped BEFORE the write.
        let state_before = engine.state();
        let game_before = state_before.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(
            game_before.volume_pct, 100,
            "before write: pw-dump reports 100 (use_live=true, no prior write)"
        );

        // Write 50 → sets last_volume_write = now (after taken_before).
        engine
            .set_channel_volume("game", 50)
            .expect("set_channel_volume should succeed");

        // state() hits the cache (within TTL) but taken_before < written → use_live = false
        // → falls back to config value 50, NOT the stale cached 100.
        let state_after = engine.state();
        let game_after = state_after.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(
            game_after.volume_pct, 50,
            "state() after write must report 50 from config, not 100 from stale pre-write cache"
        );

        // Verify the cache was NOT re-spawned (no additional pw-dump — A4 preserved).
        let pw_dump_calls = engine.runner.calls.iter().filter(|c| c[0] == "pw-dump").count();
        assert_eq!(
            pw_dump_calls, 1,
            "only one pw-dump must be spawned (cache still hit within TTL); got {pw_dump_calls}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_channel_volume_state_uses_live_after_cache_refresh() {
        // I1 live-after-refresh: after expire_volume_cache() + state(), the fresh pw-dump is
        // taken AFTER the write (taken >= written) → use_live = true → live value is used.
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("ch_vol_live_refresh");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].channels[0].volume_pct = 100;

        let pw_dump_old = concat!(
            r#"[{"type":"PipeWire:Interface:Node","id":10,"info":{"props":{"node.name":"Arctis_Game"},"#,
            r#""params":{"Props":[{"channelVolumes":[1.0,1.0],"mute":false}]}}}]"#
        );
        // Fresh pw-dump after the write: reflects the perceptual 50% (cubic linear 0.125).
        let pw_dump_new = concat!(
            r#"[{"type":"PipeWire:Interface:Node","id":10,"info":{"props":{"node.name":"Arctis_Game"},"#,
            r#""params":{"Props":[{"channelVolumes":[0.125,0.125],"mute":false}]}}}]"#
        );
        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, pw_dump_old, "") // [0] first pw-dump (seeds cache)
            .with_output(0, &ls, "")         // [1] find_node_id
            .with_output(0, "", "")           // [2] Props set
            .with_output(0, pw_dump_new, ""); // [3] fresh pw-dump after cache cleared
        let mut engine = Engine::new(runner, cfg);

        engine.state(); // seed cache (taken_before < written)
        engine
            .set_channel_volume("game", 50)
            .expect("set_channel_volume should succeed");
        engine.expire_volume_cache(); // force fresh dump; taken_after > written
        let state = engine.state();   // runs pw-dump [3], taken_after >= written → use_live = true
        let game = state.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(
            game.volume_pct, 50,
            "after cache refresh, state() must report live 50 from fresh pw-dump"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_mic_volume_pct_wpctl_argv() {
        // TDD A2: set_mic_volume must call wpctl set-volume <mic_node_id> <pct/100>
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_vol");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_enabled();
        // arctis_clean_mic → id "20"
        let ls_with_mic = "id 20\n    node.name = \"arctis_clean_mic\"\n";
        let runner = MockRunner::new()
            .with_output(0, ls_with_mic, "") // [0] pw-cli ls Node (find_node_id for mic)
            .with_output(0, "", ""); // [1] wpctl set-volume (success)
        let mut engine = Engine::new(runner, cfg);
        engine
            .set_mic_volume(75)
            .expect("set_mic_volume should succeed");
        let calls = &engine.runner.calls;
        assert_eq!(
            calls[0],
            vec!["pw-cli", "ls", "Node"],
            "first call must be pw-cli ls Node (mic find_node_id)"
        );
        assert_eq!(
            calls[1],
            vec!["wpctl", "set-volume", "20", "0.7500"],
            "second call must be wpctl set-volume <mic_id> 0.7500"
        );
        // Config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config must be persisted");
        let saved = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved.contains("volume_pct = 75"),
            "config.toml must contain volume_pct = 75, got: {saved}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_mic_volume_out_of_range_bad_request() {
        // TDD A2: pct > 100 must return BadRequest
        let cfg = make_config_mic_enabled();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let err = engine
            .set_mic_volume(101)
            .expect_err("101 pct should be rejected");
        assert!(
            matches!(err, EngineError::BadRequest(_)),
            "expected BadRequest, got: {err:?}"
        );
    }

    #[test]
    fn set_master_volume_persists_and_reports() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("master_vol");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        // headset detect: pw-metadata 0 + pw-dump (empty → fallback), then wpctl.
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, "", "")
            .with_output(0, "[]", "")
            .with_output(0, "", "");
        let mut engine = Engine::new(runner, cfg);
        engine.set_master_volume(50).unwrap();
        assert_eq!(engine.state().master_volume_pct, 50);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Master volume must target the REAL hardware headset sink (by object id):
    /// @DEFAULT_AUDIO_SINK@ is wrong once set_default_sink_channel points the
    /// system default at one of our virtual sinks — master would then stack on
    /// the channel volume (double attenuation) and stop controlling the
    /// hardware tail.
    #[test]
    fn set_master_volume_targets_headset_sink_id_when_present() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("master_vol_hw");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let dump = include_str!("../../../../audio/tests/fixtures/pw_dump_sinks.json");
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, "", "") // pw-metadata 0 (no default key)
            .with_output(0, dump, "") // pw-dump (headset sink id 10)
            .with_output(0, "", ""); // wpctl set-volume
        let mut engine = Engine::new(runner, make_config_no_eq_no_routes());
        engine.set_master_volume(50).unwrap();
        assert_eq!(
            engine.runner.last_call().unwrap(),
            &vec!["wpctl", "set-volume", "10", "0.5000"],
            "must target the headset sink id, not @DEFAULT_AUDIO_SINK@"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Without a detectable headset sink, master volume falls back to
    /// @DEFAULT_AUDIO_SINK@ (better than doing nothing).
    #[test]
    fn set_master_volume_falls_back_to_default_sink_when_no_headset() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("master_vol_fb");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let runner = arctis_audio::MockRunner::new()
            .with_output(0, "", "") // pw-metadata 0
            .with_output(0, "[]", "") // pw-dump: no sinks at all
            .with_output(0, "", ""); // wpctl set-volume
        let mut engine = Engine::new(runner, make_config_no_eq_no_routes());
        engine.set_master_volume(75).unwrap();
        assert_eq!(
            engine.runner.last_call().unwrap(),
            &vec!["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", "0.7500"]
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Master mute has the same targeting rule as master volume.
    #[test]
    fn set_master_mute_targets_headset_sink_id_with_default_fallback() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("master_mute_hw");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let dump = include_str!("../../../../audio/tests/fixtures/pw_dump_sinks.json");
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, "", "")
            .with_output(0, dump, "")
            .with_output(0, "", ""); // wpctl set-mute
        let mut engine = Engine::new(runner, make_config_no_eq_no_routes());
        engine.set_master_mute(true).unwrap();
        assert_eq!(
            engine.runner.last_call().unwrap(),
            &vec!["wpctl", "set-mute", "10", "1"]
        );

        // Fallback path.
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, "", "")
            .with_output(0, "[]", "")
            .with_output(0, "", "");
        let mut engine = Engine::new(runner, make_config_no_eq_no_routes());
        engine.set_master_mute(false).unwrap();
        assert_eq!(
            engine.runner.last_call().unwrap(),
            &vec!["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "0"]
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_chatmix_updates_game_chat_volumes() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("chatmix");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        // set_chatmix → set_channel_volume("game") + set_channel_volume("chat")
        // Each set_channel_volume calls apply_volume_mute → 2 runner calls (find_node_id + Props set).
        // With MockRunner exhausted after 2 calls, the second channel fails non-fatally.
        // Queue 2 outputs for the game channel's apply_volume_mute.
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, "", "") // game: find_node_id (empty → node not found, non-fatal)
            .with_output(0, "", ""); // chat: find_node_id (empty → node not found, non-fatal)
        let mut engine = Engine::new(runner, cfg);
        engine.set_chatmix(9).unwrap(); // full game
        assert_eq!(engine.state().chatmix_position, 9);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// The dB targets from chatmix_to_volumes must reach the cubic write path as
    /// pct = 100·10^(dB/60). The old linear conversion (100·10^(dB/20)) tripled
    /// the attenuation: position 5 played at ≈ −13.3 dB instead of −4.4, and
    /// "full" (position 9/0) at −120 dB (pct 1) instead of −40 dB (pct 22).
    #[test]
    fn chatmix_to_volume_pcts_uses_cubic_conversion() {
        // (game_pct, chat_pct) per position; winner side is always 0 dB = 100 %.
        assert_eq!(chatmix_to_volume_pcts(9), (100, 22)); // chat at −40 dB
        assert_eq!(chatmix_to_volume_pcts(7), (100, 43)); // chat at −22.2 dB
        assert_eq!(chatmix_to_volume_pcts(5), (100, 84)); // chat at −4.44 dB
        assert_eq!(chatmix_to_volume_pcts(4), (84, 100)); // game at −4.44 dB
        assert_eq!(chatmix_to_volume_pcts(0), (22, 100)); // game at −40 dB
    }

    /// End-to-end argv check: set_chatmix(9) must write pct 100 (game) and pct 22
    /// (chat) through the cubic Props path — i.e. chat channelVolumes = 0.22^3,
    /// NOT the old linear pct 1 (channelVolumes = 0.01^3 ≈ −120 dB).
    #[test]
    fn set_chatmix_applies_cubic_pcts_via_props_argv() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("chatmix_cubic");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let ls = ls_all_present();
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, &ls, "") // game: find_node_id
            .with_output(0, "", "") // game: pw-cli s Props
            .with_output(0, &ls, "") // chat: find_node_id
            .with_output(0, "", ""); // chat: pw-cli s Props
        let mut engine = Engine::new(runner, cfg);
        engine.set_chatmix(9).unwrap(); // full game

        let game_vol = 1.0f32; // pct 100
        let chat_frac = 22.0f32 / 100.0;
        let chat_vol = chat_frac * chat_frac * chat_frac; // cubic write
        let want_game =
            arctis_audio::set_node_volume_props_argv("10", &[game_vol, game_vol], false).unwrap();
        let want_chat =
            arctis_audio::set_node_volume_props_argv("11", &[chat_vol, chat_vol], false).unwrap();
        let calls = &engine.runner.calls;
        let props_calls: Vec<&Vec<String>> =
            calls.iter().filter(|c| c.get(1).map(String::as_str) == Some("s")).collect();
        assert_eq!(props_calls.len(), 2, "one Props set per side: {calls:?}");
        assert_eq!(props_calls[0][1..], want_game[..], "game side argv");
        assert_eq!(props_calls[1][1..], want_chat[..], "chat side argv");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Slider/dial parity: the GUI slider (set_chatmix) and the hardware dial
    /// (apply_dial_mix) share the SAME cubic write path — equal percent in,
    /// byte-identical Props argv out. (The losing-side CURVES differ by design:
    /// the slider ramps 0→−40 dB; the dial applies firmware percentages verbatim.)
    #[test]
    fn slider_and_dial_share_cubic_write_path() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("chatmix_parity");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let ls = ls_all_present();
        // Slider at position 9 → (game 100 %, chat 22 %).
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, &ls, "")
            .with_output(0, "", "")
            .with_output(0, &ls, "")
            .with_output(0, "", "");
        let mut slider = Engine::new(runner, make_config_no_eq_no_routes());
        slider.set_chatmix(9).unwrap();

        // Dial reporting the same percentages (game 100 %, chat 22 %).
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, &ls, "")
            .with_output(0, "", "")
            .with_output(0, &ls, "")
            .with_output(0, "", "");
        let mut dial = Engine::new(runner, make_config_no_eq_no_routes());
        dial.apply_dial_mix(100, 22).unwrap();

        let props_of = |calls: &[Vec<String>]| -> Vec<Vec<String>> {
            calls
                .iter()
                .filter(|c| c.get(1).map(String::as_str) == Some("s"))
                .cloned()
                .collect()
        };
        assert_eq!(
            props_of(&slider.runner.calls),
            props_of(&dial.runner.calls),
            "equal pct through slider and dial must produce identical Props argv"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn state_dial_controls_balance_reflects_config() {
        // dial_controls_balance defaults to true in make_config_no_eq_no_routes().
        let cfg = make_config_no_eq_no_routes();
        assert!(cfg.dial_controls_balance, "fixture must start with true");
        let mut engine = Engine::new(MockRunner::new(), cfg);
        assert!(
            engine.state().dial_controls_balance,
            "state() must reflect config=true"
        );

        // Flip the flag to false and confirm state() follows.
        let mut cfg2 = make_config_no_eq_no_routes();
        cfg2.dial_controls_balance = false;
        let mut engine2 = Engine::new(MockRunner::new(), cfg2);
        assert!(
            !engine2.state().dial_controls_balance,
            "state() must reflect config=false"
        );
    }

    // ── DIAL-REWORK: mix_to_chatmix_position pure fn + apply_dial_mix ─────────

    #[test]
    fn mix_to_chatmix_position_full_game_is_9() {
        assert_eq!(
            mix_to_chatmix_position(100, 0),
            9,
            "full game (media=100, chat=0) must map to position 9"
        );
    }

    #[test]
    fn mix_to_chatmix_position_full_chat_is_0() {
        assert_eq!(
            mix_to_chatmix_position(0, 100),
            0,
            "full chat (media=0, chat=100) must map to position 0"
        );
    }

    #[test]
    fn mix_to_chatmix_position_equal_levels_is_center() {
        let pos = mix_to_chatmix_position(100, 100);
        assert!(
            pos == 4 || pos == 5,
            "equal levels (100,100) must map to center position 4 or 5, got {pos}"
        );
    }

    #[test]
    fn mix_to_chatmix_position_game_dominant_above_center() {
        let pos = mix_to_chatmix_position(100, 66);
        assert!(
            pos > 4,
            "game-dominant (media=100, chat=66) must give position >4 (game side), got {pos}"
        );
    }

    #[test]
    fn mix_to_chatmix_position_chat_dominant_below_center() {
        let pos = mix_to_chatmix_position(66, 100);
        assert!(
            pos < 5,
            "chat-dominant (media=66, chat=100) must give position <5 (chat side), got {pos}"
        );
    }

    #[test]
    fn mix_to_chatmix_position_clamps_above_100() {
        // Values above 100 must be treated as 100.
        assert_eq!(
            mix_to_chatmix_position(200, 0),
            mix_to_chatmix_position(100, 0),
            "media_mix > 100 must clamp to 100"
        );
        assert_eq!(
            mix_to_chatmix_position(0, 200),
            mix_to_chatmix_position(0, 100),
            "chat_mix > 100 must clamp to 100"
        );
    }

    #[test]
    fn mix_to_chatmix_position_result_always_in_0_to_9() {
        // Exhaustive spot-check covering boundary combos.
        for m in [0u8, 50, 100, 200] {
            for c in [0u8, 50, 100, 200] {
                let pos = mix_to_chatmix_position(m, c);
                assert!(
                    (0..=9).contains(&pos),
                    "mix_to_chatmix_position({m},{c}) = {pos} out of 0..=9"
                );
            }
        }
    }

    /// apply_dial_mix(80, 100): game=80%, chat=100% applied via pw-cli Props
    /// (perceptual/cubic: 0.8^3=0.512 and 1.0^3=1.0), chatmix_position updated, NO config written.
    #[test]
    fn apply_dial_mix_applies_both_channels_no_save() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_dial_mix_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let ls = ls_all_present(); // Arctis_Game=10, Arctis_Chat=11

        // Queue: ls+Props for game, ls+Props for chat (4 total; no save_config calls)
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // [0] ls → game id 10
            .with_output(0, "", "") // [1] Props for game
            .with_output(0, &ls, "") // [2] ls → chat id 11
            .with_output(0, "", ""); // [3] Props for chat
        let mut engine = Engine::new(runner, cfg);

        engine
            .apply_dial_mix(80, 100)
            .expect("apply_dial_mix must succeed");

        // Verify the 4 pw-cli calls (no extra save_config calls through runner)
        let calls = &engine.runner.calls;
        assert_eq!(calls.len(), 4, "exactly 4 runner calls (ls+Props×2), got {}", calls.len());
        assert_eq!(
            calls[0],
            vec!["pw-cli", "ls", "Node"],
            "call[0] must be pw-cli ls Node (find game id)"
        );
        assert_eq!(
            calls[1],
            vec![
                "pw-cli",
                "s",
                "10",
                "Props",
                "{ channelVolumes = [ 0.512 0.512 ] mute = false }",
            ],
            "call[1] must set game to 80% perceptual (0.8^3=0.512 linear)"
        );
        assert_eq!(
            calls[2],
            vec!["pw-cli", "ls", "Node"],
            "call[2] must be pw-cli ls Node (find chat id)"
        );
        assert_eq!(
            calls[3],
            vec![
                "pw-cli",
                "s",
                "11",
                "Props",
                "{ channelVolumes = [ 1.0 1.0 ] mute = false }",
            ],
            "call[3] must set chat to 100% (1.0 linear)"
        );

        // Verify no config file was written (no save_config on the hot path)
        let cfg_path = tmp.join("config.toml");
        assert!(
            !cfg_path.exists(),
            "config.toml must NOT be written by apply_dial_mix (no save on hot path)"
        );

        // Verify state() reflects the updated chatmix_position
        let expected_pos = mix_to_chatmix_position(80, 100);
        let state = engine.state(); // pw-dump gets empty default → falls back to config
        assert_eq!(
            state.chatmix_position, expected_pos,
            "state().chatmix_position must reflect mix_to_chatmix_position(80,100)={expected_pos}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// apply_hardware_master_volume(73) mirrors the knob value into master_volume_pct
    /// WITHOUT calling wpctl (no software gain) and WITHOUT persisting to disk.
    #[test]
    fn apply_hardware_master_volume_mirrors_value_no_wpctl_no_save() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_knob_master_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        engine
            .apply_hardware_master_volume(73)
            .expect("apply_hardware_master_volume must succeed");

        // No wpctl call — the knob is the hardware gain; we mirror the VALUE only.
        assert!(
            !engine
                .runner
                .calls
                .iter()
                .any(|c| c.first().map(|s| s.as_str()) == Some("wpctl")),
            "apply_hardware_master_volume must NOT call wpctl, calls: {:?}",
            engine.runner.calls
        );

        // No config persisted to disk (transient hardware mirror; no save_config).
        let cfg_path = tmp.join("config.toml");
        assert!(
            !cfg_path.exists(),
            "config.toml must NOT be written by apply_hardware_master_volume"
        );

        // state() reflects the mirrored master volume (read straight from config).
        let state = engine.state();
        assert_eq!(
            state.master_volume_pct, 73,
            "state().master_volume_pct must mirror the knob value"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// apply_dial_mix with absent game and chat channels is a graceful no-op (no panic or error).
    #[test]
    fn apply_dial_mix_graceful_when_channels_absent() {
        // remove_channel calls save_config → needs a real config dir.
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp =
            std::env::temp_dir().join(format!("asm_dial_mix_absent_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Build a config with only media/aux, then remove game/chat via remove_channel.
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        // MockRunner default empty response → sink_exists() returns false → teardown no-op.
        engine.remove_channel("game").expect("remove game");
        engine.remove_channel("chat").expect("remove chat");

        // Snapshot call count after remove_channel (those make pw-cli ls Node calls).
        let calls_before = engine.runner.calls.len();

        let result = engine.apply_dial_mix(80, 20);
        assert!(
            result.is_ok(),
            "apply_dial_mix with absent channels must not error: {result:?}"
        );
        // apply_dial_mix must make NO additional pw-cli calls when both channels absent.
        assert_eq!(
            engine.runner.calls.len(),
            calls_before,
            "apply_dial_mix must not make new runner calls when game and chat are absent"
        );
    }
