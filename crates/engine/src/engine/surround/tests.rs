    use super::*;
    use crate::engine::test_support::*;

    mod apply_tests;

    // ─────────────────────────────────────────────
    // Surround mode fallback tests
    // ─────────────────────────────────────────────

    #[test]
    fn auto_mode_maps_negotiated_channels_to_path() {
        use SurroundMode::*;
        assert!(matches!(resolve_effective_mode(Auto, Some(8)), Hrir71));
        assert!(matches!(resolve_effective_mode(Auto, Some(6)), Hrir51));
        assert!(matches!(resolve_effective_mode(Auto, Some(2)), StereoBypass));
        assert!(matches!(resolve_effective_mode(StereoBypass, Some(8)), StereoBypass));
        assert!(matches!(resolve_effective_mode(Auto, None), Hrir71));
    }

    // ─────────────────────────────────────────────
    // F1.3 TDD: resolve_hrir_path + available_hrirs
    // ─────────────────────────────────────────────

    use crate::convert;

    // These tests inject the base dir directly — NO env mutation, NO ENV_MUTEX needed.
    // This is the canonical pattern: resolve_hrir_path / available_hrirs take base_dir
    // as a parameter so tests never race on process-global HOME.

    #[test]
    fn resolve_hrir_path_with_named_stem_returns_abs_path() {
        let tmp = unique_cfg_tmp("hrir_named");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        let wav = profiles_dir.join("00-default.wav");
        std::fs::write(&wav, b"").unwrap();

        let cfg = arctis_config::SurroundConfig {
            hrir: Some("00-default".into()),
            ..Default::default()
        };
        let path = convert::resolve_hrir_path(&cfg, &base).expect("should resolve named stem");
        assert_eq!(path, wav);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_hrir_path_named_stem_missing_returns_err() {
        let tmp = unique_cfg_tmp("hrir_missing");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();

        let cfg = arctis_config::SurroundConfig {
            hrir: Some("nonexistent".into()),
            ..Default::default()
        };
        let result = convert::resolve_hrir_path(&cfg, &base);
        assert!(matches!(result, Err(EngineError::BadRequest(_))));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_hrir_path_no_stem_picks_first_lexicographic() {
        let tmp = unique_cfg_tmp("hrir_lex");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("beta.wav"), b"").unwrap();
        std::fs::write(profiles_dir.join("alpha.wav"), b"").unwrap();

        let cfg = arctis_config::SurroundConfig::default(); // hrir = None
        let path =
            convert::resolve_hrir_path(&cfg, &base).expect("should pick first lexicographic");
        assert!(
            path.ends_with("alpha.wav"),
            "expected alpha.wav, got {:?}",
            path
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_hrir_path_no_stem_no_profiles_fallback_to_hrir_wav() {
        let tmp = unique_cfg_tmp("hrir_fallback");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("hrir.wav"), b"").unwrap();

        let cfg = arctis_config::SurroundConfig::default();
        let path = convert::resolve_hrir_path(&cfg, &base).expect("should fall back to hrir.wav");
        assert!(
            path.ends_with("hrir.wav"),
            "expected hrir.wav, got {:?}",
            path
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_hrir_path_no_hrir_at_all_returns_err() {
        let tmp = unique_cfg_tmp("hrir_none");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        std::fs::create_dir_all(&base).unwrap();

        let cfg = arctis_config::SurroundConfig::default();
        let result = convert::resolve_hrir_path(&cfg, &base);
        assert!(matches!(result, Err(EngineError::BadRequest(_))));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_or_fallback_present_stem_no_missing_flag() {
        let tmp = unique_cfg_tmp("hrir_or_fb_present");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("04-gsx-sennheiser-gsx.wav"), b"").unwrap();
        let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
        let (path, missing) = convert::resolve_hrir_path_or_fallback(&cfg, &base).expect("resolves");
        assert!(path.ends_with("04-gsx-sennheiser-gsx.wav"));
        assert_eq!(missing, None, "no fallback used → no missing flag");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_or_fallback_missing_pinned_uses_bundled_and_reports_missing() {
        let tmp = unique_cfg_tmp("hrir_or_fb_missing");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        // Pinned stem absent; bundled dry fallback present.
        std::fs::write(profiles_dir.join(format!("{}.wav", convert::FALLBACK_HRIR_STEM)), b"").unwrap();
        let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
        let (path, missing) = convert::resolve_hrir_path_or_fallback(&cfg, &base).expect("falls back");
        assert!(path.ends_with(format!("{}.wav", convert::FALLBACK_HRIR_STEM)));
        assert_eq!(missing, Some("04-gsx-sennheiser-gsx".to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_or_fallback_missing_pinned_falls_back_to_any_available() {
        let tmp = unique_cfg_tmp("hrir_or_fb_any");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        // Neither pinned nor bundled present, but another HRIR exists.
        std::fs::write(profiles_dir.join("99-other.wav"), b"").unwrap();
        let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
        let (path, missing) = convert::resolve_hrir_path_or_fallback(&cfg, &base).expect("falls back to any");
        assert!(path.ends_with("99-other.wav"));
        assert_eq!(missing, Some("04-gsx-sennheiser-gsx".to_string()));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_or_fallback_no_hrir_at_all_errors() {
        let tmp = unique_cfg_tmp("hrir_or_fb_none");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        std::fs::create_dir_all(&base).unwrap();
        let cfg = arctis_config::SurroundConfig { hrir: Some("04-gsx-sennheiser-gsx".into()), ..Default::default() };
        let result = convert::resolve_hrir_path_or_fallback(&cfg, &base);
        assert!(matches!(result, Err(EngineError::BadRequest(_))));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn available_hrirs_returns_sorted_stems() {
        let tmp = unique_cfg_tmp("avail_hrirs");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        let profiles_dir = base.join("profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("zz-last.wav"), b"").unwrap();
        std::fs::write(profiles_dir.join("aa-first.wav"), b"").unwrap();
        std::fs::write(profiles_dir.join("mm-middle.wav"), b"").unwrap();

        let hrirs = convert::available_hrirs(&base);
        assert_eq!(hrirs, vec!["aa-first", "mm-middle", "zz-last"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn available_hrirs_empty_when_dir_missing() {
        let tmp = unique_cfg_tmp("avail_hrirs_empty");
        let base = tmp.join(convert::HRIR_BASE_SUBPATH);
        // Intentionally don't create the profiles dir.
        std::fs::create_dir_all(&tmp).unwrap();

        let hrirs = convert::available_hrirs(&base);
        assert!(hrirs.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn reconcile_step6_disabled_calls_source_exists_noop() {
        // Surround disabled → remove() is called → source_exists() fires → 1 ls
        // queue_reconcile_present already adds the surround step6 ls output
        let runner = queue_reconcile_present(MockRunner::new());
        let cfg = make_config_no_eq_no_routes(); // surround disabled by default
        let mut engine = Engine::new(runner, cfg);
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        // The last call should be the surround source_exists check (ls Node, step6)
        let calls = &engine.runner.calls;
        let last = calls.last().expect("at least one call");
        assert_eq!(
            last,
            &vec!["pw-cli", "ls", "Node"],
            "step6 must call source_exists"
        );
        // No surround spawn (disabled → remove path, source absent → no-op)
        assert!(
            !engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("arctis_surround"))
                .unwrap_or(false)),
            "no surround spawn when disabled"
        );
        // Total 60 calls: 2 (detect) + 4 (phase1) + 4*(1+10+1+1) (EQ+vol per channel) + 1 (mic) + 1 (surround)
        assert_eq!(calls.len(), 60, "expected 60 total pw-cli calls");
    }

    /// Remove stale on-disk confs from previous tests/runs so the
    /// diff-before-recreate guards never skip a scripted teardown+respawn.
    fn scrub_stale_confs() {
        let t = std::env::temp_dir();
        for f in [
            "arctis_arctis_surround.conf",
            "arctis_eq.Arctis_Game.conf",
            "arctis_eq.Arctis_Chat.conf",
            "arctis_eq.Arctis_Media.conf",
            "arctis_eq.Arctis_Aux.conf",
        ] {
            let _ = std::fs::remove_file(t.join(f));
        }
    }

    /// Queue outputs for reconcile with surround ENABLED and surround node absent.
    /// Engine::new seeds "aux" → 4 channels (game/chat/media/aux).
    /// Step6 enabled (apply_surround uses recreate_ex in enabled path):
    ///   1. recreate_ex() = remove() + spawn_conf() (no second source_exists):
    ///      remove: source_exists() → 1 ls (absent, no destroy)
    ///      spawn_conf: writes conf + spawns (no runner call)
    ///   2. reroute "game": set_output → ls (source_exists: present) → find_node_id → destroy → pkill + ls (source absent → spawn)
    ///   3. reroute "media": same
    fn queue_reconcile_surround_enabled_absent(runner: MockRunner) -> MockRunner {
        scrub_stale_confs();
        let ls_channels = ls_all_present();
        let ls_surround_absent = ls_all_absent(); // no surround node
        let mut r = runner;
        // detect_headset_sink: pw-metadata 0 + pw-dump (no SteelSeries → detect returns None)
        r = r.with_output(0, "", ""); // pw-metadata 0
        r = r.with_output(0, "[]", ""); // pw-dump []
        // Phase 1: 4 ls (all channels present, including aux seeded by Engine::new)
        for _ in 0..4 {
            r = r.with_output(0, &ls_channels, "");
        }
        // Phase 2 + 2b interleaved: per channel (4), EQ apply then volume/mute apply
        for _ in 0..4 {
            // Phase 2: EQ apply (1 ls + 10 band sets + 1 preamp set)
            r = r.with_output(0, &ls_channels, "");
            for _ in 0..11 {
                r = r.with_output(0, "", "");
            }
            // Phase 2b: volume/mute apply (1 ls + 1 Props set)
            r = r.with_output(0, &ls_channels, ""); // find_node_id
            r = r.with_output(0, "", ""); // Props set
        }
        // Phase 5 (mic disabled): source_exists → 1 ls (no mic)
        r = r.with_output(0, &ls_surround_absent, "");
        // Phase 6 start: apply_surround (mode = Auto) probes the negotiated input
        // layout first → 1 pw-dump (no streams → None → HRIR 7.1).
        r = r.with_output(0, "[]", ""); // apply_surround Auto probe: pw-dump
        // Then it re-detects the headset to default the convolver output sink
        // (hw_sink unset) → pw-metadata 0 + pw-dump (no SteelSeries → None,
        // so the convolver output is left untargeted in this fixture).
        r = r.with_output(0, "", ""); // apply_surround detect: pw-metadata 0
        r = r.with_output(0, "[]", ""); // apply_surround detect: pw-dump []
        // Phase 6 (surround enabled, absent) — recreate_ex():
        //   remove: source_exists() → 1 ls (absent, no destroy needed)
        //   spawn_conf: writes conf + spawns directly (no second runner call)
        r = r.with_output(0, &ls_surround_absent, "");
        // reroute "game": set_output → ChannelManager::set_output →
        //   find existing present node: ls (source_exists present) + find_node_id ls + destroy + pkill
        //   create absent: ls absent → spawn
        r = r.with_output(0, &ls_channels, ""); // set_output: source_exists (game present)
        r = r.with_output(0, &ls_channels, ""); // set_output: find_node_id
        r = r.with_output(0, "", ""); // set_output: destroy
        r = r.with_output(0, "", ""); // set_output: pkill
        r = r.with_output(0, &ls_surround_absent, ""); // set_output: create source_exists (absent → spawn)
                                                       // reroute "media": same
        r = r.with_output(0, &ls_channels, "");
        r = r.with_output(0, &ls_channels, "");
        r = r.with_output(0, "", "");
        r = r.with_output(0, "", "");
        r = r.with_output(0, &ls_surround_absent, "");
        r
    }

    #[test]
    fn reconcile_step6_enabled_spawns_surround_and_reroutes_channels() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_step6_enabled");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("test-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        let runner = queue_reconcile_surround_enabled_absent(MockRunner::new());
        let cfg = make_config_surround_enabled("test-hrir");
        let mut engine = Engine::new(runner, cfg);
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        // Surround sink was spawned
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("arctis_surround"))
                .unwrap_or(false)),
            "surround sink must be spawned: {:?}",
            engine.runner.spawned
        );
        // Channel reroutes spawned new instances for game + media
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("Arctis_Game"))
                .unwrap_or(false)),
            "game channel must be respawned for surround routing"
        );
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.contains("Arctis_Media"))
                .unwrap_or(false)),
            "media channel must be respawned for surround routing"
        );
        // Children tracked: surround + game + media
        assert!(engine.children.len() >= 3, "at least 3 children tracked");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    // ─────────────────────────────────────────────
    // F1.3 TDD: surround_set_* methods
    // ─────────────────────────────────────────────

    #[test]
    fn surround_set_enabled_true_without_hrir_returns_err() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_on_no_hrir");
        // Don't create any HRIR file → resolve_hrir_path must fail
        std::fs::create_dir_all(&tmp).unwrap();
        std::env::set_var("HOME", &tmp);

        let cfg = make_config_no_eq_no_routes(); // surround.enabled = false
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let result = engine.surround_set_enabled(true);
        assert!(
            matches!(result, Err(EngineError::BadRequest(_))),
            "must error when no HRIR exists: {result:?}"
        );
        // Config must NOT have been mutated (error before mutation)
        assert!(
            !engine.config().active().unwrap().surround.enabled,
            "surround.enabled must remain false on error"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    #[test]
    fn surround_set_hrir_persists_and_emits_event() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp_home = unique_cfg_tmp("surr_set_hrir_home");
        let tmp_cfg = unique_cfg_tmp("surr_set_hrir_cfg");
        let profiles_dir = tmp_home.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("my-hrir.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp_home);
        std::env::set_var("ASM_CONFIG_HOME", &tmp_cfg);

        // surround disabled → surround_set_hrir just persists, no recreate
        let cfg = make_config_no_eq_no_routes(); // surround disabled
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        engine.set_event_sink(tx);

        engine
            .surround_set_hrir("my-hrir".into())
            .expect("surround_set_hrir should succeed");

        // Config persisted
        let saved_path = tmp_cfg.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("my-hrir"),
            "persisted config must contain hrir stem"
        );

        // In-memory updated
        assert_eq!(
            engine.config().active().unwrap().surround.hrir.as_deref(),
            Some("my-hrir")
        );

        // Event emitted
        let event = rx.try_recv().expect("SurroundHrirSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::SurroundHrirSet {
                hrir: Some("my-hrir".into())
            }
        );

        let _ = std::fs::remove_dir_all(&tmp_home);
        let _ = std::fs::remove_dir_all(&tmp_cfg);
        std::env::remove_var("HOME");
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn surround_set_hrir_nonexistent_returns_err() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_hrir_err");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        // Don't write the file.
        std::env::set_var("HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let result = engine.surround_set_hrir("nonexistent".into());
        assert!(matches!(result, Err(EngineError::BadRequest(_))));
        // hrir must remain None (rollback)
        assert!(engine.config().active().unwrap().surround.hrir.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    #[test]
    fn surround_set_channels_persists_and_emits_event() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_ch");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        engine.set_event_sink(tx);

        engine
            .surround_set_channels(vec!["game".into(), "chat".into(), "media".into()])
            .expect("surround_set_channels should succeed");

        // Config persisted
        assert!(tmp.join("config.toml").exists());

        // Event emitted
        let event = rx
            .try_recv()
            .expect("SurroundChannelsSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::SurroundChannelsSet {
                channels: vec!["game".into(), "chat".into(), "media".into()]
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn surround_set_blocksize_rejects_zero() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_bs_zero");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let before = engine.config.active().unwrap().surround.blocksize;

        let res = engine.surround_set_blocksize(Some(0));
        assert!(matches!(res, Err(crate::error::EngineError::BadRequest(_))));
        assert_eq!(
            engine.config.active().unwrap().surround.blocksize,
            before,
            "config must be unchanged when blocksize is rejected"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn surround_set_blocksize_persists() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_bs");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        engine.surround_set_blocksize(Some(128)).unwrap();
        assert_eq!(engine.config.active().unwrap().surround.blocksize, Some(128));
        engine.surround_set_blocksize(None).unwrap();
        assert_eq!(engine.config.active().unwrap().surround.blocksize, None);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn surround_set_blocksize_rejects_non_power_of_two_and_out_of_range() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_bs_invalid");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        for bad in [100u32, 32, 16384, 65] {
            let res = engine.surround_set_blocksize(Some(bad));
            assert!(
                matches!(res, Err(crate::error::EngineError::BadRequest(_))),
                "blocksize {bad} must be rejected (power of two in 64..=8192)"
            );
        }
        // Valid values pass (64 and 8192 are the range edges).
        engine.surround_set_blocksize(Some(64)).unwrap();
        engine.surround_set_blocksize(Some(8192)).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn surround_set_tailsize_validates_and_persists() {
        scrub_stale_confs();
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_set_ts");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        // Same shape validation as blocksize.
        assert!(matches!(
            engine.surround_set_tailsize(Some(100)),
            Err(crate::error::EngineError::BadRequest(_))
        ));
        // tailsize must be >= a pinned blocksize.
        engine.surround_set_blocksize(Some(256)).unwrap();
        assert!(
            matches!(
                engine.surround_set_tailsize(Some(128)),
                Err(crate::error::EngineError::BadRequest(_))
            ),
            "tailsize below the pinned blocksize must be rejected"
        );
        engine.surround_set_tailsize(Some(4096)).unwrap();
        assert_eq!(engine.config.active().unwrap().surround.tailsize, Some(4096));
        // And blocksize may not be raised past the pinned tailsize.
        assert!(matches!(
            engine.surround_set_blocksize(Some(8192)),
            Err(crate::error::EngineError::BadRequest(_))
        ));
        // Clearing works.
        engine.surround_set_tailsize(None).unwrap();
        assert_eq!(engine.config.active().unwrap().surround.tailsize, None);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn state_surround_snapshot_reflects_config() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("surr_state");
        let profiles_dir = tmp.join(".local/share/pipewire/hrir_hesuvi/profiles");
        std::fs::create_dir_all(&profiles_dir).unwrap();
        std::fs::write(profiles_dir.join("aa-first.wav"), b"").unwrap();
        std::fs::write(profiles_dir.join("zz-last.wav"), b"").unwrap();
        std::env::set_var("HOME", &tmp);

        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround = arctis_config::SurroundConfig {
            enabled: false,
            hrir: Some("aa-first".into()),
            channels: vec!["game".into()],
            hw_sink: Some("alsa_output.pci".into()),
            ..Default::default()
        };
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let s = engine.state();

        assert!(!s.surround.enabled);
        assert_eq!(s.surround.hrir.as_deref(), Some("aa-first"));
        assert_eq!(s.surround.channels, vec!["game"]);
        assert_eq!(s.surround.hw_sink.as_deref(), Some("alsa_output.pci"));
        assert_eq!(s.surround.available_hrirs, vec!["aa-first", "zz-last"]);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
    }

    #[test]
    fn surround_snapshot_includes_display_entries_for_available_hrirs() {
        let base = unique_cfg_tmp("hrir_entries");
        let profiles = base.join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        std::fs::write(profiles.join("04-gsx-sennheiser-gsx.wav"), b"RIFF").unwrap();
        let entries = crate::engine::hrir_entries_for(&base);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].stem, "04-gsx-sennheiser-gsx");
        assert_eq!(entries[0].display, "Sennheiser GSX");
        assert_eq!(entries[0].group, "Sennheiser");
        let _ = std::fs::remove_dir_all(&base);
    }
