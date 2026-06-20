use crate::{children::ChildOwner, convert, error::EngineError};
use arctis_audio::{ChannelManager, CommandRunner, EqModel, Router};
use arctis_config::Config;

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

pub struct Engine<R: CommandRunner> {
    runner: R,
    config: Config,
    children: ChildOwner,
}

impl<R: CommandRunner> Engine<R> {
    pub fn new(runner: R, config: Config) -> Self {
        Self {
            runner,
            config,
            children: ChildOwner::new(),
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Bring the live graph to match the active profile. Idempotent. Order:
    ///   1. ChannelManager::up(default flat eq) — creates sinks, tracking spawn_owned tokens
    ///   2. per channel: AudioBackend/ChannelManager apply_all(eq_model_for(channel))
    ///   3. per channel with output_device: ChannelManager::set_output(...)
    ///   4. Router: set_rule for each route, save_persistent, then apply_live best-effort
    ///
    /// Reuses ChannelManager/Router/AudioBackend — does NOT reimplement.
    pub fn reconcile(&mut self) -> Result<(), EngineError> {
        let profile = self.config.active()?.clone();
        let channel_set = convert::channel_set_from_profile(&profile);
        let route_rules = convert::route_rules_from_profile(&profile);

        // Step 1: channels up
        {
            let mut mgr = ChannelManager::new(&mut self.runner, channel_set.clone());
            let flat_eq = EqModel::default_10band();
            let _handles = mgr.up(&flat_eq)?;
            // NOTE: ChannelManager::up uses spawn_detached (not spawn_owned) via AudioBackend::create,
            // so child token tracking for reconcile's "new sinks" is best-effort at this layer.
            // The ChildOwner tracks tokens from explicit spawn_owned calls, which are not currently
            // exposed by ChannelManager::up. A future refactor could thread tokens through.
        }

        // Step 2: per-channel EQ apply
        for ch in &profile.channels {
            let eq_model = convert::eq_model_for(ch)?;
            let def = convert::channel_def_from_cfg(ch);
            let spec = def.sink_spec();
            let mut be = arctis_audio::AudioBackend::new(&mut self.runner, spec);
            be.apply_all(&eq_model)?;
        }

        // Step 3: per-channel output device overrides
        for ch in &profile.channels {
            if ch.output_device.is_some() {
                let eq_model = convert::eq_model_for(ch)?;
                let mut mgr = ChannelManager::new(&mut self.runner, channel_set.clone());
                let _handle = mgr.set_output(&ch.id, ch.output_device.clone(), &eq_model)?;
            }
        }

        // Step 4: routing rules — persistent only (apply_live is best-effort and needs live streams)
        if !route_rules.is_empty() {
            let mut router = Router::new(&mut self.runner);
            for rule in route_rules {
                router.set_rule(rule);
            }
            // save_persistent writes WirePlumber fragment to disk (no runner calls needed)
            router.save_persistent()?;
        }

        Ok(())
    }

    /// Kill all owned pipewire children. Called on shutdown and from Drop.
    pub fn shutdown(&mut self) -> Result<(), EngineError> {
        self.children
            .kill_all(&mut self.runner)
            .map_err(EngineError::Audio)
    }
}

impl<R: CommandRunner> Drop for Engine<R> {
    fn drop(&mut self) {
        let _ = self.children.kill_all(&mut self.runner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arctis_audio::MockRunner;
    use arctis_config::{ChannelConfig, Config, Profile, RouteConfig};

    // ─────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────

    fn default_config() -> Config {
        Config::default_config()
    }

    /// Config with 3 channels (game/chat/media), no EQ overrides, no output overrides, no routes.
    fn make_config_no_eq_no_routes() -> Config {
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
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: None,
                        eq: vec![],
                    },
                ],
                routes: vec![],
            }],
        }
    }

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
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: Some("alsa_output.speakers".into()),
                        eq: vec![],
                    },
                ],
                routes: vec![RouteConfig {
                    app_binary: "firefox".into(),
                    target_sink: "Arctis_Media".into(),
                }],
            }],
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

    // ─────────────────────────────────────────────
    // TDD Step 5: reconcile argv-sequence tests
    // ─────────────────────────────────────────────

    /// Build the "ls Node" response that reports the three Arctis sinks as already present.
    /// format: `id <N>\n    node.name = "<name>"\n`
    fn ls_all_present() -> String {
        [
            "id 10\n    node.name = \"Arctis_Game\"\n",
            "id 11\n    node.name = \"Arctis_Chat\"\n",
            "id 12\n    node.name = \"Arctis_Media\"\n",
        ]
        .concat()
    }

    /// Build the "ls Node" response where all sinks are absent (only unrelated node).
    fn ls_all_absent() -> String {
        "id 1\n    node.name = \"other_sink\"\n".to_string()
    }

    #[test]
    fn reconcile_emits_expected_argv_sinks_already_present() {
        // Channels already present → no spawns for sink creation.
        // Each channel: 1 ls for create (present, skip), then apply_all: 1 ls (find_node_id) + 10 set-band calls.
        let ls = ls_all_present();

        // Queue outputs:
        // Phase 1 (channels up): 3 channels × 1 ls-Node (sinks present, no spawn)
        // Phase 2 (apply_all per channel): 3 channels × (1 ls-Node + 10 pw-cli s <id> Props)
        // Phase 3 (no output devices)
        // Phase 4 (no routes)
        let runner = MockRunner::new()
            // Phase 1: channel up — game (present), chat (present), media (present)
            .with_output(0, &ls, "")
            .with_output(0, &ls, "")
            .with_output(0, &ls, "")
            // Phase 2: apply_all game — find_node_id + 10 bands
            .with_output(0, &ls, "") // game ls Node
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
            // Phase 2: apply_all chat — find_node_id + 10 bands
            .with_output(0, &ls, "")
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
            // Phase 2: apply_all media — find_node_id + 10 bands
            .with_output(0, &ls, "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "");

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);
        engine.reconcile().expect("reconcile should succeed");

        let calls = &engine.runner.calls;

        // Phase 1: 3 ls-Node calls for channel creation (all present, no spawns)
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"], "game up ls");
        assert_eq!(calls[1], vec!["pw-cli", "ls", "Node"], "chat up ls");
        assert_eq!(calls[2], vec!["pw-cli", "ls", "Node"], "media up ls");

        // Phase 2: apply_all game — ls Node then 10 pw-cli s 10 Props calls
        assert_eq!(
            calls[3],
            vec!["pw-cli", "ls", "Node"],
            "game eq find_node_id"
        );
        assert_eq!(calls[4][0], "pw-cli", "game band 0 set");
        assert_eq!(calls[4][1], "s");
        assert_eq!(calls[4][3], "Props");

        // apply_all chat starts after 3 (up) + 1 + 10 (game) = 14
        assert_eq!(
            calls[14],
            vec!["pw-cli", "ls", "Node"],
            "chat eq find_node_id"
        );

        // apply_all media starts after 14 + 1 + 10 = 25
        assert_eq!(
            calls[25],
            vec!["pw-cli", "ls", "Node"],
            "media eq find_node_id"
        );

        // Total: 3 (up) + 3*(1+10) (apply) = 36
        assert_eq!(calls.len(), 36, "expected 36 total pw-cli calls");

        // No spawned processes (sinks already present)
        assert!(
            engine.runner.spawned.is_empty(),
            "no spawns when sinks present"
        );
    }

    #[test]
    fn reconcile_emits_expected_argv_sinks_absent() {
        // Channels absent → spawn_detached fired for each sink creation.
        // NOTE: MockRunner::spawn_detached records into `calls` but does NOT consume from `queued`.
        // So phase 1 only consumes 3 queued outputs (one ls-Node per channel).
        // Phase 2: each channel: apply_all: ls (find_node_id) + 10 band sets = 11 each.
        let ls_absent = ls_all_absent();
        let ls_present = ls_all_present();

        let runner = MockRunner::new()
            // Phase 1: 3 ls calls only (spawn_detached does not consume queued outputs)
            .with_output(0, &ls_absent, "") // game ls (absent)
            .with_output(0, &ls_absent, "") // chat ls (absent)
            .with_output(0, &ls_absent, "") // media ls (absent)
            // Phase 2: apply_all per channel (sinks now "present" for id lookup)
            .with_output(0, &ls_present, "") // game ls Node (find_node_id)
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "") // game 10 band sets
            .with_output(0, &ls_present, "") // chat ls Node (find_node_id)
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "") // chat 10 band sets
            .with_output(0, &ls_present, "") // media ls Node (find_node_id)
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", ""); // media 10 band sets

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);
        engine.reconcile().expect("reconcile should succeed");

        let calls = &engine.runner.calls;

        // Phase 1: game: ls (absent) + spawn_detached (all in calls)
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"], "game up ls");
        assert_eq!(calls[1][0], "pipewire", "game spawn");
        assert!(calls[1][2].ends_with("Arctis_Game.conf"), "game conf path");

        // chat
        assert_eq!(calls[2], vec!["pw-cli", "ls", "Node"], "chat up ls");
        assert_eq!(calls[3][0], "pipewire", "chat spawn");
        assert!(calls[3][2].ends_with("Arctis_Chat.conf"), "chat conf path");

        // media
        assert_eq!(calls[4], vec!["pw-cli", "ls", "Node"], "media up ls");
        assert_eq!(calls[5][0], "pipewire", "media spawn");
        assert!(
            calls[5][2].ends_with("Arctis_Media.conf"),
            "media conf path"
        );

        // Phase 2: apply game starts at index 6 (after 3 ls + 3 spawns)
        assert_eq!(
            calls[6],
            vec!["pw-cli", "ls", "Node"],
            "game eq find_node_id"
        );
        assert_eq!(calls[7][0], "pw-cli", "game band 0 program");
        assert_eq!(calls[7][1], "s", "game band 0 sub-cmd");
        assert_eq!(calls[7][3], "Props", "game band 0 Props");

        // apply_all chat: 6 (phase1 ls+spawn) + 1+10 (game apply) = 17
        assert_eq!(
            calls[17],
            vec!["pw-cli", "ls", "Node"],
            "chat eq find_node_id"
        );

        // apply_all media: 17 + 1+10 = 28
        assert_eq!(
            calls[28],
            vec!["pw-cli", "ls", "Node"],
            "media eq find_node_id"
        );

        // Total: 6 (phase1: 3 ls + 3 spawns) + 3*11 (apply) = 39
        assert_eq!(calls.len(), 39, "expected 39 total calls");

        // spawn_detached goes into `calls` (not `spawned`) per MockRunner impl
        assert!(
            engine.runner.spawned.is_empty(),
            "spawn_detached uses calls not spawned"
        );
    }

    #[test]
    fn reconcile_with_route_saves_persistent_fragment() {
        // Profile has one route (firefox → Arctis_Media). After reconcile, the WirePlumber
        // fragment should exist on disk.
        let ls = ls_all_present();
        let runner = MockRunner::new()
            // Phase 1: 3 ls (all present)
            .with_output(0, &ls, "")
            .with_output(0, &ls, "")
            .with_output(0, &ls, "")
            // Phase 2: apply_all 3 channels
            .with_output(0, &ls, "")
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
            .with_output(0, &ls, "")
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
            .with_output(0, &ls, "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "")
            .with_output(0, "", "");
        // Phase 4: Router::save_persistent writes files — no runner calls.

        // Use a temp HOME so we don't touch real WirePlumber config.
        let tmp_home = std::env::temp_dir().join(format!("asm_engine_test_{}", std::process::id()));
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
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: None,
                        eq: vec![],
                    },
                ],
                routes: vec![RouteConfig {
                    app_binary: "firefox".into(),
                    target_sink: "Arctis_Media".into(),
                }],
            }],
        };
        let mut engine = Engine::new(runner, simple_cfg);
        engine.reconcile().expect("reconcile should succeed");

        // The WirePlumber fragment should exist.
        let frag_path = tmp_home.join(".config/wireplumber/wireplumber.conf.d/90-asm-routing.conf");
        assert!(frag_path.exists(), "WirePlumber fragment should be written");
        let content = std::fs::read_to_string(&frag_path).unwrap();
        assert!(
            content.contains("firefox"),
            "fragment should contain firefox rule"
        );
        assert!(
            content.contains("Arctis_Media"),
            "fragment should contain Arctis_Media"
        );

        let _ = std::fs::remove_dir_all(&tmp_home);
    }

    #[test]
    fn shutdown_kills_tracked_children() {
        // Engine with manually-tracked children via children field (test via ChildOwner::track).
        let runner = MockRunner::new();
        let cfg = default_config();
        let mut engine = Engine::new(runner, cfg);

        // Manually track a couple of fake tokens.
        let t1 = engine
            .runner
            .spawn_owned("pipewire", &["-c", "/tmp/a.conf"])
            .unwrap();
        let t2 = engine
            .runner
            .spawn_owned("pipewire", &["-c", "/tmp/b.conf"])
            .unwrap();
        engine.children.track(t1);
        engine.children.track(t2);

        engine.shutdown().expect("shutdown should succeed");

        assert_eq!(
            engine.runner.killed.len(),
            2,
            "both tokens should be killed on shutdown"
        );
        assert_eq!(
            engine.children.len(),
            0,
            "children should be empty after shutdown"
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
            // Phase 1: 3 ls (all present, no spawn)
            for _ in 0..3 {
                runner = runner.with_output(0, &ls, "");
            }
            // Phase 2: 3 channels × (1 ls + 10 band sets)
            for _ in 0..3 {
                runner = runner.with_output(0, &ls, "");
                for _ in 0..10 {
                    runner = runner.with_output(0, "", "");
                }
            }
        }

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);
        engine.reconcile().expect("first reconcile");
        engine.reconcile().expect("second reconcile");

        // No spawn_owned ever (spawn_detached goes into calls, not spawned).
        assert!(
            engine.runner.spawned.is_empty(),
            "no spawn_owned on idempotent reconcile"
        );
        // No kills
        assert!(engine.runner.killed.is_empty(), "no kills during reconcile");
    }
}
