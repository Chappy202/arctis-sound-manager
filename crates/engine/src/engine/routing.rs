//! Per-app routing, stream discovery/moves, output sinks, default-sink control.
use super::*;

impl<R: CommandRunner> Engine<R> {
    /// Project the active profile's routes — the single source of truth (G4) —
    /// into `routes.json` + the persistent conf fragments via Router. Building
    /// the Router from the FULL route set is what keeps sibling rules intact:
    /// constructing it empty and applying one rule used to clobber every other
    /// persisted route on each GUI route change.
    fn project_routes(&mut self) -> Result<(), EngineError> {
        let rules = convert::route_rules_from_profile(self.config.active()?);
        let router = Router::with_rules(&mut self.runner, rules);
        router.save_persistent()?;
        Ok(())
    }

    /// Persist a route rule without doing a live move.
    ///
    /// Upserts the route in the active profile's in-memory config, writes the unified
    /// config to disk, and re-projects ALL profile routes into the persistent
    /// fragments via Router. Emits `RouteSet`. Does NOT call `apply_live`.
    fn persist_route(&mut self, app_binary: &str, target_sink: &str) -> Result<(), EngineError> {
        // Update in-memory config
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            if let Some(existing) = profile
                .routes
                .iter_mut()
                .find(|r| r.app_binary == app_binary)
            {
                existing.target_sink = target_sink.to_string();
            } else {
                profile.routes.push(arctis_config::RouteConfig {
                    app_binary: app_binary.to_string(),
                    target_sink: target_sink.to_string(),
                });
            }
        }
        // Persist unified config
        self.save_config()?;
        // Re-project the FULL route set (no live move)
        self.project_routes()?;
        // Emit event
        self.emit(Event::RouteSet {
            app_binary: app_binary.to_string(),
            target_sink: target_sink.to_string(),
        });
        Ok(())
    }

    /// Add/upsert a route in the active profile, persist, set_rule + save_persistent + apply_live.
    pub fn set_route(&mut self, app_binary: &str, target_sink: &str) -> Result<(), EngineError> {
        self.persist_route(app_binary, target_sink)?;
        // Best-effort live move by binary (ignore error if app not running)
        {
            let mut router = Router::new(&mut self.runner);
            let _ = router.apply_live(&AppMatch::Binary(app_binary.to_string()), target_sink);
        }
        Ok(())
    }

    /// Remove the routing rule for `app_binary`.
    ///
    /// Drops the rule from in-memory config + persists, re-projects the remaining
    /// routes into the persistent fragments (siblings survive), and attempts a
    /// best-effort live clear (moves the stream back to the default sink by
    /// deleting its `target.object` metadata key).
    ///
    /// KNOWN LIMITATION: WirePlumber's restore-stream module
    /// (`node.stream.restore-target`, default true) remembers the app's last
    /// manual target in `~/.local/state/wireplumber/restore-stream`; there is no
    /// supported way to clear a single app's entry without restarting
    /// WirePlumber, so the app's NEXT stream may reappear on the old sink until
    /// the user moves it once. See KNOWN_ISSUES.md (KI-6).
    pub fn clear_route(&mut self, app_binary: &str) -> Result<(), EngineError> {
        // Update in-memory config
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            profile.routes.retain(|r| r.app_binary != app_binary);
        }
        // Persist unified config
        self.save_config()?;
        // Re-project the REMAINING routes (clear_route used to write an empty
        // rule set here, wiping every other persisted route).
        self.project_routes()?;
        // Best-effort live clear (ignore error if app not running)
        {
            let mut router = Router::new(&mut self.runner);
            let _ = router.clear_live(&AppMatch::Binary(app_binary.to_string()));
        }
        // Emit event
        self.emit(Event::RouteCleared {
            app_binary: app_binary.to_string(),
        });
        Ok(())
    }

    /// Per-node application output streams (one entry per PipeWire output node),
    /// each resolved to a channel id (via its linked sink node.name) and flagged
    /// with a persistent route. One `pw-dump` per call; pure mapping otherwise.
    /// Read-only. This is the pre-dedup view used by routing, which must act on
    /// EVERY node of an app — `list_streams` collapses it to one badge per app.
    fn list_app_streams_raw(&mut self) -> Result<Vec<crate::state::AppStream>, EngineError> {
        // node.name -> channel id, built from the active profile (never hard-coded).
        let (name_to_id, routed_bins): (
            std::collections::HashMap<String, String>,
            std::collections::HashSet<String>,
        ) = {
            let profile = self.config.active()?;
            let map = profile
                .channels
                .iter()
                .map(|c| (c.node_name.clone(), c.id.clone()))
                .collect();
            let routed = profile
                .routes
                .iter()
                .map(|r| r.app_binary.clone())
                .collect();
            (map, routed)
        };

        let out = self.runner.run("pw-dump", &[])?;
        if out.status != 0 {
            return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                program: "pw-dump".into(),
                status: out.status,
                stderr: out.stderr,
            }));
        }
        let parsed = arctis_audio::parse_app_streams(&out.stdout)?;
        let mut per_node: Vec<crate::state::AppStream> = parsed
            .into_iter()
            .map(|p| crate::state::AppStream {
                current_channel: p
                    .sink_node_name
                    .as_ref()
                    .and_then(|n| name_to_id.get(n).cloned()),
                routed: routed_bins.contains(&p.binary),
                id: p.id,
                binary: p.binary,
                app_name: p.app_name,
                pid: p.pid,
                icon_name: p.icon_name,
                media_name: p.media_name,
            })
            .collect();
        // Stable order by node id so the lowest-id node is the dedup representative.
        per_node.sort_by_key(|s| s.id);
        Ok(per_node)
    }

    /// Discover running application output streams for the mixer: ONE badge per
    /// app. Read-only (no graph mutation).
    pub fn list_streams(&mut self) -> Result<Vec<crate::state::AppStream>, EngineError> {
        let per_node = self.list_app_streams_raw()?;
        // Collapse multiple output nodes of the same app into ONE badge. A browser
        // (Vivaldi/Chrome) can hold several Stream/Output/Audio nodes at once — e.g.
        // a second node appears when a video starts — and one-per-node would show
        // duplicate badges. Keep the lowest-id node (per_node is id-sorted) as the
        // representative; if a sibling is linked to a channel, adopt that channel so
        // a routed stream isn't hidden behind an unlinked one. (Routing is
        // per-binary, so `routed` is identical across a group.)
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut deduped: Vec<crate::state::AppStream> = Vec::with_capacity(per_node.len());
        for s in per_node {
            if let Some(&idx) = seen.get(&s.binary) {
                if deduped[idx].current_channel.is_none() && s.current_channel.is_some() {
                    deduped[idx].current_channel = s.current_channel;
                }
                continue;
            }
            seen.insert(s.binary.clone(), deduped.len());
            deduped.push(s);
        }
        Ok(deduped)
    }

    /// Discover real output sinks (physical devices) via `pw-metadata 0` + `pw-dump`.
    ///
    /// Best-effort: on any subprocess failure an empty `Vec` is returned.
    /// Never panics; never returns `Err`. Shared by the UI output selector and
    /// the headset-sink detection helpers below (G1).
    fn list_output_sinks(&mut self) -> Vec<arctis_audio::OutputSink> {
        // Step 1: get default sink name from pw-metadata 0.
        let default_sink = match self.runner.run("pw-metadata", &["0"]) {
            Ok(out) if out.status == 0 => arctis_audio::parse_default_sink_name(&out.stdout),
            _ => None,
        };

        // Step 2: run pw-dump to enumerate all nodes.
        let out = match self.runner.run("pw-dump", &[]) {
            Ok(o) => o,
            Err(_) => return Vec::new(),
        };
        if out.status != 0 {
            return Vec::new();
        }

        // Step 3: parse.
        arctis_audio::parse_output_sinks(&out.stdout, default_sink.as_deref())
            .unwrap_or_default()
    }

    /// Discover real output devices (physical sinks) for the UI output selector.
    pub fn list_output_devices(&mut self) -> Vec<crate::state::OutputDeviceSnapshot> {
        self.list_output_sinks()
            .into_iter()
            .map(|s| crate::state::OutputDeviceSnapshot {
                node_name: s.node_name,
                description: s.description,
                is_default: s.is_default,
            })
            .collect()
    }

    /// True when the sink looks like the headset hardware sink.
    fn is_headset_sink(node_name: &str) -> bool {
        let lower = node_name.to_lowercase();
        // Virtual channel sinks are "Arctis_<Channel>" — parse_output_sinks
        // already excludes them, so a plain "arctis" match is safe here.
        lower.contains("steelseries") || lower.contains("arctis")
    }

    /// Detect the headset hardware sink by scanning the real output sinks for a
    /// node_name that contains "steelseries" or "arctis" (case-insensitive).
    /// Returns `None` if no hardware headset sink is found or on any subprocess
    /// failure. Never panics.
    pub fn detect_headset_sink(&mut self) -> Option<String> {
        self.list_output_sinks()
            .into_iter()
            .find(|d| Self::is_headset_sink(&d.node_name))
            .map(|d| d.node_name)
    }

    /// Detect the headset hardware sink's PipeWire object id (for `wpctl <id>`,
    /// which does not accept node names). Same discovery as `detect_headset_sink`.
    pub(super) fn detect_headset_sink_id(&mut self) -> Option<u32> {
        self.list_output_sinks()
            .into_iter()
            .find(|d| Self::is_headset_sink(&d.node_name))
            .map(|d| d.id)
    }

    /// Route a running stream to a channel: resolve channel id → sink node.name,
    /// live-move the specific stream (by node id) via pw-metadata, and persist a
    /// binary→sink rule so it sticks next launch. `stream` may be a node id or a
    /// binary; the binary is resolved from discovery for persistence.
    pub fn move_stream(&mut self, stream: &str, channel_id: &str) -> Result<(), EngineError> {
        // Resolve channel -> sink node.name from the active profile.
        let sink = {
            let profile = self.config.active()?;
            profile
                .channels
                .iter()
                .find(|c| c.id == channel_id)
                .map(|c| c.node_name.clone())
                .ok_or_else(|| EngineError::BadRequest(format!("unknown channel: {channel_id}")))?
        };

        // Find the requested stream (by node id string or by binary) to resolve
        // which APP (binary) is being routed. Use the per-node view, not the
        // deduped badge list, so we can move every node of that app below.
        let streams = self.list_app_streams_raw()?;
        let target = streams
            .iter()
            .find(|s| s.id.to_string() == stream || s.binary == stream)
            .ok_or_else(|| EngineError::BadRequest(format!("no running stream: {stream}")))?
            .clone();

        // Live-move EVERY currently-running output node of this app, not just the
        // matched one. Browsers (Vivaldi/Chrome) hold multiple output nodes; moving
        // a single node leaves the others on the old sink, which is the "jumps back"
        // symptom the user sees. Routing is per-binary, so move all same-binary nodes.
        let node_ids: Vec<u32> = streams
            .iter()
            .filter(|s| s.binary == target.binary)
            .map(|s| s.id)
            .collect();
        for id in node_ids {
            let argv = move_stream_argv(&id.to_string(), &sink)?;
            let args: Vec<&str> = argv.iter().map(String::as_str).collect();
            let out = self.runner.run("pw-metadata", &args)?;
            if out.status != 0 {
                return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                    program: "pw-metadata".into(),
                    status: out.status,
                    stderr: out.stderr,
                }));
            }
        }

        // Persist binary -> sink without a second live move.
        // The per-node moves above already covered every running instance; calling
        // set_route would also trigger a best-effort binary-match live move (an
        // extra pw-dump + pw-metadata). persist_route writes config + WP fragment +
        // emits RouteSet without any additional pw-dump / pw-metadata calls.
        self.persist_route(&target.binary, &sink)?;
        Ok(())
    }

    /// Set (or clear) which channel's sink is the system default output. When set,
    /// uses `pw-metadata -n default 0 default.configured.audio.sink {"name":"<node.name>"}`
    /// to set the default by name (robust across restarts; `wpctl set-default` requires a
    /// numeric node-id which is ephemeral and would error with "is not a valid number").
    /// Persists + emits.
    pub fn set_default_sink_channel(
        &mut self,
        channel: Option<String>,
    ) -> Result<(), EngineError> {
        // Validate + resolve sink before mutating.
        let sink = match &channel {
            Some(id) => {
                let p = self.config.active()?;
                Some(
                    p.channels
                        .iter()
                        .find(|c| &c.id == id)
                        .map(|c| c.node_name.clone())
                        .ok_or_else(|| {
                            EngineError::BadRequest(format!("unknown channel: {id}"))
                        })?,
                )
            }
            None => None,
        };
        {
            let name = self.config.active_profile.clone();
            let p = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            p.default_sink_channel = channel.clone();
        }
        self.save_config()?;
        if let Some(sink_name) = sink {
            // pw-metadata accepts node names; wpctl set-default requires a numeric id.
            let val = format!("{{\"name\":\"{sink_name}\"}}");
            let out = self.runner.run(
                "pw-metadata",
                &["-n", "default", "0", "default.configured.audio.sink", &val],
            )?;
            if out.status != 0 {
                return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                    program: "pw-metadata".into(),
                    status: out.status,
                    stderr: out.stderr,
                }));
            }
        }
        self.emit(Event::DefaultSinkChannelSet { channel });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
        let dump = include_str!("../../../audio/tests/fixtures/pw_dump_app_streams.json");
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
        let dump = include_str!("../../../audio/tests/fixtures/pw_dump_sinks.json");
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
        let dump = include_str!("../../../audio/tests/fixtures/pw_dump_sinks.json");
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
        let dump = include_str!("../../../audio/tests/fixtures/pw_dump_app_streams.json");
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
        let dump = include_str!("../../../audio/tests/fixtures/pw_dump_app_streams.json");
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
}
