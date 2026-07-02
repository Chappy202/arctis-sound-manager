//! Reconcile: pure planning (plan_reconcile) and bringing the live graph to config.
use super::*;

/// A reconcile-step descriptor used for pure planning + test assertions before any I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileStep {
    ChannelsUp,
    ApplyEq {
        channel_id: String,
    },
    SetOutput {
        channel_id: String,
        device: Option<String>,
    },
    RouteSet {
        app_binary: String,
        target_sink: String,
    },
}

/// Pure planner: compute the ordered step list for a profile (no I/O). Lets us unit-test the
/// reconcile PLAN independently of execution (G8).
pub fn plan_reconcile(cfg: &Config) -> Result<Vec<ReconcileStep>, EngineError> {
    let profile = cfg.active()?;
    let mut steps = Vec::new();

    // Step 1: bring channels up
    steps.push(ReconcileStep::ChannelsUp);

    // Step 2: per-channel EQ apply
    for ch in &profile.channels {
        steps.push(ReconcileStep::ApplyEq {
            channel_id: ch.id.clone(),
        });
    }

    // Step 3: per-channel output device overrides
    for ch in &profile.channels {
        if ch.output_device.is_some() {
            steps.push(ReconcileStep::SetOutput {
                channel_id: ch.id.clone(),
                device: ch.output_device.clone(),
            });
        }
    }

    // Step 4: routing rules
    for route in &profile.routes {
        steps.push(ReconcileStep::RouteSet {
            app_binary: route.app_binary.clone(),
            target_sink: route.target_sink.clone(),
        });
    }

    Ok(steps)
}

