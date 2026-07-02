    use super::*;

    /// pw-dump fixture: a STEREO stream linked to Arctis_Game (the surround sink).
    const STEREO_STREAM_DUMP: &str = r#"[
      { "id": 50, "type": "PipeWire:Interface:Node",
        "info": { "props": { "media.class": "Audio/Sink", "node.name": "Arctis_Game" } } },
      { "id": 51, "type": "PipeWire:Interface:Node",
        "info": { "props": { "media.class": "Stream/Output/Audio",
            "application.name": "Spotify", "application.process.binary": "spotify" },
          "params": { "Format": [ { "channels": 2, "position": ["FL","FR"] } ] } } },
      { "id": 99, "type": "PipeWire:Interface:Link",
        "info": { "output-node-id": 51, "input-node-id": 50 } }
    ]"#;

    /// Auto + a stereo-only feeding stream → state() must report the REAL
    /// effective mode (stereo_bypass), not the old hardcoded hrir71.
    #[test]
    fn state_effective_mode_auto_reports_bypass_for_stereo_input() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let runner = MockRunner::new().with_output(0, STEREO_STREAM_DUMP, ""); // state(): pw-dump
        let cfg = make_config_surround_single_game_no_eq("test-hrir"); // mode = Auto
        let mut engine = Engine::new(runner, cfg);
        let st = engine.state();
        assert_eq!(st.surround.mode, "auto");
        assert_eq!(st.surround.negotiated_channels, Some(2));
        assert_eq!(
            st.surround.effective_mode, "stereo_bypass",
            "Auto with a stereo input must report stereo_bypass"
        );
    }

    /// Auto + a stereo-only feeding stream → apply_surround must build the
    /// stereo-bypass graph (2-ch, no convolver), not the 7.1 HRIR graph.
    #[test]
    fn apply_surround_auto_with_stereo_input_builds_bypass() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let runner = queue_apply_surround_both_absent(
            MockRunner::new()
                .with_output(0, STEREO_STREAM_DUMP, "") // Auto probe: pw-dump (stereo stream)
                .with_output(0, "", "") // detect: pw-metadata 0
                .with_output(0, "[]", ""), // detect: pw-dump []
        );
        let cfg = make_config_surround_single_game_no_eq("test-hrir"); // mode = Auto
        let mut engine = Engine::new(runner, cfg);
        let profile = engine.config.active().unwrap().clone();
        engine.apply_surround(&profile).expect("apply_surround must succeed");

        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| argv.get(2).map(|s| s.contains("arctis_surround")).unwrap_or(false))
            .expect("surround conf must have been spawned");
        let conf = std::fs::read_to_string(&surround_argv[2]).expect("conf exists");
        assert!(
            conf.contains("audio.channels = 2"),
            "Auto+stereo must spawn the 2-ch bypass graph, got:\n{conf}"
        );
        assert!(
            !conf.contains("convolver"),
            "Auto+stereo must NOT build the HRIR convolver graph"
        );
    }

    // ─────────────────────────────────────────────
    // F1.3 Bug Fix tests: C1, C2, I3
    // ─────────────────────────────────────────────

    /// C1 fix: apply_surround restores a channel removed from the surround list.
    ///
    /// Setup: `surround_routed = {"game", "media"}`, config has `channels = ["game"]`.
    /// After apply_surround: media must be restored to its output_device; surround_routed = {"game"}.
    #[test]
    fn apply_surround_restores_removed_channel() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp_home = unique_cfg_tmp("c1_restore_home");
        let profiles_dir = tmp_home.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp_home);

        // Config: surround enabled with only "game" in channels.
        // "media" was previously routed to surround but is now removed.
        let cfg = make_config_surround_enabled("test-hrir"); // channels = ["game", "media"]
                                                             // We'll override channels to just ["game"] below.

        // Runner calls for apply_surround with surround_routed = {"game", "media"},
        // desired = {"game"}, to_restore = ["media"], to_route = [] (game already tracked):
        //
        // recreate surround (absent) — recreate_ex():
        //   remove: source_exists → 1 ls (absent, no destroy)
        //   spawn_conf: writes conf + spawns directly (no second runner call)
        //
        // restore media (Arctis_Media present → destroy + pkill, then create absent → spawn):
        //   remove: sink_exists → 1 ls (present), find_node_id → 1 ls, destroy, pkill
        //   create: sink_exists → 1 ls (absent) → spawn
        let ls_channels = ls_all_present();
        let ls_absent = ls_all_absent();

        let runner = MockRunner::new()
            // recreate surround: remove source_exists (absent)
            .with_output(0, &ls_absent, "")
            // restore media: remove sink_exists (present)
            .with_output(0, &ls_channels, "")
            // restore media: remove find_node_id
            .with_output(0, &ls_channels, "")
            // restore media: destroy
            .with_output(0, "", "")
            // restore media: pkill
            .with_output(0, "", "")
            // restore media: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "");

        let mut engine = Engine::new(runner, cfg);
        // Pre-populate surround_routed to simulate prior state.
        engine.surround_routed.insert("game".into());
        engine.surround_routed.insert("media".into());

        // Mutate config so surround only has "game".
        {
            let p = engine.config.profile_mut("default").unwrap();
            p.surround.channels = vec!["game".into()];
        }

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        // surround_routed must now only contain "game" (media was removed).
        assert!(
            engine.surround_routed.contains("game"),
            "game must remain in surround_routed"
        );
        assert!(
            !engine.surround_routed.contains("media"),
            "media must be removed from surround_routed after restore"
        );
        assert_eq!(
            engine.surround_routed.len(),
            1,
            "exactly 1 channel in surround_routed"
        );

        // media was respawned (set_output → recreate → create → spawn)
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("Arctis_Media"))
                .unwrap_or(false)),
            "Arctis_Media must be respawned to restore from surround routing: {:?}",
            engine.runner.spawned
        );

        // surround sink was recreated (spawned)
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("arctis_surround"))
                .unwrap_or(false)),
            "surround sink must be respawned: {:?}",
            engine.runner.spawned
        );

        let _ = std::fs::remove_dir_all(&tmp_home);
        std::env::remove_var("HOME");
    }

    /// C2 fix: apply_surround disabled path restores channels with output_device=None.
    ///
    /// Setup: `surround_routed = {"game"}` (simulating prior enable), config has
    /// `surround.enabled = false`. After apply_surround: game must be restored
    /// (even though output_device = None), surround_routed = {}.
    #[test]
    fn apply_surround_disabled_restores_stale_channel_with_no_output_device() {
        scrub_stale_confs();
        // Config: surround disabled (default). Game channel has output_device = None.
        let cfg = make_config_no_eq_no_routes(); // surround.enabled = false

        // Runner calls for apply_surround with surround_routed = {"game"},
        // surround disabled:
        //
        // remove surround (absent → no destroy):
        //   source_exists → 1 ls (absent)
        //
        // restore game (Arctis_Game present, then recreate):
        //   remove: sink_exists → 1 ls (present), find_node_id → 1 ls, destroy, pkill
        //   create: sink_exists → 1 ls (absent) → spawn
        let ls_channels = ls_all_present();
        let ls_absent = ls_all_absent();

        let runner = MockRunner::new()
            // remove surround: source_exists (absent)
            .with_output(0, &ls_absent, "")
            // restore game: remove sink_exists (present)
            .with_output(0, &ls_channels, "")
            // restore game: remove find_node_id
            .with_output(0, &ls_channels, "")
            // restore game: destroy
            .with_output(0, "", "")
            // restore game: pkill
            .with_output(0, "", "")
            // restore game: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "");

        let mut engine = Engine::new(runner, cfg);
        // Simulate prior enable: game was routed to surround.
        engine.surround_routed.insert("game".into());

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        // surround_routed must be empty after disable.
        assert!(
            engine.surround_routed.is_empty(),
            "surround_routed must be empty after disable: {:?}",
            engine.surround_routed
        );

        // game channel was respawned (restored even with output_device = None).
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("Arctis_Game"))
                .unwrap_or(false)),
            "Arctis_Game must be respawned after disable even with output_device=None: {:?}",
            engine.runner.spawned
        );

        // Verify destroy was called (channel was present, then destroyed and recreated).
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c.len() >= 3 && c[1] == "destroy"),
            "destroy must be called when restoring the channel"
        );
    }

    /// Enable → disable flow: all listed channels are restored after disable.
    ///
    /// This tests the full round-trip:
    /// 1. surround_set_enabled(true) → populates surround_routed
    /// 2. surround_set_enabled(false) → drains surround_routed + restores channels
    #[test]
    fn surround_set_enabled_disable_restores_all_channels() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp_home = unique_cfg_tmp("surr_roundtrip_home");
        let tmp_cfg = unique_cfg_tmp("surr_roundtrip_cfg");
        let profiles_dir = tmp_home.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp_home);
        std::env::set_var("ASM_CONFIG_HOME", &tmp_cfg);

        // Config: surround disabled, channels = ["game", "media"] (will be enabled).
        // Start with surround disabled so surround_set_enabled(true) is meaningful.
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround = arctis_config::SurroundConfig {
            enabled: false,
            hrir: Some("test-hrir".into()),
            channels: vec!["game".into(), "media".into()],
            hw_sink: None,
            ..Default::default()
        };

        // Phase 1: surround_set_enabled(true) → apply_surround(enabled):
        //   recreate surround (recreate_ex): remove (absent, 1 ls) + spawn_conf (no ls) = 1 ls + spawn
        //   to_route = ["game", "media"] (both not yet tracked):
        //     route game: remove (absent) + create (absent → spawn) = 2 ls + spawn
        //     route media: remove (absent) + create (absent → spawn) = 2 ls + spawn
        //
        // Phase 2: surround_set_enabled(false) → apply_surround(disabled):
        //   remove surround: source_exists (absent) = 1 ls
        //   restore game (was present from spawn above — but MockRunner can't distinguish;
        //     we'll report game as absent for simplicity → remove is no-op, create spawns)
        //   restore media: same
        //
        // For simplicity, we report all channels/surround as absent throughout.
        let ls_absent = ls_all_absent();

        let runner = MockRunner::new()
            // --- Phase 1: surround_set_enabled(true) ---
            // recreate surround (recreate_ex): remove source_exists (absent) — spawn_conf has no ls
            .with_output(0, &ls_absent, "")
            // route game: remove sink_exists (absent)
            .with_output(0, &ls_absent, "")
            // route game: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "")
            // route media: remove sink_exists (absent)
            .with_output(0, &ls_absent, "")
            // route media: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "")
            // --- Phase 2: surround_set_enabled(false) ---
            // remove surround: source_exists (absent)
            .with_output(0, &ls_absent, "")
            // restore game: remove sink_exists (absent)
            .with_output(0, &ls_absent, "")
            // restore game: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "")
            // restore media: remove sink_exists (absent)
            .with_output(0, &ls_absent, "")
            // restore media: create sink_exists (absent → spawn)
            .with_output(0, &ls_absent, "");

        let mut engine = Engine::new(runner, cfg);

        // Enable surround.
        engine
            .surround_set_enabled(true)
            .expect("enable must succeed");

        // After enable: surround_routed must contain both channels.
        assert!(
            engine.surround_routed.contains("game"),
            "game must be in surround_routed after enable"
        );
        assert!(
            engine.surround_routed.contains("media"),
            "media must be in surround_routed after enable"
        );

        // Disable surround.
        engine
            .surround_set_enabled(false)
            .expect("disable must succeed");

        // After disable: surround_routed must be empty.
        assert!(
            engine.surround_routed.is_empty(),
            "surround_routed must be empty after disable: {:?}",
            engine.surround_routed
        );

        // game and media channels must have been respawned (restored).
        let game_spawns = engine
            .runner
            .spawned
            .iter()
            .filter(|argv| {
                argv.get(2)
                    .map(|s| s.contains("Arctis_Game"))
                    .unwrap_or(false)
            })
            .count();
        let media_spawns = engine
            .runner
            .spawned
            .iter()
            .filter(|argv| {
                argv.get(2)
                    .map(|s| s.contains("Arctis_Media"))
                    .unwrap_or(false)
            })
            .count();

        // game: spawned on route (enable) + spawned on restore (disable) = 2
        assert!(
            game_spawns >= 2,
            "game must be spawned at least twice (route + restore): {game_spawns}"
        );
        // media: same
        assert!(
            media_spawns >= 2,
            "media must be spawned at least twice (route + restore): {media_spawns}"
        );

        let _ = std::fs::remove_dir_all(&tmp_home);
        let _ = std::fs::remove_dir_all(&tmp_cfg);
        std::env::remove_var("HOME");
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// I3 fix: surround_set_hrir routes through apply_surround (not inlined recreate).
    ///
    /// When surround is ENABLED and HRIR changes, apply_surround is called.
    /// Since channels are already in surround_routed, to_route and to_restore are both empty
    /// → only the sink is recreated (no channel thrash).
    #[test]
    fn surround_set_hrir_enabled_uses_apply_surround_no_channel_thrash() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp_home = unique_cfg_tmp("surr_hrir_i3_home");
        let tmp_cfg = unique_cfg_tmp("surr_hrir_i3_cfg");
        let profiles_dir = tmp_home.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("old-hrir.wav"), b"").unwrap();
        std::fs::write(profiles_dir.join("new-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp_home);
        std::env::set_var("ASM_CONFIG_HOME", &tmp_cfg);

        // Config: surround enabled, channels = ["game"].
        let cfg = make_config_surround_enabled("old-hrir");

        // Pre-populate surround_routed to simulate that "game" is already routed.
        // apply_surround will find to_route = [], to_restore = [] → only sink recreated.
        //
        // recreate surround (recreate_ex, absent):
        //   remove: source_exists → 1 ls (absent)
        //   spawn_conf: writes conf + spawns directly (no second runner call)
        // No channel operations.
        let ls_absent = ls_all_absent();

        let runner = MockRunner::new()
            // recreate surround: remove source_exists (absent)
            .with_output(0, &ls_absent, "");

        let mut engine = Engine::new(runner, cfg);
        // Simulate that channels are already tracked (prior enable).
        engine.surround_routed.insert("game".into());
        engine.surround_routed.insert("media".into());

        engine
            .surround_set_hrir("new-hrir".into())
            .expect("surround_set_hrir must succeed");

        // Surround sink was respawned (HRIR change took effect).
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("arctis_surround"))
                .unwrap_or(false)),
            "surround sink must be respawned after HRIR change: {:?}",
            engine.runner.spawned
        );

        // No channel thrash: game and media were NOT respawned (already tracked).
        assert!(
            !engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("Arctis_Game") || s.contains("Arctis_Media"))
                .unwrap_or(false)),
            "channels must NOT be thrashed when HRIR changes (already in surround_routed): {:?}",
            engine.runner.spawned
        );

        // surround_routed unchanged.
        assert!(engine.surround_routed.contains("game"));
        assert!(engine.surround_routed.contains("media"));

        let _ = std::fs::remove_dir_all(&tmp_home);
        let _ = std::fs::remove_dir_all(&tmp_cfg);
        std::env::remove_var("HOME");
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    // ─────────────────────────────────────────────
    // Task B6b: surround mode-selection + EQ-on-binaural-output
    // ─────────────────────────────────────────────

    /// Helper: make a config where "game" is the single surround channel with empty EQ.
    fn make_config_surround_single_game_no_eq(hrir_stem: &str) -> Config {
        Config {
            version: arctis_config::CURRENT_VERSION,
            active_profile: "default".into(),
            profiles: vec![Profile {
                name: "default".into(),
                channels: vec![ChannelConfig {
                    id: "game".into(),
                    node_name: "Arctis_Game".into(),
                    description: "Game".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                }],
                routes: vec![],
                mic: MicChainConfig::default(),
                surround: arctis_config::SurroundConfig {
                    enabled: true,
                    hrir: Some(hrir_stem.into()),
                    channels: vec!["game".into()],
                    hw_sink: None,
                    ..Default::default()
                },
                master_volume_db: 0.0,
                master_volume_pct: 100,
                master_mute: false,
                chatmix_position: 4,
                default_sink_channel: None,
            }],
            eq_presets: vec![],
            dial_controls_balance: true,
            knob_controls_master: true,
        }
    }

    /// Queue 3 MockRunner outputs for a direct `apply_surround` call where surround and
    /// the channel sink are both absent:
    ///   recreate_ex: remove source_exists (absent, 1 ls) + spawn_conf (no ls) = 1 ls
    ///   set_output game: remove sink_exists (absent) + create sink_exists (absent → spawn) = 2 ls
    fn queue_apply_surround_both_absent(runner: MockRunner) -> MockRunner {
        scrub_stale_confs();
        let ls_absent = ls_all_absent();
        runner
            .with_output(0, &ls_absent, "") // recreate_ex: remove source_exists (absent)
            .with_output(0, &ls_absent, "") // set_output game: remove sink_exists (absent)
            .with_output(0, &ls_absent, "") // set_output game: create sink_exists (absent → spawn)
    }

    /// Back-compat: a default profile (game channel, NO custom EQ) with surround enabled
    /// must produce an 8-channel HRIR conf WITHOUT an EQ tail — exactly as before this task.
    #[test]
    fn apply_surround_default_profile_renders_8ch_no_eq_conf() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("b6b_no_eq");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        let runner = queue_apply_surround_both_absent(MockRunner::new());
        let cfg = make_config_surround_single_game_no_eq("test-hrir");
        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        // Find the surround conf spawn (path contains "arctis_surround").
        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| {
                argv.get(2)
                    .map(|s| s.contains("arctis_surround"))
                    .unwrap_or(false)
            })
            .expect("surround conf must have been spawned");
        let conf = std::fs::read_to_string(&surround_argv[2])
            .expect("surround conf file must exist");

        // 8-channel HRIR (default Auto → hrir71).
        assert!(
            conf.contains("audio.channels = 8"),
            "default profile must produce an 8-channel HRIR conf, got:\n{conf}"
        );
        // No EQ tail: game channel has empty eq → output_eq=None → no eq nodes in conf.
        assert!(
            !conf.contains("\"eq_l_0\""),
            "no EQ tail expected when game channel has no custom EQ, got:\n{conf}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    /// The HRIR convolver's OUTPUT must reach the headset. When `surround.hw_sink` is
    /// unset (the common case — profiles store None = "auto → headset"), apply_surround
    /// must default the convolver output to the DETECTED headset sink, mirroring how
    /// reconcile defaults each channel's output via `overlay_default_output`. Otherwise
    /// the convolver output falls to the system default sink (e.g. onboard speakers) and
    /// the HRIR-processed audio never reaches the headphones.
    #[test]
    fn apply_surround_defaults_convolver_output_to_detected_headset_when_hw_sink_unset() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surround_hwsink_default");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        // Runner: detect_headset_sink probes first (pw-metadata 0, then a pw-dump of real
        // sinks that includes the SteelSeries Arctis hw sink), then the recreate/set_output
        // existence checks (absent → spawn).
        let sinks_dump = include_str!("../../../../../audio/tests/fixtures/pw_dump_sinks.json");
        let runner = queue_apply_surround_both_absent(
            MockRunner::new()
                .with_output(0, "[]", "") // Auto probe: pw-dump (no streams)
                .with_output(0, PW_METADATA_SINK, "") // detect: pw-metadata 0 (default sink)
                .with_output(0, sinks_dump, ""), // detect: pw-dump (real sinks)
        );
        let cfg = make_config_surround_single_game_no_eq("test-hrir"); // surround.hw_sink = None
        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| {
                argv.get(2)
                    .map(|s| s.contains("arctis_surround"))
                    .unwrap_or(false)
            })
            .expect("surround conf must have been spawned");
        let conf =
            std::fs::read_to_string(&surround_argv[2]).expect("surround conf file must exist");

        assert!(
            conf.contains("target.object")
                && conf.contains(
                    "alsa_output.usb-SteelSeries_Arctis_Nova_Pro_Wireless-00.analog-stereo"
                ),
            "convolver output must target the detected headset, got:\n{conf}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    /// One-EQ-per-channel: a profile whose "game" channel has a non-empty EQ must
    /// (a) produce a surround conf with NO eq_l_0 / eq_r_0 tail nodes (no config-driven
    ///     post-convolution EQ), AND
    /// (b) route the game channel sink WITH its own EQ (custom band gain present in the
    ///     channel conf) — i.e. the channel EQ is applied pre-convolution.
    #[test]
    fn apply_surround_game_eq_stays_on_channel_sink_no_tail() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("b6b_eq_tail");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        // Use a distinctive gain value (6.0 dB) to identify whether the custom EQ ends up
        // in the surround conf tail vs. the channel sink conf.  The conf renderer uses
        // `fmt_num` which formats integral-valued floats as "{:.1}" → "6.0", NOT "6".
        // Using "6.0" is safe: it does not appear in band-index names like "eq_band_6".
        let custom_gain: f32 = 6.0;
        let gain_str_in_conf = format!("{custom_gain:.1}"); // "6.0"
        let mut cfg = make_config_surround_single_game_no_eq("test-hrir");
        cfg.profiles[0].channels[0].eq = vec![arctis_config::EqBandConfig {
            kind: "peaking".into(),
            freq_hz: 1000.0,
            q: 1.0,
            gain_db: custom_gain,
        }];

        let runner = queue_apply_surround_both_absent(MockRunner::new());
        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        // (a) Surround conf must have NO EQ tail nodes (no config-driven post-conv EQ).
        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| {
                argv.get(2)
                    .map(|s| s.contains("arctis_surround"))
                    .unwrap_or(false)
            })
            .expect("surround conf must have been spawned");
        let surround_conf = std::fs::read_to_string(&surround_argv[2])
            .expect("surround conf file must exist");
        assert!(
            !surround_conf.contains("\"eq_l_0\""),
            "surround conf must NOT contain eq_l_0 tail node (channel EQ stays pre-convolution)"
        );

        // (b) Game channel sink conf must carry its own custom gain (EQ applied pre-conv).
        // The channel conf is at {tmp_dir}/arctis_eq.Arctis_Game.conf.
        let game_conf_path = std::env::temp_dir().join("arctis_eq.Arctis_Game.conf");
        let game_conf = std::fs::read_to_string(&game_conf_path)
            .expect("game channel conf file must exist");
        assert!(
            game_conf.contains(&gain_str_in_conf),
            "game channel sink conf MUST contain custom gain {gain_str_in_conf} (EQ pre-convolution), conf:\n{game_conf}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    /// A pinned `blocksize` must be carried into the spawned surround conf, and — since the
    /// pinned HRIR is present — `state().surround.hrir_missing` must stay `None`. There is
    /// no config-driven post-convolution EQ tail.
    #[test]
    fn apply_surround_carries_blocksize_and_clears_missing_no_tail() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("t6_explicit_eq");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("g.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        let mut cfg = make_config_surround_single_game_no_eq("g");
        cfg.profiles[0].surround.blocksize = Some(128);

        let runner = queue_apply_surround_both_absent(MockRunner::new());
        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        // HRIR present → no missing flag.
        assert_eq!(engine.state().surround.hrir_missing, None);

        // Inspect the spawned surround conf (established mechanism: find the "arctis_surround"
        // spawn argv and read the conf file at argv[2]).
        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| {
                argv.get(2)
                    .map(|s| s.contains("arctis_surround"))
                    .unwrap_or(false)
            })
            .expect("surround conf must have been spawned");
        let conf = std::fs::read_to_string(&surround_argv[2])
            .expect("surround conf file must exist");
        assert!(
            conf.contains("blocksize = 128"),
            "surround conf must carry pinned blocksize, got:\n{conf}"
        );
        assert!(
            !conf.contains("\"eq_l_0\""),
            "surround conf must NOT carry an EQ tail (no config-driven post-conv EQ), got:\n{conf}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    /// A pinned HRIR stem that is not installed must fall back to the bundled dry HRIR
    /// (`07-oal+++-openal-max`) and record the missing stem in `state().surround.hrir_missing`.
    #[test]
    fn apply_surround_missing_pinned_hrir_falls_back_and_sets_flag() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("t6_missing_hrir");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        // Only the bundled fallback is present; the pinned stem is absent.
        std::fs::write(profiles_dir.join("07-oal+++-openal-max.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        let cfg = make_config_surround_single_game_no_eq("04-gsx-sennheiser-gsx");
        let runner = queue_apply_surround_both_absent(MockRunner::new());
        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround must succeed");

        assert_eq!(
            engine.state().surround.hrir_missing,
            Some("04-gsx-sennheiser-gsx".to_string())
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    /// StereoBypass mode: spawns a 2-channel conf with no convolver, no HRIR path needed.
    #[test]
    fn apply_surround_stereo_bypass_mode_spawns_2ch_conf() {
        // Hold ENV_MUTEX to serialize with other tests that write to /tmp/arctis_arctis_surround.conf
        // even though StereoBypass doesn't need HOME (no HRIR resolution).
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let runner = queue_apply_surround_both_absent(MockRunner::new());
        let mut cfg = make_config_surround_single_game_no_eq("unused-hrir");
        cfg.profiles[0].surround.mode = SurroundMode::StereoBypass;

        let mut engine = Engine::new(runner, cfg);

        let profile = engine.config.active().unwrap().clone();
        engine
            .apply_surround(&profile)
            .expect("apply_surround with StereoBypass must succeed");

        let surround_argv = engine
            .runner
            .spawned
            .iter()
            .find(|argv| {
                argv.get(2)
                    .map(|s| s.contains("arctis_surround"))
                    .unwrap_or(false)
            })
            .expect("surround conf must have been spawned");
        let conf = std::fs::read_to_string(&surround_argv[2])
            .expect("surround conf file must exist");

        assert!(
            conf.contains("audio.channels = 2"),
            "StereoBypass mode must produce a 2-channel conf, got:\n{conf}"
        );
        assert!(
            !conf.contains("convolver"),
            "StereoBypass mode conf must NOT contain any convolver node, got:\n{conf}"
        );
    }

    /// state() surround snapshot must expose `mode` and `effective_mode` strings.
    /// Default config has mode=Auto → effective_mode="hrir71" (no negotiated channels).
    #[test]
    fn surround_snapshot_exposes_mode_and_effective_mode() {
        // Use a config with surround enabled and default mode (Auto).
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround = arctis_config::SurroundConfig {
            enabled: true,
            hrir: None,
            channels: vec!["game".into()],
            hw_sink: None,
            ..Default::default() // mode = Auto, crossfeed = 0
        };

        let mut engine = Engine::new(MockRunner::new(), cfg);
        let st = engine.state();

        // mode must reflect the configured value.
        assert_eq!(
            st.surround.mode, "auto",
            "mode must be 'auto' for default SurroundConfig, got: {:?}",
            st.surround.mode
        );
        // effective_mode for Auto + no negotiated channels → Hrir71 → "hrir71".
        assert_eq!(
            st.surround.effective_mode, "hrir71",
            "effective_mode must be 'hrir71' when mode=Auto and no negotiated count, got: {:?}",
            st.surround.effective_mode
        );
        // negotiated_channels not yet probed → None.
        assert_eq!(
            st.surround.negotiated_channels, None,
            "negotiated_channels must be None before any pw-dump probe"
        );
    }

    #[test]
    fn surround_snapshot_stereo_bypass_mode_string() {
        // Set mode explicitly to StereoBypass and verify the snapshot produces "stereo_bypass".
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround = arctis_config::SurroundConfig {
            enabled: true,
            mode: arctis_config::SurroundMode::StereoBypass,
            hrir: None,
            channels: vec!["game".into()],
            hw_sink: None,
            ..Default::default()
        };

        let mut engine = Engine::new(MockRunner::new(), cfg);
        let st = engine.state();

        // mode must reflect StereoBypass as "stereo_bypass" (snake_case, not "stereobypass").
        assert_eq!(
            st.surround.mode, "stereo_bypass",
            "mode must be 'stereo_bypass' for SurroundMode::StereoBypass, got: {:?}",
            st.surround.mode
        );
        // effective_mode for explicit StereoBypass → "stereo_bypass".
        assert_eq!(
            st.surround.effective_mode, "stereo_bypass",
            "effective_mode must be 'stereo_bypass' when mode=StereoBypass, got: {:?}",
            st.surround.effective_mode
        );
    }

    #[test]
    fn state_reports_negotiated_surround_input_for_game_channel() {
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround.enabled = true;
        cfg.profiles[0].surround.channels = vec!["game".into()];

        // DayZ 7.1 stream linked to the Arctis_Game sink.
        let dump = r#"[
          { "id": 50, "type": "PipeWire:Interface:Node",
            "info": { "props": { "media.class": "Audio/Sink", "node.name": "Arctis_Game" } } },
          { "id": 51, "type": "PipeWire:Interface:Node",
            "info": { "props": { "media.class": "Stream/Output/Audio",
                "application.name": "DayZ", "application.process.binary": "DayZ" },
              "params": { "Format": [ { "channels": 8,
                "position": ["FL","FR","FC","LFE","RL","RR","SL","SR"] } ] } } },
          { "id": 99, "type": "PipeWire:Interface:Link",
            "info": { "output-node-id": 51, "input-node-id": 50 } }
        ]"#;
        let runner = MockRunner::new().with_output(0, dump, "");
        let mut engine = Engine::new(runner, cfg);
        let st = engine.state();
        assert_eq!(st.surround.negotiated_channels, Some(8));
        assert_eq!(st.surround.negotiated_surround, Some(true));
    }

    #[test]
    fn state_reports_none_surround_input_when_no_game_stream() {
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround.enabled = true;
        cfg.profiles[0].surround.channels = vec!["game".into()];
        // pw-dump with no app streams.
        let runner = MockRunner::new().with_output(0, "[]", "");
        let mut engine = Engine::new(runner, cfg);
        let st = engine.state();
        assert_eq!(st.surround.negotiated_channels, None);
        assert_eq!(st.surround.negotiated_surround, None);
    }
