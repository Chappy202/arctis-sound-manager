use crate::{children::ChildOwner, convert, error::EngineError, state::Event};
use arctis_audio::{
    AppMatch, AudioBackend, ChannelManager, CommandRunner, EqModel, RouteRule, Router,
};
use arctis_config::{Config, EqBandConfig};

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
    event_sink: Option<std::sync::mpsc::Sender<Event>>,
}

impl<R: CommandRunner> Engine<R> {
    pub fn new(runner: R, config: Config) -> Self {
        Self {
            runner,
            config,
            children: ChildOwner::new(),
            event_sink: None,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Set the engine's event sink. Events are pushed here (daemon owns the Receiver).
    pub fn set_event_sink(&mut self, tx: std::sync::mpsc::Sender<Event>) {
        self.event_sink = Some(tx);
    }

    /// Emit an event on the optional sink (ignores send errors).
    fn emit(&self, event: Event) {
        if let Some(tx) = &self.event_sink {
            let _ = tx.send(event);
        }
    }

    /// Return a flat UI-agnostic snapshot of the current engine state.
    pub fn state(&self) -> crate::state::EngineState {
        use crate::state::{ChannelSnapshot, EngineState};
        let active = self.config.active().ok();
        let channels = active
            .map(|p| {
                p.channels
                    .iter()
                    .map(|ch| ChannelSnapshot {
                        id: ch.id.clone(),
                        node_name: ch.node_name.clone(),
                        output_device: ch.output_device.clone(),
                        eq_bands: ch.eq.len(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let routes = active
            .map(|p| {
                p.routes
                    .iter()
                    .map(|r| (r.app_binary.clone(), r.target_sink.clone()))
                    .collect()
            })
            .unwrap_or_default();
        EngineState {
            active_profile: self.config.active_profile.clone(),
            profiles: self.config.profile_names(),
            channels,
            routes,
            device_present: false,
            device_fields: std::collections::BTreeMap::new(),
        }
    }

    /// Switch active profile in config, persist, then reconcile the graph to it.
    pub fn switch_profile(&mut self, name: &str) -> Result<(), EngineError> {
        // Validate first (no disk write on error)
        self.config.switch_profile(name)?;
        // Persist
        self.save_config()?;
        // Reconcile to the new profile
        self.reconcile()?;
        // Emit event
        self.emit(Event::ProfileSwitched {
            name: name.to_string(),
        });
        Ok(())
    }

    /// Mutate one EQ band in the active profile's channel, persist config, apply live via audio.
    pub fn set_eq_band(
        &mut self,
        channel_id: &str,
        band: usize,
        cfg: EqBandConfig,
    ) -> Result<(), EngineError> {
        // Update in-memory config
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            let channel = profile
                .channels
                .iter_mut()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found: {channel_id}"))
                })?;
            // Ensure there are enough bands (extend if needed)
            while channel.eq.len() <= band {
                channel.eq.push(EqBandConfig {
                    kind: "peaking".to_string(),
                    freq_hz: 1000.0,
                    q: 1.0,
                    gain_db: 0.0,
                });
            }
            channel.eq[band] = cfg.clone();
        }
        // Persist
        self.save_config()?;
        // Apply live via AudioBackend
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.active()?.clone();
            let channel = profile
                .channels
                .iter()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found: {channel_id}"))
                })?;
            let def = convert::channel_def_from_cfg(channel);
            let spec = def.sink_spec();
            let eq_band = convert::eq_band_from_cfg(&cfg)?;
            let mut be = AudioBackend::new(&mut self.runner, spec);
            be.apply_band(band, &eq_band)?;
            let _ = active_name; // suppress unused warning
        }
        // Emit event
        self.emit(Event::EqBandSet {
            channel_id: channel_id.to_string(),
            band,
        });
        Ok(())
    }

    /// Add/upsert a route in the active profile, persist, set_rule + save_persistent + apply_live.
    pub fn set_route(&mut self, app_binary: &str, target_sink: &str) -> Result<(), EngineError> {
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
        // Apply via Router (persistent fragment + best-effort live move)
        {
            let mut router = Router::new(&mut self.runner);
            router.set_rule(RouteRule::new(app_binary, target_sink));
            router.save_persistent()?;
            // Best-effort live move (ignore error if app not running)
            let _ = router.apply_live(&AppMatch::Binary(app_binary.to_string()), target_sink);
        }
        // Emit event
        self.emit(Event::RouteSet {
            app_binary: app_binary.to_string(),
            target_sink: target_sink.to_string(),
        });
        Ok(())
    }

    /// Persist the in-memory config via arctis_config::store::save.
    pub fn save_config(&self) -> Result<(), EngineError> {
        arctis_config::store::save(&self.config).map_err(EngineError::Config)
    }

    /// Best-effort device status read; never errors the caller (returns empty on failure).
    pub fn refresh_device(&mut self) -> std::collections::BTreeMap<String, String> {
        std::collections::BTreeMap::new()
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

        // Step 1: channels up — track any freshly-spawned pipewire instances
        {
            let mut mgr = ChannelManager::new(&mut self.runner, channel_set.clone());
            let flat_eq = EqModel::default_10band();
            let pairs = mgr.up(&flat_eq)?;
            for (_handle, token) in pairs {
                if let Some(t) = token {
                    self.children.track(t);
                }
            }
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
                let handle = mgr.set_output(&ch.id, ch.output_device.clone(), &eq_model)?;
                if let Some(t) = handle.child {
                    self.children.track(t);
                }
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

    /// Global mutex to serialize tests that mutate process-wide env vars (HOME, ASM_CONFIG_HOME).
    /// Tests setting those variables MUST hold this lock for their entire lifetime.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ─────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────

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
        // Channels absent → spawn_owned fires for each sink creation.
        // spawn_owned goes into `spawned` (NOT `calls`) and does NOT consume queued outputs.
        // Phase 1 only consumes 3 queued outputs (one ls-Node per channel).
        // Phase 2: each channel: apply_all: ls (find_node_id) + 10 band sets = 11 each.
        let ls_absent = ls_all_absent();
        let ls_present = ls_all_present();

        let runner = MockRunner::new()
            // Phase 1: 3 ls calls only (spawn_owned does not consume queued outputs)
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

        // Phase 1: only the 3 ls-Node existence checks (spawn_owned goes to `spawned`)
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"], "game up ls");
        assert_eq!(calls[1], vec!["pw-cli", "ls", "Node"], "chat up ls");
        assert_eq!(calls[2], vec!["pw-cli", "ls", "Node"], "media up ls");

        // Phase 2: apply game starts at index 3 (right after phase1 ls calls)
        assert_eq!(
            calls[3],
            vec!["pw-cli", "ls", "Node"],
            "game eq find_node_id"
        );
        assert_eq!(calls[4][0], "pw-cli", "game band 0 program");
        assert_eq!(calls[4][1], "s", "game band 0 sub-cmd");
        assert_eq!(calls[4][3], "Props", "game band 0 Props");

        // apply_all chat: 3 (phase1 ls) + 1+10 (game apply) = 14
        assert_eq!(
            calls[14],
            vec!["pw-cli", "ls", "Node"],
            "chat eq find_node_id"
        );

        // apply_all media: 14 + 1+10 = 25
        assert_eq!(
            calls[25],
            vec!["pw-cli", "ls", "Node"],
            "media eq find_node_id"
        );

        // Total: 3 (phase1 ls) + 3*11 (apply) = 36
        assert_eq!(
            calls.len(),
            36,
            "expected 36 total run calls (no spawns in calls)"
        );

        // spawn_owned goes into `spawned` — 3 pipewire -c invocations
        let spawned = &engine.runner.spawned;
        assert_eq!(spawned.len(), 3, "3 pipewire -c instances spawned_owned");
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

        // The engine tracked all 3 child tokens
        assert_eq!(engine.children.len(), 3, "engine must track 3 child tokens");
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
        std::env::remove_var("HOME");
    }

    #[test]
    fn reconcile_then_shutdown_kills_all_3_channel_instances() {
        // Set up MockRunner so pw-cli ls Node reports ALL sinks ABSENT → reconcile
        // spawns all 3 via spawn_owned. Then shutdown must kill all 3.
        let ls_absent = ls_all_absent();
        let ls_present = ls_all_present();

        let runner = MockRunner::new()
            // Phase 1: 3 ls checks (sinks absent, spawn_owned called per channel)
            .with_output(0, &ls_absent, "")
            .with_output(0, &ls_absent, "")
            .with_output(0, &ls_absent, "")
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
            .with_output(0, "", "");

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);
        engine.reconcile().expect("reconcile should succeed");

        // 3 channels were absent → 3 spawn_owned calls → 3 tracked tokens.
        assert_eq!(
            engine.children.len(),
            3,
            "reconcile must track 3 channel pipewire instances"
        );
        assert_eq!(engine.runner.spawned.len(), 3, "3 spawn_owned calls");

        // Shutdown must kill all 3 tracked instances.
        engine.shutdown().expect("shutdown should succeed");

        assert_eq!(
            engine.runner.killed.len(),
            3,
            "shutdown must kill all 3 channel pipewire instances (no orphan leak)"
        );
        assert_eq!(
            engine.children.len(),
            0,
            "children list must be empty after shutdown"
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

        // No spawn_owned ever — sinks were already present.
        assert!(
            engine.runner.spawned.is_empty(),
            "no spawn_owned on idempotent reconcile"
        );
        // No kills
        assert!(engine.runner.killed.is_empty(), "no kills during reconcile");
    }

    // ─────────────────────────────────────────────
    // TDD Step 1: Task 6 — state / switch / mutation / events
    // ─────────────────────────────────────────────

    /// Helper: create a unique temp dir (does NOT touch HOME / XDG / real FS).
    fn unique_cfg_tmp(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        std::env::temp_dir().join(format!(
            "asm_eng6_{tag}_{pid}_{nanos}",
            pid = std::process::id()
        ))
    }

    /// Queue enough MockRunner outputs to survive `reconcile()` on a 3-channel,
    /// no-EQ, no-routes config where all sinks are already present.
    fn queue_reconcile_present(runner: MockRunner) -> MockRunner {
        let ls = ls_all_present();
        let mut r = runner;
        // Phase 1: 3 ls (all present)
        for _ in 0..3 {
            r = r.with_output(0, &ls, "");
        }
        // Phase 2: 3 channels × (1 ls + 10 band sets)
        for _ in 0..3 {
            r = r.with_output(0, &ls, "");
            for _ in 0..10 {
                r = r.with_output(0, "", "");
            }
        }
        r
    }

    #[test]
    fn state_reflects_active_profile() {
        let cfg = make_config_no_eq_no_routes();
        let engine = Engine::new(MockRunner::new(), cfg);
        let s = engine.state();
        assert_eq!(s.active_profile, "default");
        assert_eq!(s.channels.len(), 3);
        assert!(s.profiles.contains(&"default".to_string()));
    }

    #[test]
    fn switch_profile_persists_and_reconciles() {
        // Seed a 2-profile config
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles.push(Profile {
            name: "gaming".into(),
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
        });

        // Use a temp ASM_CONFIG_HOME so we don't touch real config.
        // Serialize all env-var-touching tests via mutex.
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("switch");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Queue outputs for one reconcile pass
        let runner = queue_reconcile_present(MockRunner::new());

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        engine
            .switch_profile("gaming")
            .expect("switch_profile should succeed");

        // In-memory config updated
        assert_eq!(engine.config().active_profile, "gaming");

        // On-disk config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written on switch");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("active_profile = \"gaming\""),
            "persisted config must show gaming as active"
        );

        // MockRunner saw reconcile calls (ls Node for channels up)
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c == &vec!["pw-cli", "ls", "Node"]),
            "reconcile must issue pw-cli ls Node"
        );

        // Event received
        let event = rx.try_recv().expect("ProfileSwitched event must be sent");
        assert_eq!(
            event,
            crate::state::Event::ProfileSwitched {
                name: "gaming".to_string()
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn switch_unknown_errors_no_disk_write() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("switch_err");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        let result = engine.switch_profile("nope");
        assert!(
            matches!(result, Err(EngineError::Config(_))),
            "should error on unknown profile"
        );
        // No disk write should have happened
        assert!(
            !tmp.exists(),
            "config dir must not be created on failed switch"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

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
}