impl<R: CommandRunner> Engine<R> {
    /// Bring the live graph to match the active profile. Idempotent. Order:
    ///   1. ChannelManager::up(default flat eq) — creates sinks, tracking spawn_owned tokens
    ///   2. per channel: AudioBackend/ChannelManager apply_all(eq_model_for(channel))
    ///   3. per channel with output_device: ChannelManager::set_output(...)
    ///   4. Router: set_rule for each route, save_persistent, then apply_live best-effort
    ///
    /// Reuses ChannelManager/Router/AudioBackend — does NOT reimplement.
    pub fn reconcile(&mut self) -> Result<(), EngineError> {
        let mut profile = self.config.active()?.clone();
        if let Some(headset) = self.detect_headset_sink() {
            convert::overlay_default_output(&mut profile.channels, &headset);
        }
        let channel_set = convert::channel_set_from_profile(&profile);
        let route_rules = convert::route_rules_from_profile(&profile);

        // Step 1: channels up — track any freshly-spawned pipewire instances.
        // Record which channel IDs were freshly spawned (token.is_some()) so Step 2
        // can treat the post-spawn apply_all as non-fatal for those channels.
        let freshly_spawned: std::collections::HashSet<String> = {
            let mut mgr = ChannelManager::new(&mut self.runner, channel_set.clone());
            let flat_eq = EqModel::default_10band();
            let pairs = mgr.up(&flat_eq)?;
            let mut fresh = std::collections::HashSet::new();
            for (i, (_handle, token)) in pairs.into_iter().enumerate() {
                if let Some(t) = token {
                    // Record the channel id as freshly spawned
                    if let Some(ch) = profile.channels.get(i) {
                        fresh.insert(ch.id.clone());
                    }
                    self.children.track(t);
                }
            }
            fresh
        };

        // Step 2: per-channel EQ apply.
        // For freshly-spawned channels the EQ is already baked into the filter-chain
        // conf written at spawn time, so this live apply_all is a redundant re-apply
        // for the initial state. A transient "node not yet registered" race (PipeWire
        // hasn't published the node to the graph yet) must not fail daemon startup —
        // log a warning and continue. For channels that were already present we treat
        // the error the same way (non-fatal warn) for consistency; those are idempotent
        // re-applies and a transient error there is equally recoverable.
        for ch in &profile.channels {
            let eq_model = convert::eq_model_for(ch)?;
            let def = convert::channel_def_from_cfg(ch);
            let spec = def.sink_spec();
            {
                let mut be = arctis_audio::AudioBackend::new(&mut self.runner, spec.clone());
                if let Err(e) = be.apply_all(&eq_model) {
                    let freshness = if freshly_spawned.contains(&ch.id) {
                        "freshly-spawned"
                    } else {
                        "already-present"
                    };
                    eprintln!(
                        "warning: reconcile apply_all for channel '{}' ({freshness}) failed \
                         (EQ is conf-baked; ignoring): {e}",
                        ch.id
                    );
                }
            }
            // Volume/mute apply
            {
                let mut be2 = arctis_audio::AudioBackend::new(&mut self.runner, spec);
                if let Err(e) = be2.apply_volume_mute_pct(ch.volume_pct, ch.muted) {
                    eprintln!(
                        "warning: reconcile apply_volume_mute_pct for channel '{}' failed (ignoring): {e}",
                        ch.id
                    );
                }
            }
        }

        // Step 3: per-channel output device overrides.
        // SKIP surround-routed channels: their live output is owned by Step 6
        // (apply_surround routes them to effect_input.arctis_surround). Without this
        // skip, Step 3 re-points the channel to its output_device (the headset, via
        // overlay_default_output) on every reconcile, while apply_surround's no-thrash
        // guard then declines to re-route it — silently bypassing the HRIR after the
        // first reconcile (bug C1). One writer per channel = no fight.
        for ch in &profile.channels {
            if ch.output_device.is_some()
                && !convert::surround_routes_channel(&profile.surround, &ch.id)
            {
                let eq_model = convert::eq_model_for(ch)?;
                let mut mgr = ChannelManager::new(&mut self.runner, channel_set.clone());
                let handle = mgr.set_output(&ch.id, ch.output_device.clone(), &eq_model)?;
                if let Some(t) = handle.child {
                    self.children.track(t);
                }
            }
        }

        // Step 4: routing rules — persistent projection only (apply_live is
        // best-effort and needs live streams). Always save, even when empty:
        // routes.json and the conf fragments are pure projections of
        // profile.routes (G4), so an empty profile must clear them rather than
        // leave stale rules behind. save_persistent is disk-only (no runner calls).
        {
            let router = Router::with_rules(&mut self.runner, route_rules);
            router.save_persistent()?;
        }

        // Step 5: mic source build/teardown (Clean Mic virtual Audio/Source).
        // Query PW version once (cached) then build availability from the probe.
        self.ensure_pw_version();
        let (nodes, availability) =
            convert::mic_chain_nodes(&profile.mic, self.probe.as_ref(), self.builtin_noisegate);
        self.mic_availability = availability;

        if !profile.mic.enabled {
            // Master switch off: remove the source (idempotent).
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            if let Err(e) = mic_be.remove() {
                eprintln!("warning: reconcile mic remove failed (ignoring): {e}");
            }
        } else {
            // Master switch on: create (idempotent — no-op if already present).
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            match mic_be.create(&nodes) {
                Ok(handle) => {
                    if let Some(token) = handle.child {
                        self.children.track(token);
                    }
                }
                Err(e) => {
                    // Transient post-spawn find_node_id race is non-fatal (mirror channel pattern).
                    eprintln!("warning: reconcile mic create failed (post-spawn race?): {e}");
                }
            }
        }

        // Step 6: surround sink create/teardown + channel re-routing (surround channels → effect_input.arctis_surround).
        // AFTER step 3/4 so that surround re-routing overrides any output-device set in step 3.
        if let Err(e) = self.apply_surround(&profile) {
            eprintln!("warning: reconcile surround step failed (ignoring): {e}");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use arctis_config::RouteConfig;

    /// Config with media pinned to "speakers" and one firefox→Arctis_Media route.
    fn make_config_with_output_and_route() -> Config {
        Config {
            version: arctis_config::CURRENT_VERSION,
            active_profile: "default".into(),
            profiles: vec![Profile {
                name: "default".into(),
                channels: vec![
                    ChannelConfig {
                        id: "game".into(),
                        node_name: "Arctis_Game".into(),
                        description: "Game".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        volume_pct: 100,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        volume_pct: 100,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: Some("alsa_output.speakers".into()),
                        eq: vec![],
                        volume_db: 0.0,
                        volume_pct: 100,
                        muted: false,
                    },
                ],
                routes: vec![RouteConfig {
                    app_binary: "firefox".into(),
                    target_sink: "Arctis_Media".into(),
                }],
                mic: MicChainConfig::default(),
                surround: arctis_config::SurroundConfig::default(),
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

    // ─────────────────────────────────────────────
    // TDD Step 1: PURE plan_reconcile tests
    // ─────────────────────────────────────────────

    #[test]
    fn plan_reconcile_orders_steps_default_3chan_no_eq_no_routes() {
        let cfg = make_config_no_eq_no_routes();
        let steps = plan_reconcile(&cfg).expect("plan should not fail");

        // Expected: ChannelsUp, then ApplyEq for each channel in order, no SetOutput, no RouteSet
        assert_eq!(
            steps,
            vec![
                ReconcileStep::ChannelsUp,
                ReconcileStep::ApplyEq {
                    channel_id: "game".into()
                },
                ReconcileStep::ApplyEq {
                    channel_id: "chat".into()
                },
                ReconcileStep::ApplyEq {
                    channel_id: "media".into()
                },
            ]
        );
    }

    #[test]
    fn plan_reconcile_appends_set_output_and_route_set() {
        let cfg = make_config_with_output_and_route();
        let steps = plan_reconcile(&cfg).expect("plan should not fail");

        assert_eq!(
            steps,
            vec![
                ReconcileStep::ChannelsUp,
                ReconcileStep::ApplyEq {
                    channel_id: "game".into()
                },
                ReconcileStep::ApplyEq {
                    channel_id: "chat".into()
                },
                ReconcileStep::ApplyEq {
                    channel_id: "media".into()
                },
                ReconcileStep::SetOutput {
                    channel_id: "media".into(),
                    device: Some("alsa_output.speakers".into()),
                },
                ReconcileStep::RouteSet {
                    app_binary: "firefox".into(),
                    target_sink: "Arctis_Media".into(),
                },
            ]
        );
    }

    #[test]
    fn plan_reconcile_missing_active_profile_is_error() {
        let mut cfg = make_config_no_eq_no_routes();
        cfg.active_profile = "nonexistent".into();
        let result = plan_reconcile(&cfg);
        assert!(result.is_err());
    }

    #[test]
    fn reconcile_emits_expected_argv_sinks_already_present() {
        // Channels already present → no spawns for sink creation.
        // Engine::new seeds "aux" via ensure_standard_channels() → 4 channels total.
        // Per channel: Phase 2 (1 ls + 10 bands) then Phase 2b (1 ls + 1 Props), interleaved.
        // Reconcile processes each channel fully before moving to the next.
        let ls = ls_all_present(); // includes Arctis_Game/Chat/Media/Aux

        // Queue outputs (interleaved per channel):
        // detect_headset_sink: 2 calls (pw-metadata 0 + pw-dump)
        // Phase 1 (channels up): 4 × 1 ls-Node
        // Per channel (4 channels): (1 ls + 10 bands + 1 preamp) + (1 ls + 1 Props) = 14 × 4 = 56
        // Phase 5: 1 ls (mic disabled)
        // Phase 6: 1 ls (surround disabled)
        // Total: 2 + 4 + 56 + 1 + 1 = 64
        let mut runner = MockRunner::new()
            // detect_headset_sink: pw-metadata 0 + pw-dump []
            .with_output(0, "", "") // [0] detect: pw-metadata 0
            .with_output(0, "[]", ""); // [1] detect: pw-dump []
        // Phase 1: channel up — game[2], chat[3], media[4], aux[5] (all present)
        for _ in 0..4 {
            runner = runner.with_output(0, &ls, "");
        }
        // Per channel: Phase 2 (1 ls + 10 band sets + 1 preamp set) then
        // Phase 2b (1 ls + 1 vol/mute Props set).
        for _ in 0..4 {
            runner = runner.with_output(0, &ls, ""); // EQ find_node_id
            for _ in 0..11 {
                runner = runner.with_output(0, "", ""); // 10 bands + preamp
            }
            runner = runner
                .with_output(0, &ls, "") // vol find_node_id
                .with_output(0, "", ""); // vol Props set
        }
        // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
        // Phase 6: surround disabled → remove() → source_exists() → 1 ls (surround absent)
        let runner = runner.with_output(0, &ls, "").with_output(0, &ls, "");

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        let calls = &engine.runner.calls;

        // Phase 1: 4 ls-Node calls for channel creation (all present, no spawns)
        assert_eq!(calls[2], vec!["pw-cli", "ls", "Node"], "game up ls");
        assert_eq!(calls[3], vec!["pw-cli", "ls", "Node"], "chat up ls");
        assert_eq!(calls[4], vec!["pw-cli", "ls", "Node"], "media up ls");
        assert_eq!(calls[5], vec!["pw-cli", "ls", "Node"], "aux up ls");

        // Phase 2: apply_all game — ls Node then 10 pw-cli s Props calls
        assert_eq!(
            calls[6],
            vec!["pw-cli", "ls", "Node"],
            "game eq find_node_id"
        );
        assert_eq!(calls[7][0], "pw-cli", "game band 0 set");
        assert_eq!(calls[7][1], "s");
        assert_eq!(calls[7][3], "Props");

        // Preamp set closes each channel's apply_all — index 17 for game.
        assert_eq!(calls[17][0], "pw-cli", "game preamp set");
        assert!(
            calls[17][4].contains("eq_preamp:Mult"),
            "call 17 must be the game auto-preamp set: {:?}",
            calls[17]
        );

        // Phase 2b: apply_volume_mute game — index 18
        assert_eq!(
            calls[18],
            vec!["pw-cli", "ls", "Node"],
            "game vol find_node_id"
        );

        // Phase 2 chat: EQ starts at index 20
        assert_eq!(
            calls[20],
            vec!["pw-cli", "ls", "Node"],
            "chat eq find_node_id"
        );

        // Phase 2b chat: volume starts at index 32
        assert_eq!(
            calls[32],
            vec!["pw-cli", "ls", "Node"],
            "chat vol find_node_id"
        );

        // Phase 2 media: EQ starts at index 34
        assert_eq!(
            calls[34],
            vec!["pw-cli", "ls", "Node"],
            "media eq find_node_id"
        );

        // Phase 2b media: volume starts at index 46
        assert_eq!(
            calls[46],
            vec!["pw-cli", "ls", "Node"],
            "media vol find_node_id"
        );

        // Phase 2 aux: EQ starts at index 48
        assert_eq!(
            calls[48],
            vec!["pw-cli", "ls", "Node"],
            "aux eq find_node_id"
        );

        // Phase 2b aux: volume starts at index 60
        assert_eq!(
            calls[60],
            vec!["pw-cli", "ls", "Node"],
            "aux vol find_node_id"
        );

        // Total: 2 (detect) + 4 (up) + 4*(1+10+1+1+1) (apply_all+preamp+vol/mute) + 1 (mic step5) + 1 (surround step6) = 64
        assert_eq!(calls.len(), 64, "expected 64 total pw-cli calls");

        // No spawned processes (sinks already present, including aux)
        assert!(
            engine.runner.spawned.is_empty(),
            "no spawns when sinks present"
        );
    }

    #[test]
    fn reconcile_emits_expected_argv_sinks_absent() {
        // Channels absent → spawn_owned fires for each sink creation.
        // spawn_owned goes into `spawned` (NOT `calls`) and does NOT consume queued outputs.
        // Engine::new seeds "aux" via ensure_standard_channels() → 4 channels total.
        // Phase 1 consumes 4 ls calls (one per channel, all absent → all spawned).
        // Per channel: Phase 2 (ls + 10 bands) then Phase 2b (ls + 1 Props), interleaved.
        let ls_absent = ls_all_absent();
        let ls_present = ls_all_present();

        let mut runner = MockRunner::new()
            // detect_headset_sink: pw-metadata 0 + pw-dump []
            .with_output(0, "", "") // [0] detect: pw-metadata 0
            .with_output(0, "[]", ""); // [1] detect: pw-dump []
        // Phase 1: 4 ls calls only (spawn_owned does not consume queued outputs)
        for _ in 0..4 {
            runner = runner.with_output(0, &ls_absent, "");
        }
        // Per channel: Phase 2 (1 ls + 10 band sets + 1 preamp set) then
        // Phase 2b (1 ls + 1 vol/mute Props set).
        for _ in 0..4 {
            runner = runner.with_output(0, &ls_present, ""); // EQ find_node_id
            for _ in 0..11 {
                runner = runner.with_output(0, "", ""); // 10 bands + preamp
            }
            runner = runner
                .with_output(0, &ls_present, "") // vol find_node_id
                .with_output(0, "", ""); // vol Props set
        }
        // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
        // Phase 6: surround disabled → remove() → source_exists() → 1 ls (surround absent)
        let runner = runner
            .with_output(0, &ls_absent, "")
            .with_output(0, &ls_absent, "");

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        let calls = &engine.runner.calls;

        // Phase 1: only the 4 ls-Node existence checks (spawn_owned goes to `spawned`)
        assert_eq!(calls[2], vec!["pw-cli", "ls", "Node"], "game up ls");
        assert_eq!(calls[3], vec!["pw-cli", "ls", "Node"], "chat up ls");
        assert_eq!(calls[4], vec!["pw-cli", "ls", "Node"], "media up ls");
        assert_eq!(calls[5], vec!["pw-cli", "ls", "Node"], "aux up ls");

        // Phase 2: apply game EQ starts at index 6 (right after detect[0,1] + phase1 ls calls[2-5])
        assert_eq!(
            calls[6],
            vec!["pw-cli", "ls", "Node"],
            "game eq find_node_id"
        );
        assert_eq!(calls[7][0], "pw-cli", "game band 0 program");
        assert_eq!(calls[7][1], "s", "game band 0 sub-cmd");
        assert_eq!(calls[7][3], "Props", "game band 0 Props");

        // Phase 2b: apply game vol at index 18 (after 10 bands + preamp)
        assert_eq!(
            calls[18],
            vec!["pw-cli", "ls", "Node"],
            "game vol find_node_id"
        );

        // Phase 2 chat EQ: index 20
        assert_eq!(
            calls[20],
            vec!["pw-cli", "ls", "Node"],
            "chat eq find_node_id"
        );

        // Phase 2b chat vol: index 32
        assert_eq!(
            calls[32],
            vec!["pw-cli", "ls", "Node"],
            "chat vol find_node_id"
        );

        // Phase 2 media EQ: index 34
        assert_eq!(
            calls[34],
            vec!["pw-cli", "ls", "Node"],
            "media eq find_node_id"
        );

        // Phase 2b media vol: index 46
        assert_eq!(
            calls[46],
            vec!["pw-cli", "ls", "Node"],
            "media vol find_node_id"
        );

        // Phase 2 aux EQ: index 48
        assert_eq!(
            calls[48],
            vec!["pw-cli", "ls", "Node"],
            "aux eq find_node_id"
        );

        // Phase 2b aux vol: index 60
        assert_eq!(
            calls[60],
            vec!["pw-cli", "ls", "Node"],
            "aux vol find_node_id"
        );

        // Total: 2 (detect) + 4 (phase1 ls) + 4*(1+10+1+1+1) (EQ+preamp+vol) + 1 (mic step5) + 1 (surround step6) = 64
        assert_eq!(
            calls.len(),
            64,
            "expected 64 total run calls (no spawns in calls)"
        );

        // spawn_owned goes into `spawned` — 4 pipewire -c invocations (channels only)
        let spawned = &engine.runner.spawned;
        assert_eq!(spawned.len(), 4, "4 pipewire -c instances spawned_owned");
        assert_eq!(spawned[0][0], "pipewire", "game spawn program");
        assert!(
            spawned[0][2].ends_with("Arctis_Game.conf"),
            "game conf path"
        );
        assert!(
            spawned[1][2].ends_with("Arctis_Chat.conf"),
            "chat conf path"
        );
        assert!(
            spawned[2][2].ends_with("Arctis_Media.conf"),
            "media conf path"
        );
        assert!(
            spawned[3][2].ends_with("Arctis_Aux.conf"),
            "aux conf path"
        );

        // The engine tracked all 4 child tokens
        assert_eq!(engine.children.len(), 4, "engine must track 4 child tokens");
    }

    #[test]
    fn reconcile_with_route_saves_persistent_fragment() {
        // Profile has one route (firefox → Arctis_Media). After reconcile, the WirePlumber
        // fragment should exist on disk.
        let ls = ls_all_present();
        let runner = MockRunner::new()
            // detect_headset_sink: pw-metadata 0 + pw-dump []
            .with_output(0, "", "") // pw-metadata 0
            .with_output(0, "[]", "") // pw-dump []
            // Phase 1: 3 ls (all present)
            .with_output(0, &ls, "")
            .with_output(0, &ls, "")
            .with_output(0, &ls, "")
            // game: Phase 2 (EQ) + Phase 2b (vol/mute)
            .with_output(0, &ls, "") // game EQ ls
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "") // 10 band sets
            .with_output(0, &ls, "") // game vol ls
            .with_output(0, "", "") // game vol Props
            // chat: Phase 2 (EQ) + Phase 2b (vol/mute)
            .with_output(0, &ls, "") // chat EQ ls
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "") // 10 band sets
            .with_output(0, &ls, "") // chat vol ls
            .with_output(0, "", "") // chat vol Props
            // media: Phase 2 (EQ) + Phase 2b (vol/mute)
            .with_output(0, &ls, "") // media EQ ls
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "") // 10 band sets
            .with_output(0, &ls, "") // media vol ls
            .with_output(0, "", "") // media vol Props
            // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
            .with_output(0, &ls, "")
            // Phase 6: surround disabled → remove() → source_exists() → 1 ls (surround absent)
            .with_output(0, &ls, "");
        // Phase 4: Router::save_persistent writes files — no runner calls.

        // Use a temp HOME so we don't touch real WirePlumber config.
        // Serialize all env-var-touching tests via mutex.
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp_home = unique_cfg_tmp("route_frag");
        std::env::set_var("HOME", &tmp_home);

        let _cfg = make_config_with_output_and_route();
        // For this test, use the default config without output override to avoid
        // the recreate calls that require more queued outputs.
        let simple_cfg = Config {
            version: arctis_config::CURRENT_VERSION,
            active_profile: "default".into(),
            profiles: vec![Profile {
                name: "default".into(),
                channels: vec![
                    ChannelConfig {
                        id: "game".into(),
                        node_name: "Arctis_Game".into(),
                        description: "Game".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        volume_pct: 100,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        volume_pct: 100,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        volume_pct: 100,
                        muted: false,
                    },
                ],
                routes: vec![RouteConfig {
                    app_binary: "firefox".into(),
                    target_sink: "Arctis_Media".into(),
                }],
                mic: MicChainConfig::default(),
                surround: arctis_config::SurroundConfig::default(),
                master_volume_db: 0.0,
                master_volume_pct: 100,
                master_mute: false,
                chatmix_position: 4,
                default_sink_channel: None,
            }],
            eq_presets: vec![],
            dial_controls_balance: true,
            knob_controls_master: true,
        };
        let mut engine = Engine::new(runner, simple_cfg);
        engine.reconcile().expect("reconcile should succeed");

        // Both persistent routing fragments should exist (client + pulse).
        for frag_path in [
            tmp_home.join(".config/pipewire/client.conf.d/90-asm-routing.conf"),
            tmp_home.join(".config/pipewire/pipewire-pulse.conf.d/90-asm-routing.conf"),
        ] {
            assert!(frag_path.exists(), "fragment should be written: {frag_path:?}");
            let content = std::fs::read_to_string(&frag_path).unwrap();
            assert!(
                content.contains("firefox"),
                "fragment should contain firefox rule"
            );
            assert!(
                content.contains("Arctis_Media"),
                "fragment should contain Arctis_Media"
            );
        }

        let _ = std::fs::remove_dir_all(&tmp_home);
        std::env::remove_var("HOME");
    }

    #[test]
    fn reconcile_then_shutdown_kills_all_3_channel_instances() {
        // Set up MockRunner so pw-cli ls Node reports ALL sinks ABSENT → reconcile
        // spawns all 4 (game/chat/media/aux) via spawn_owned. Then shutdown must kill all 4.
        // Engine::new calls ensure_standard_channels() → aux is seeded automatically.
        let ls_absent = ls_all_absent();
        let ls_present = ls_all_present();

        let runner = MockRunner::new()
            // detect_headset_sink: pw-metadata 0 + pw-dump []
            .with_output(0, "", "") // pw-metadata 0
            .with_output(0, "[]", "") // pw-dump []
            // Phase 1: 4 ls checks (sinks absent, spawn_owned called per channel)
            .with_output(0, &ls_absent, "")
            .with_output(0, &ls_absent, "")
            .with_output(0, &ls_absent, "")
            .with_output(0, &ls_absent, "") // aux (seeded by Engine::new)
            // Phase 2: apply_all game — find_node_id + 10 band sets
            .with_output(0, &ls_present, "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            // Phase 2: apply_all chat
            .with_output(0, &ls_present, "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            // Phase 2: apply_all media
            .with_output(0, &ls_present, "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            // Phase 2: apply_all aux
            .with_output(0, &ls_present, "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
            .with_output(0, &ls_absent, "")
            // Phase 6: surround disabled → remove() → source_exists() → 1 ls (surround absent)
            .with_output(0, &ls_absent, "");

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);
        engine.reconcile().expect("reconcile should succeed");

        // 4 channels were absent → 4 spawn_owned calls → 4 tracked tokens.
        assert_eq!(
            engine.children.len(),
            4,
            "reconcile must track 4 channel pipewire instances"
        );
        assert_eq!(engine.runner.spawned.len(), 4, "4 spawn_owned calls");

        // Shutdown must kill all 4 tracked instances.
        engine.shutdown().expect("shutdown should succeed");

        assert_eq!(
            engine.runner.killed.len(),
            4,
            "shutdown must kill all 4 channel pipewire instances (no orphan leak)"
        );
        assert_eq!(
            engine.children.len(),
            0,
            "children list must be empty after shutdown"
        );
    }

    /// Regression test: post-spawn apply_all must be NON-FATAL when find_node_id fails.
    ///
    /// Simulates the real-PipeWire timing race: channels are freshly spawned (ls absent
    /// in Phase 1), but in Phase 2 the node has not registered yet (ls still absent).
    /// apply_all returns a Parse error; reconcile must return Ok (EQ is conf-baked).
    #[test]
    fn reconcile_ok_when_post_spawn_apply_all_node_not_yet_registered() {
        let ls_absent = ls_all_absent();

        // Engine::new seeds aux → 4 channels total.
        // Phase 1: 4 ls-absent → spawn_owned per channel (tokens tracked)
        // Phase 2: 4 ls-absent for find_node_id → apply_all errors → non-fatal warn
        // Phase 2b: 4 ls-absent for apply_volume_mute find_node_id → also non-fatal
        // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
        // Phase 6: surround disabled → remove() → source_exists() → 1 ls
        let runner = MockRunner::new()
            // detect_headset_sink: pw-metadata 0 + pw-dump []
            .with_output(0, "", "") // pw-metadata 0
            .with_output(0, "[]", "") // pw-dump []
            // Phase 1 ls checks (absent → spawns)
            .with_output(0, &ls_absent, "")
            .with_output(0, &ls_absent, "")
            .with_output(0, &ls_absent, "")
            .with_output(0, &ls_absent, "") // aux
            // Phase 2: find_node_id for each channel — node absent → Parse error (warn+continue)
            .with_output(0, &ls_absent, "") // game
            .with_output(0, &ls_absent, "") // chat
            .with_output(0, &ls_absent, "") // media
            .with_output(0, &ls_absent, "") // aux
            // Phase 2b: apply_volume_mute find_node_id for each channel — also absent (warn+continue)
            .with_output(0, &ls_absent, "") // game
            .with_output(0, &ls_absent, "") // chat
            .with_output(0, &ls_absent, "") // media
            .with_output(0, &ls_absent, "") // aux
            // Phase 5: mic source_exists check
            .with_output(0, &ls_absent, "")
            // Phase 6: surround disabled → remove() → source_exists() → 1 ls
            .with_output(0, &ls_absent, "");

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));

        // Must return Ok — the Parse error from find_node_id is non-fatal
        let result = engine.reconcile();
        assert!(
            result.is_ok(),
            "reconcile must return Ok when post-spawn apply_all fails with node-not-yet-registered: {result:?}"
        );

        // 4 channels spawned (aux seeded by Engine::new)
        assert_eq!(
            engine.runner.spawned.len(),
            4,
            "4 pipewire instances spawned"
        );
        // 4 tracked tokens
        assert_eq!(engine.children.len(), 4, "4 child tokens tracked");

        // 16 run calls total: 2 (detect) + 4 (phase1 ls) + 4 (phase2 find_node_id) + 4 (phase2b find_node_id) + 1 (mic step5) + 1 (surround step6)
        assert_eq!(
            engine.runner.calls.len(),
            16,
            "expected 16 run calls: 2 detect + 4 ls-up + 4 ls-find-node + 4 ls-vol-find-node + 1 mic source_exists + 1 surround source_exists"
        );
    }

    #[test]
    fn reconcile_is_idempotent_no_spawn_when_present() {
        // Running reconcile twice with sinks present should not spawn.
        // Second reconcile same config → same output count but NO spawn_owned calls.
        let ls = ls_all_present();

        // Queue enough outputs for two full reconcile passes.
        let mut runner = MockRunner::new();
        for _ in 0..2 {
            // detect_headset_sink: pw-metadata 0 + pw-dump []
            runner = runner.with_output(0, "", ""); // pw-metadata 0
            runner = runner.with_output(0, "[]", ""); // pw-dump []
            // Phase 1: 4 ls (all present, no spawn — Engine::new seeds aux)
            for _ in 0..4 {
                runner = runner.with_output(0, &ls, "");
            }
            // Phase 2 + 2b interleaved: per channel (4), EQ apply then volume/mute apply
            for _ in 0..4 {
                // Phase 2: EQ apply (1 ls + 10 band sets)
                runner = runner.with_output(0, &ls, "");
                for _ in 0..10 {
                    runner = runner.with_output(0, "", "");
                }
                // Phase 2b: volume/mute apply (1 ls + 1 Props set)
                runner = runner.with_output(0, &ls, ""); // find_node_id
                runner = runner.with_output(0, "", ""); // Props set
            }
            // Phase 5: mic disabled → remove() → source_exists() → 1 ls
            runner = runner.with_output(0, &ls, "");
            // Phase 6: surround disabled → remove() → source_exists() → 1 ls
            runner = runner.with_output(0, &ls, "");
        }

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);
        engine.reconcile().expect("first reconcile");
        engine.reconcile().expect("second reconcile");

        // No spawn_owned ever — sinks were already present.
        assert!(
            engine.runner.spawned.is_empty(),
            "no spawn_owned on idempotent reconcile"
        );
        // No kills
        assert!(engine.runner.killed.is_empty(), "no kills during reconcile");
    }
}
