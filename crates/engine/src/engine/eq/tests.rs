    use super::*;
    use crate::engine::test_support::*;

    #[test]
    fn set_eq_band_persists_and_applies_live() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("eq_band");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();

        // set_eq_band("game", 2, band) → apply_band:
        //   apply_band calls find_node_id (1 ls) + 1 pw-cli s <id> Props
        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // find_node_id ls
            .with_output(0, "", ""); // pw-cli s <id> Props

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        let band_cfg = EqBandConfig {
            kind: "peaking".to_string(),
            freq_hz: 1000.0,
            q: 1.0,
            gain_db: 3.0,
        };
        engine
            .set_eq_band("game", 2, band_cfg)
            .expect("set_eq_band should succeed");

        // Config persisted to disk
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");

        // pw-cli s <id> Props was called for band 2
        let calls = &engine.runner.calls;
        assert!(
            calls
                .iter()
                .any(|c| c.len() >= 4 && c[1] == "s" && c[3] == "Props"),
            "must issue pw-cli s <id> Props for band set"
        );

        // Event received
        let event = rx.try_recv().expect("EqBandSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::EqBandSet {
                channel_id: "game".to_string(),
                band: 2,
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Bugfix: changing a band's filter TYPE (kind) must REBUILD the channel sink
    /// (the bq_* node label is fixed at chain-build time and can't be live-set),
    /// so the audible filter matches the on-screen curve without a daemon restart.
    #[test]
    fn set_eq_band_kind_change_rebuilds_sink() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("eq_band_kind_change");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // game band 0 is stored as "peaking"; we change it to "lowshelf".
        let cfg = make_config_with_eq_bands();

        // recreate() = remove() [sink_exists ls + find_node_id ls + destroy + pkill]
        //            + create() [sink_exists ls]. Queue two present ls so remove
        // proceeds to destroy; create's sink_exists then sees the default-empty
        // output → absent → spawn_owned a fresh `pipewire -c <Game.conf>`.
        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // remove: sink_exists (present)
            .with_output(0, &ls, ""); // remove: find_node_id (present)

        let mut engine = Engine::new(runner, cfg);

        let band_cfg = EqBandConfig {
            kind: "lowshelf".to_string(), // CHANGED from "peaking"
            freq_hz: 100.0,
            q: 1.0,
            gain_db: 3.0,
        };
        engine
            .set_eq_band("game", 0, band_cfg)
            .expect("set_eq_band kind-change should succeed");

        // A fresh filter-chain instance was spawned for the game channel → rebuild.
        let spawned = &engine.runner.spawned;
        assert_eq!(spawned.len(), 1, "kind change must rebuild the channel sink");
        assert_eq!(spawned[0][0], "pipewire");
        assert!(
            spawned[0][2].ends_with("Arctis_Game.conf"),
            "rebuild must respawn the Game channel conf, got {:?}",
            spawned[0]
        );
        // The teardown half of recreate ran (destroy the old node).
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c.len() >= 2 && c[0] == "pw-cli" && c[1] == "destroy"),
            "kind change must destroy the old node as part of the rebuild"
        );
        // The new child token is tracked for shutdown reaping.
        assert_eq!(
            engine.children.len(),
            1,
            "rebuild must track the new pipewire child"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Bugfix guard: a VALUE-ONLY edit (same kind, different gain) must keep the
    /// fast live `pw-cli s … Props` path — NO sink rebuild (no regression of G3).
    #[test]
    fn set_eq_band_value_only_change_keeps_live_apply() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("eq_band_value_only");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // game band 0 is stored as "peaking"; we keep "peaking", change only gain.
        let cfg = make_config_with_eq_bands();

        // apply_band: find_node_id (1 ls) + 1 pw-cli s <id> Props.
        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // find_node_id
            .with_output(0, "", ""); // pw-cli s <id> Props

        let mut engine = Engine::new(runner, cfg);

        let band_cfg = EqBandConfig {
            kind: "peaking".to_string(), // SAME kind
            freq_hz: 100.0,
            q: 1.0,
            gain_db: 9.0, // changed value only
        };
        engine
            .set_eq_band("game", 0, band_cfg)
            .expect("set_eq_band value-only should succeed");

        // No rebuild: no spawn, no destroy, no child tracked.
        assert!(
            engine.runner.spawned.is_empty(),
            "value-only edit must NOT rebuild the sink"
        );
        assert!(
            !engine
                .runner
                .calls
                .iter()
                .any(|c| c.len() >= 2 && c[0] == "pw-cli" && c[1] == "destroy"),
            "value-only edit must NOT destroy the node"
        );
        assert_eq!(engine.children.len(), 0, "value-only edit tracks no child");
        // The live Props set DID run.
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c.len() >= 4 && c[1] == "s" && c[3] == "Props"),
            "value-only edit must issue the live pw-cli s <id> Props"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn save_eq_preset_captures_channel_bands_into_named_preset() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("eq_preset_save");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_no_eq_no_routes();
        // Give the game channel two EQ bands
        cfg.profiles[0].channels[0].eq = vec![
            EqBandConfig {
                kind: "peaking".into(),
                freq_hz: 200.0,
                q: 1.0,
                gain_db: 4.0,
            },
            EqBandConfig {
                kind: "highshelf".into(),
                freq_hz: 6000.0,
                q: 0.7,
                gain_db: -2.0,
            },
        ];

        let mut engine = Engine::new(MockRunner::new(), cfg);

        engine
            .save_eq_preset("my-preset", "game")
            .expect("save_eq_preset should succeed");

        // Preset must exist in config with the correct bands
        let preset = engine
            .config()
            .eq_presets
            .iter()
            .find(|p| p.name == "my-preset")
            .expect("preset must exist in config after save");
        assert_eq!(preset.bands.len(), 2, "preset must have 2 bands");
        assert_eq!(preset.bands[0].freq_hz, 200.0, "first band freq must match");
        assert_eq!(preset.bands[0].gain_db, 4.0, "first band gain must match");
        assert_eq!(
            preset.bands[1].kind, "highshelf",
            "second band kind must match"
        );

        // Persisted to disk
        assert!(
            tmp.join("config.toml").exists(),
            "config.toml must be written"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn apply_eq_preset_copies_bands_to_channel_and_persists() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("eq_preset_apply");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let preset_bands = vec![EqBandConfig {
            kind: "peaking".into(),
            freq_hz: 500.0,
            q: 1.5,
            gain_db: 6.0,
        }];

        let mut cfg = make_config_no_eq_no_routes();
        // Inject the preset directly into config
        cfg.eq_presets.push(arctis_config::EqPreset {
            name: "test-preset".into(),
            kind_hint: None,
            bands: preset_bands.clone(),
        });

        // apply_eq_preset → apply_all: 1 ls (find_node_id) + 10 band set calls
        let ls = ls_all_present();
        let mut runner = MockRunner::new();
        runner = runner.with_output(0, &ls, ""); // find_node_id
        for _ in 0..10 {
            runner = runner.with_output(0, "", ""); // band Props sets
        }

        let mut engine = Engine::new(runner, cfg);

        engine
            .apply_eq_preset("test-preset", "game")
            .expect("apply_eq_preset should succeed");

        // Channel EQ in config must equal preset bands
        let active = engine.config().active_profile.clone();
        let profile = engine
            .config()
            .profile(&active)
            .expect("active profile must exist");
        let channel = profile
            .channels
            .iter()
            .find(|c| c.id == "game")
            .expect("game channel must exist");
        assert_eq!(channel.eq.len(), 1, "channel must have 1 band after apply");
        assert_eq!(
            channel.eq[0].freq_hz, 500.0,
            "channel band freq must match preset"
        );
        assert_eq!(
            channel.eq[0].gain_db, 6.0,
            "channel band gain must match preset"
        );

        // Persisted to disk
        assert!(
            tmp.join("config.toml").exists(),
            "config.toml must be written"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn apply_eq_preset_resolves_factory_name() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("fxeq_factory");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // No user presets — factory catalog must be consulted
        let cfg = make_config_no_eq_no_routes();

        // "Bass Boost" has 10 bands → apply_all: 1 ls (find_node_id) + 10 band set calls
        let ls = ls_all_present();
        let mut runner = MockRunner::new();
        runner = runner.with_output(0, &ls, "");
        for _ in 0..10 {
            runner = runner.with_output(0, "", "");
        }

        let mut engine = Engine::new(runner, cfg);
        engine
            .apply_eq_preset("Bass Boost", "game")
            .expect("factory preset should apply successfully");

        let st = engine.state();
        let game = st.channels.iter().find(|c| c.id == "game").unwrap();
        // "Bass Boost" band 0 is lowshelf/31 Hz/4.0 dB; default flat band is peaking/0.0 dB
        assert_eq!(game.eq_bands[0].kind, "lowshelf", "Bass Boost band 0 must be lowshelf");
        assert_eq!(
            game.eq_bands[0].gain_db, 4.0,
            "Bass Boost band 0 lowshelf gain must be 4.0 dB"
        );
        assert!(
            st.factory_eq_presets.iter().any(|p| p.name == "Reference (Calibrated)"),
            "state must expose factory catalog including Reference (Calibrated)"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn apply_eq_preset_unknown_name_errors() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("fxeq_unknown");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let result = engine.apply_eq_preset("NonExistent__XYZ", "game");
        assert!(
            matches!(result, Err(EngineError::BadRequest(_))),
            "unknown preset name must return BadRequest"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn apply_eq_preset_user_preset_wins_over_factory() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("fxeq_user_wins");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // User preset with same name as a factory preset — user must win
        let mut cfg = make_config_no_eq_no_routes();
        cfg.eq_presets.push(arctis_config::EqPreset {
            name: "Bass Boost".into(),
            kind_hint: None,
            bands: vec![EqBandConfig {
                kind: "peaking".into(),
                freq_hz: 100.0,
                q: 1.0,
                gain_db: 9.9,
            }],
        });

        // User preset has 1 band but apply_all runs 10 dense bands; queue all 10
        let ls = ls_all_present();
        let mut runner = MockRunner::new();
        runner = runner.with_output(0, &ls, "");
        for _ in 0..10 {
            runner = runner.with_output(0, "", "");
        }

        let mut engine = Engine::new(runner, cfg);
        engine
            .apply_eq_preset("Bass Boost", "game")
            .expect("user preset should apply");

        // Channel config must reflect user preset (1 band), not factory (10 bands)
        let active = engine.config().active_profile.clone();
        let profile = engine.config().profile(&active).unwrap();
        let ch = profile.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(ch.eq.len(), 1, "user preset (1 band) must win over factory (10 bands)");
        assert_eq!(ch.eq[0].freq_hz, 100.0, "user band freq must be preserved");
        assert_eq!(ch.eq[0].gain_db, 9.9, "user band gain must be preserved");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn delete_eq_preset_removes_it_from_config() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("eq_preset_delete");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_no_eq_no_routes();
        cfg.eq_presets.push(arctis_config::EqPreset {
            name: "to-delete".into(),
            kind_hint: None,
            bands: vec![],
        });

        let mut engine = Engine::new(MockRunner::new(), cfg);

        // Verify it exists before delete
        assert!(
            engine
                .config()
                .eq_presets
                .iter()
                .any(|p| p.name == "to-delete"),
            "preset must exist before delete"
        );

        engine
            .delete_eq_preset("to-delete")
            .expect("delete_eq_preset should succeed");

        // Must be gone from config
        assert!(
            !engine
                .config()
                .eq_presets
                .iter()
                .any(|p| p.name == "to-delete"),
            "preset must be removed from config after delete"
        );

        // Must be gone from state
        assert!(
            !engine
                .state()
                .eq_presets
                .iter()
                .any(|p| p.name == "to-delete"),
            "preset must not appear in state after delete"
        );

        // Persisted to disk
        assert!(
            tmp.join("config.toml").exists(),
            "config.toml must be written"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    // ─────────────────────────────────────────────
    // Task 1: dense fixed-10-band EQ model
    // ─────────────────────────────────────────────

    #[test]
    fn state_returns_ten_dense_bands_for_flat_channel() {
        let mut engine = Engine::new(arctis_audio::MockRunner::new(), make_config_no_eq_no_routes());
        let st = engine.state();
        let game = st.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(game.eq_bands.len(), 10, "flat channel must report 10 dense bands");
        assert_eq!(game.eq_bands[0].freq_hz, 31.0);
        assert_eq!(game.eq_bands[9].freq_hz, 16000.0);
        assert!(game.eq_bands.iter().all(|b| b.gain_db == 0.0));
        // Dense defaults: shelves at the extremes, peaking in the middle.
        assert_eq!(game.eq_bands[0].kind, "lowshelf");
        assert_eq!(game.eq_bands[9].kind, "highshelf");
        assert!(game.eq_bands[1..9].iter().all(|b| b.kind == "peaking"));
    }

    #[test]
    fn set_eq_band_seeds_dense_defaults_no_1000hz_padding() {
        // Editing band index 3 first must NOT create 1000 Hz placeholders at 0..2.
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("eq_band_dense_seed");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        // apply_band call sequence: find_node_id (pw-cli ls Node) + pw-cli s <id> Props.
        // Provide a proper ls response so find_node_id can resolve "Arctis_Game" to id 10.
        let ls = ls_all_present();
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, &ls, "") // find_node_id: pw-cli ls Node
            .with_output(0, "", ""); // pw-cli s <id> Props
        let mut engine = Engine::new(runner, cfg);
        let band = arctis_config::EqBandConfig {
            kind: "peaking".into(), freq_hz: 250.0, q: 1.4, gain_db: 3.0,
        };
        engine.set_eq_band("game", 3, band).unwrap();
        let st = engine.state();
        let game = st.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(game.eq_bands.len(), 10);
        // Band 3 is the edit; bands 0-2 are canonical defaults (NOT 1000 Hz).
        assert_eq!(game.eq_bands[3].freq_hz, 250.0);
        assert_eq!(game.eq_bands[3].gain_db, 3.0);
        assert_eq!(game.eq_bands[0].freq_hz, 31.0, "band 0 must be canonical default, not 1000 Hz");
        assert_eq!(game.eq_bands[1].freq_hz, 62.0);
        assert_eq!(game.eq_bands[2].freq_hz, 125.0);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_eq_band_rejects_out_of_range_index() {
        let mut engine = Engine::new(arctis_audio::MockRunner::new(), make_config_no_eq_no_routes());
        let band = arctis_config::EqBandConfig { kind: "peaking".into(), freq_hz: 1000.0, q: 1.0, gain_db: 0.0 };
        assert!(engine.set_eq_band("game", 10, band).is_err());
    }
