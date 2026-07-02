    use super::*;
    use crate::engine::test_support::*;

    #[test]
    fn set_route_persists() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("route");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);
        // Also set HOME so WirePlumber fragment goes somewhere safe
        let tmp_home = unique_cfg_tmp("route_home");
        std::env::set_var("HOME", &tmp_home);

        let cfg = make_config_no_eq_no_routes();

        // set_route: Router::save_persistent (disk only, no runner calls for that)
        //            Router::apply_live (pw-dump + pw-metadata) — but app likely absent,
        //            so we queue pw-dump returning empty JSON array (apply_live will error
        //            internally but that's best-effort and ignored).
        let runner = MockRunner::new().with_output(0, "[]", ""); // pw-dump for apply_live (app not running → error ignored)

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        engine
            .set_route("firefox", "Arctis_Media")
            .expect("set_route should succeed");

        // Unified config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("firefox"),
            "persisted config must contain firefox route"
        );

        // MockRunner shows pw-metadata was attempted (pw-dump ran at minimum)
        assert!(
            engine.runner.calls.iter().any(|c| c[0] == "pw-dump"),
            "pw-dump must be called for live move attempt"
        );

        // Event received
        let event = rx.try_recv().expect("RouteSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::RouteSet {
                app_binary: "firefox".to_string(),
                target_sink: "Arctis_Media".to_string(),
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&tmp_home);
        std::env::remove_var("ASM_CONFIG_HOME");
        std::env::remove_var("HOME");
    }

    /// Regression (route-clobber): persist_route used to build an EMPTY Router,
    /// apply one set_rule and save — wiping every sibling rule from routes.json
    /// and the persistent fragments on each route change. The projection must
    /// carry ALL of profile.routes (G4: profile is the single source of truth).
    #[test]
    fn set_route_preserves_sibling_persisted_rules() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("route_sib");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);
        let tmp_home = unique_cfg_tmp("route_sib_home");
        std::env::set_var("HOME", &tmp_home);

        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].routes.push(arctis_config::RouteConfig {
            app_binary: "discord".into(),
            target_sink: "Arctis_Chat".into(),
        });
        // apply_live best-effort: pw-dump returns empty array → live move skipped.
        let runner = MockRunner::new().with_output(0, "[]", "");
        let mut engine = Engine::new(runner, cfg);
        engine.set_route("firefox", "Arctis_Media").unwrap();

        let routes_json = std::fs::read_to_string(
            tmp_home.join(".config/arctis-sound-manager/routes.json"),
        )
        .expect("routes.json written");
        assert!(routes_json.contains("firefox"), "new rule present: {routes_json}");
        assert!(routes_json.contains("discord"), "sibling rule survives: {routes_json}");

        let frag = std::fs::read_to_string(
            tmp_home.join(".config/pipewire/client.conf.d/90-asm-routing.conf"),
        )
        .expect("client fragment written");
        assert!(frag.contains("firefox") && frag.contains("discord"), "fragment: {frag}");

        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&tmp_home);
        std::env::remove_var("ASM_CONFIG_HOME");
        std::env::remove_var("HOME");
    }

    /// Regression (route-clobber): clear_route used to save an EMPTY rule set
    /// even when other routes existed. Clearing one app must re-project the
    /// remaining profile routes.
    #[test]
    fn clear_route_preserves_sibling_persisted_rules() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("route_clear_sib");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);
        let tmp_home = unique_cfg_tmp("route_clear_sib_home");
        std::env::set_var("HOME", &tmp_home);

        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].routes = vec![
            arctis_config::RouteConfig {
                app_binary: "discord".into(),
                target_sink: "Arctis_Chat".into(),
            },
            arctis_config::RouteConfig {
                app_binary: "firefox".into(),
                target_sink: "Arctis_Media".into(),
            },
        ];
        // clear_live best-effort: pw-dump returns empty array → skipped.
        let runner = MockRunner::new().with_output(0, "[]", "");
        let mut engine = Engine::new(runner, cfg);
        engine.clear_route("firefox").unwrap();

        let routes_json = std::fs::read_to_string(
            tmp_home.join(".config/arctis-sound-manager/routes.json"),
        )
        .expect("routes.json written");
        assert!(!routes_json.contains("firefox"), "cleared rule gone: {routes_json}");
        assert!(routes_json.contains("discord"), "sibling rule survives: {routes_json}");

        let frag = std::fs::read_to_string(
            tmp_home.join(".config/pipewire/client.conf.d/90-asm-routing.conf"),
        )
        .expect("client fragment written");
        assert!(!frag.contains("firefox") && frag.contains("discord"), "fragment: {frag}");

        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&tmp_home);
        std::env::remove_var("ASM_CONFIG_HOME");
        std::env::remove_var("HOME");
    }

    #[test]
    fn list_streams_maps_sink_to_channel_and_marks_routed() {
        // Active profile: game/chat/media (node_name Arctis_Game/Chat/Media).
        let mut cfg = make_config_no_eq_no_routes();
        // Add a persistent route so `routed` flips for firefox.
        cfg.profiles[0].routes = vec![arctis_config::RouteConfig {
            app_binary: "firefox".into(),
            target_sink: "Arctis_Game".into(),
        }];
        let dump = include_str!("../../../../audio/tests/fixtures/pw_dump_app_streams.json");
        let runner = arctis_audio::MockRunner::new().with_output(0, dump, ""); // pw-dump
        let mut engine = Engine::new(runner, cfg);
        let streams = engine.list_streams().unwrap();

        let ff = streams.iter().find(|s| s.binary == "firefox").unwrap();
        assert_eq!(ff.current_channel.as_deref(), Some("game")); // Arctis_Game → game
        assert!(ff.routed, "firefox has a persistent rule");

        let sp = streams.iter().find(|s| s.binary == "spotify").unwrap();
        assert_eq!(sp.current_channel, None, "unlinked spotify is unrouted");
        assert!(!sp.routed);
    }

    #[test]
    fn list_streams_dedupes_multiple_nodes_of_one_app_into_one_badge() {
        // A browser (Vivaldi) can hold several Stream/Output/Audio nodes at once
        // (e.g. a second node appears when a video starts). The mixer must show
        // ONE badge per app, not one per node: two vivaldi-bin nodes → one entry.
        let dump = r#"[
          {"id":70,"type":"PipeWire:Interface:Node","info":{"props":{
            "media.class":"Stream/Output/Audio","application.name":"Vivaldi",
            "application.process.binary":"vivaldi-bin","application.process.id":"100","media.name":"Playback"}}},
          {"id":71,"type":"PipeWire:Interface:Node","info":{"props":{
            "media.class":"Stream/Output/Audio","application.name":"Vivaldi",
            "application.process.binary":"vivaldi-bin","application.process.id":"100","media.name":"AudioStream"}}}
        ]"#;
        let runner = arctis_audio::MockRunner::new().with_output(0, dump, ""); // pw-dump
        let mut engine = Engine::new(runner, make_config_no_eq_no_routes());
        let streams = engine.list_streams().unwrap();
        let vivaldi: Vec<_> = streams.iter().filter(|s| s.binary == "vivaldi-bin").collect();
        assert_eq!(
            vivaldi.len(),
            1,
            "multiple nodes of one app must collapse into a single badge: {streams:?}"
        );
    }

    #[test]
    fn list_output_devices_returns_real_sinks_and_marks_default() {
        let dump = include_str!("../../../../audio/tests/fixtures/pw_dump_sinks.json");
        // Runner queue: [0] pw-metadata 0, [1] pw-dump
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, PW_METADATA_SINK, "") // pw-metadata 0
            .with_output(0, dump, ""); // pw-dump
        let mut engine = Engine::new(runner, make_config_no_eq_no_routes());
        let devices = engine.list_output_devices();

        // Headset sink present
        assert!(
            devices.iter().any(|d| d.node_name.contains("SteelSeries_Arctis")),
            "headset sink missing: {devices:?}"
        );
        // Virtual sinks excluded
        assert!(
            !devices.iter().any(|d| d.node_name.starts_with("Arctis_")),
            "virtual sinks must be excluded: {devices:?}"
        );
        // Onboard marked default
        let onboard = devices
            .iter()
            .find(|d| d.node_name.contains("analog-stereo"))
            .expect("onboard sink missing");
        assert!(onboard.is_default, "onboard must be is_default=true");
    }

    #[test]
    fn list_output_devices_returns_empty_on_pw_dump_error() {
        // Queue [0] pw-metadata (ok), [1] pw-dump with non-zero exit
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, PW_METADATA_SINK, "") // pw-metadata 0
            .with_output(1, "", "pw-dump: error"); // pw-dump fails
        let mut engine = Engine::new(runner, make_config_no_eq_no_routes());
        let devices = engine.list_output_devices();
        assert!(
            devices.is_empty(),
            "must return empty Vec on pw-dump failure, got: {devices:?}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Task 5 TDD: detect_headset_sink + overlay_default_output wired into reconcile
    // ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn detect_headset_sink_returns_steelseries_sink() {
        // Queue: [0] pw-metadata 0, [1] pw-dump (contains SteelSeries sink)
        let dump = include_str!("../../../../audio/tests/fixtures/pw_dump_sinks.json");
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, PW_METADATA_SINK, "") // pw-metadata 0
            .with_output(0, dump, "");             // pw-dump
        let mut engine = Engine::new(runner, make_config_no_eq_no_routes());
        let result = engine.detect_headset_sink();
        assert_eq!(
            result.as_deref(),
            Some("alsa_output.usb-SteelSeries_Arctis_Nova_Pro_Wireless-00.analog-stereo"),
            "must return the SteelSeries hardware sink node_name"
        );
    }

    #[test]
    fn detect_headset_sink_returns_none_when_no_steelseries_sink() {
        // pw-dump with only the onboard sink (no SteelSeries/Arctis hardware sink)
        let dump_no_headset = r#"[
          { "id": 11, "type": "PipeWire:Interface:Node",
            "info": { "props": {
              "media.class": "Audio/Sink",
              "node.name": "alsa_output.pci-0000_00_1f.3.analog-stereo",
              "node.description": "Speakers" } } }
        ]"#;
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, PW_METADATA_SINK, "") // pw-metadata 0
            .with_output(0, dump_no_headset, ""); // pw-dump (no SteelSeries)
        let mut engine = Engine::new(runner, make_config_no_eq_no_routes());
        let result = engine.detect_headset_sink();
        assert!(
            result.is_none(),
            "must return None when no SteelSeries/Arctis hardware sink present"
        );
    }

    // ─────────────────────────────────────────────
    // Task 3 TDD: move_stream (live move + persist)
    // ─────────────────────────────────────────────

    /// Exact MockRunner call sequence for move_stream("70", "chat"):
    ///   [0] pw-dump          — list_streams
    ///   [1] pw-metadata      — explicit id-move (target.object on node 70)
    ///
    /// move_stream now calls persist_route (not set_route), so there is NO third
    /// pw-dump / apply_live call. Outputs needed: [0]=dump fixture, [1]="".
    #[test]
    fn move_stream_by_id_persists_rule_and_moves_live() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("move_stream");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);
        let tmp_home = unique_cfg_tmp("move_stream_home");
        std::env::set_var("HOME", &tmp_home);

        let cfg = make_config_no_eq_no_routes();
        let dump = include_str!("../../../../audio/tests/fixtures/pw_dump_app_streams.json");
        // Exact 2-call sequence: (1) pw-dump for list_streams, (2) pw-metadata for the id move.
        // persist_route (called after the live move) writes config + WP fragment only — no runner calls.
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, dump, "")
            .with_output(0, "", "");
        let mut engine = Engine::new(runner, cfg);
        engine.move_stream("70", "chat").unwrap(); // firefox node id 70 → chat

        // Persistent route recorded in the in-memory active profile.
        let active = engine.config().active().unwrap();
        assert!(
            active
                .routes
                .iter()
                .any(|r| r.app_binary == "firefox" && r.target_sink == "Arctis_Chat"),
            "expected persisted firefox->Arctis_Chat route: {:?}",
            active.routes
        );

        // pw-metadata call was issued for the live move.
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c.first().map(|s| s.as_str()) == Some("pw-metadata")),
            "pw-metadata must be called for live move"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&tmp_home);
        std::env::remove_var("ASM_CONFIG_HOME");
        std::env::remove_var("HOME");
    }

    #[test]
    fn move_stream_moves_every_node_of_a_multi_node_app() {
        // Routing an app must move ALL its current output nodes, not just the first.
        // A browser with a video playing has 2+ nodes; moving only one leaves the
        // other on the default sink, so the app appears to "jump back".
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("move_stream_all");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);
        let tmp_home = unique_cfg_tmp("move_stream_all_home");
        std::env::set_var("HOME", &tmp_home);

        let cfg = make_config_no_eq_no_routes();
        let dump = r#"[
          {"id":70,"type":"PipeWire:Interface:Node","info":{"props":{
            "media.class":"Stream/Output/Audio","application.name":"Vivaldi",
            "application.process.binary":"vivaldi-bin","application.process.id":"100","media.name":"Playback"}}},
          {"id":71,"type":"PipeWire:Interface:Node","info":{"props":{
            "media.class":"Stream/Output/Audio","application.name":"Vivaldi",
            "application.process.binary":"vivaldi-bin","application.process.id":"100","media.name":"AudioStream"}}}
        ]"#;
        // Only the pw-dump output is queued; the pw-metadata moves return default 0.
        let runner = arctis_audio::MockRunner::new().with_output(0, dump, "");
        let mut engine = Engine::new(runner, cfg);
        engine.move_stream("vivaldi-bin", "media").unwrap();

        let moved: std::collections::HashSet<String> = engine
            .runner
            .calls
            .iter()
            .filter(|c| c.first().map(|s| s.as_str()) == Some("pw-metadata"))
            .filter_map(|c| {
                c.iter()
                    .find(|a| a.as_str() == "70" || a.as_str() == "71")
                    .cloned()
            })
            .collect();
        assert!(
            moved.contains("70") && moved.contains("71"),
            "both vivaldi nodes must be live-moved; pw-metadata calls: {:?}",
            engine.runner.calls
        );

        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&tmp_home);
        std::env::remove_var("ASM_CONFIG_HOME");
        std::env::remove_var("HOME");
    }

    #[test]
    fn move_stream_unknown_channel_errors() {
        let cfg = make_config_no_eq_no_routes();
        let dump = include_str!("../../../../audio/tests/fixtures/pw_dump_app_streams.json");
        let runner = arctis_audio::MockRunner::new().with_output(0, dump, "");
        let mut engine = Engine::new(runner, cfg);
        let result = engine.move_stream("70", "nope");
        assert!(
            result.is_err(),
            "unknown channel_id must return an error"
        );
        // Verify it's a BadRequest (channel check fires before any runner call).
        assert!(
            matches!(result, Err(EngineError::BadRequest(_))),
            "must be BadRequest, got: {result:?}"
        );
        // No runner calls consumed (channel error fires before list_streams).
        assert!(
            engine.runner.calls.is_empty(),
            "no runner calls on unknown channel"
        );
    }

    // ── fix/routing: set_default_sink_channel uses pw-metadata, not wpctl set-default ──

    /// Enabling the default-output channel must call pw-metadata with a name-based JSON
    /// value, NOT wpctl set-default (which requires a numeric id and errors with
    /// "is not a valid number").
    #[test]
    fn set_default_sink_channel_calls_pw_metadata_not_wpctl() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("def_sink_pwmeta");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        // Only one subprocess call: pw-metadata (no pw-cli ls needed — name is resolved
        // directly from the in-memory config, no node-id lookup required).
        let runner = MockRunner::new().with_output(0, "", ""); // pw-metadata success
        let mut engine = Engine::new(runner, cfg);
        engine
            .set_default_sink_channel(Some("game".into()))
            .expect("set_default_sink_channel must succeed");

        let calls = &engine.runner.calls;
        // There must be exactly one subprocess call.
        assert_eq!(calls.len(), 1, "expected 1 subprocess call, got: {calls:?}");

        // It must be pw-metadata, NOT wpctl.
        assert_ne!(
            calls[0].first().map(|s| s.as_str()),
            Some("wpctl"),
            "must NOT call wpctl (set-default requires numeric id), calls: {calls:?}"
        );
        assert_eq!(
            calls[0].first().map(|s| s.as_str()),
            Some("pw-metadata"),
            "must call pw-metadata, calls: {calls:?}"
        );

        // Full argv must match the name-based form.
        assert_eq!(
            calls[0],
            vec![
                "pw-metadata",
                "-n",
                "default",
                "0",
                "default.configured.audio.sink",
                "{\"name\":\"Arctis_Game\"}",
            ],
            "pw-metadata argv mismatch"
        );

        // Config must be persisted with the chosen channel.
        let saved = std::fs::read_to_string(tmp.join("config.toml"))
            .expect("config.toml must exist after set_default_sink_channel");
        assert!(
            saved.contains("default_sink_channel"),
            "config.toml must contain default_sink_channel, got:\n{saved}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Clearing the default-output channel (None) must NOT call any subprocess — it only
    /// persists the cleared preference and emits the event.
    #[test]
    fn set_default_sink_channel_none_no_subprocess() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("def_sink_none");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let runner = MockRunner::new(); // no outputs queued — any call would panic/fail
        let mut engine = Engine::new(runner, cfg);
        engine
            .set_default_sink_channel(None)
            .expect("clearing default sink channel must succeed");

        assert!(
            engine.runner.calls.is_empty(),
            "clearing default must NOT call any subprocess, calls: {:?}",
            engine.runner.calls
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Providing an unknown channel id must return BadRequest without any subprocess call.
    #[test]
    fn set_default_sink_channel_unknown_channel_bad_request() {
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let err = engine
            .set_default_sink_channel(Some("nonexistent".into()))
            .expect_err("unknown channel must error");
        assert!(
            matches!(err, EngineError::BadRequest(_)),
            "unknown channel must be BadRequest, got: {err:?}"
        );
        assert!(
            engine.runner.calls.is_empty(),
            "no subprocess call must be made for a bad channel id, calls: {:?}",
            engine.runner.calls
        );
    }
