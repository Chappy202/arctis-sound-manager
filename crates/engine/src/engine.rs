use crate::{children::ChildOwner, convert, error::EngineError, state::Event};
use arctis_audio::{
    AppMatch, AudioBackend, ChannelManager, CommandRunner, EqModel, FsPluginProbe, MicBackend,
    PluginProbe, RouteRule, Router, StageKind,
};
use arctis_config::{Config, EqBandConfig};
use arctis_domain::{
    MIC_COMP_MAKEUP_MAX_DB, MIC_COMP_MAKEUP_MIN_DB, MIC_COMP_RATIO_MAX, MIC_COMP_RATIO_MIN,
    MIC_COMP_THRESHOLD_MAX_DB, MIC_COMP_THRESHOLD_MIN_DB, MIC_GAIN_MAX_DB, MIC_GAIN_MIN_DB,
    MIC_GATE_THRESHOLD_MAX, MIC_GATE_THRESHOLD_MIN, MIC_HIGHPASS_MAX_HZ, MIC_HIGHPASS_MIN_HZ,
    MIC_VAD_GRACE_MAX_MS, MIC_VAD_GRACE_MIN_MS, MIC_VAD_RETRO_GRACE_MAX_MS,
    MIC_VAD_RETRO_GRACE_MIN_MS, MIC_VAD_THRESHOLD_MAX, MIC_VAD_THRESHOLD_MIN,
};
use std::sync::Arc;

pub use crate::state::MicParam;

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
    device: std::sync::Arc<std::sync::Mutex<crate::state::DeviceShared>>,
    /// Sender to the DeviceWorker write-command channel. Set after the worker is spawned.
    device_tx: Option<std::sync::mpsc::Sender<crate::device::DeviceCommand>>,
    /// Plugin probe for availability detection (injected — default = FsPluginProbe).
    probe: Box<dyn PluginProbe>,
    /// Last-reconcile mic stage availability (stored for state() snapshot).
    mic_availability: Vec<crate::state::StageAvailability>,
}

impl<R: CommandRunner> Engine<R> {
    pub fn new(runner: R, config: Config) -> Self {
        Self {
            runner,
            config,
            children: ChildOwner::new(),
            event_sink: None,
            device: std::sync::Arc::new(std::sync::Mutex::new(
                crate::state::DeviceShared::default(),
            )),
            device_tx: None,
            probe: Box::new(FsPluginProbe),
            mic_availability: Vec::new(),
        }
    }

    /// Test constructor that allows injecting a custom `PluginProbe` for hermetic unit tests.
    /// All existing `Engine::new(...)` call sites remain unchanged.
    #[cfg(test)]
    pub fn with_probe(runner: R, config: Config, probe: Box<dyn PluginProbe>) -> Self {
        Self {
            runner,
            config,
            children: ChildOwner::new(),
            event_sink: None,
            device: std::sync::Arc::new(std::sync::Mutex::new(
                crate::state::DeviceShared::default(),
            )),
            device_tx: None,
            probe,
            mic_availability: Vec::new(),
        }
    }

    /// Return a clone of the Arc holding the shared device state.
    /// The DeviceWorker (spawned externally) writes to this; engine::state() reads it.
    pub fn device_shared(&self) -> std::sync::Arc<std::sync::Mutex<crate::state::DeviceShared>> {
        Arc::clone(&self.device)
    }

    /// Wire up the DeviceWorker command channel so `device_set` can route writes
    /// to the single-owner worker thread. Called after the worker is spawned.
    pub fn set_device_tx(&mut self, tx: std::sync::mpsc::Sender<crate::device::DeviceCommand>) {
        self.device_tx = Some(tx);
    }

    /// Send a validated device write through the worker thread.
    ///
    /// Returns `Err` if:
    /// - the worker is not running (`device_tx` is `None`),
    /// - the channel is broken (worker thread died), or
    /// - the write is rejected by the `enabled_writes` gate (control not yet owner-validated).
    ///
    /// Surfaces all failures — never swallows errors.
    pub fn device_set(&self, name: &str, value: i64) -> Result<(), EngineError> {
        let tx = self
            .device_tx
            .as_ref()
            .ok_or_else(|| EngineError::BadRequest("device worker not running".into()))?;
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        tx.send(crate::device::DeviceCommand::Set {
            name: name.to_string(),
            value,
            reply: reply_tx,
        })
        .map_err(|_| EngineError::BadRequest("device worker gone".into()))?;
        reply_rx
            .recv()
            .map_err(|_| EngineError::BadRequest("no reply from device worker".into()))?
            .map_err(EngineError::Device)
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
        use crate::state::{
            ChannelSnapshot, EngineState, EqBandSnapshot, MicSnapshot, MicStageSnapshot, StageName,
        };
        let active = self.config.active().ok();
        let channels = active
            .map(|p| {
                p.channels
                    .iter()
                    .map(|ch| ChannelSnapshot {
                        id: ch.id.clone(),
                        node_name: ch.node_name.clone(),
                        output_device: ch.output_device.clone(),
                        eq_bands: ch
                            .eq
                            .iter()
                            .map(|b| EqBandSnapshot {
                                kind: b.kind.clone(),
                                freq_hz: b.freq_hz,
                                q: b.q,
                                gain_db: b.gain_db,
                            })
                            .collect(),
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
        let dev = self.device.lock().map(|g| g.clone()).unwrap_or_default();

        // Build mic snapshot from active profile config + last reconcile availability.
        let mic = if let Ok(p) = self.config.active() {
            let mc = &p.mic;

            // Build a lookup map from stage → available (from last reconcile).
            let avail_map: std::collections::HashMap<StageName, bool> = self
                .mic_availability
                .iter()
                .map(|a| (a.stage, a.available))
                .collect();

            // Emit a MicStageSnapshot for each stage (even if not requested).
            let stages = vec![
                {
                    let avail = avail_map.get(&StageName::Gain).copied().unwrap_or(true);
                    let mut params = std::collections::BTreeMap::new();
                    if mc.gain.enabled {
                        params.insert("gain_db".to_string(), mc.gain.gain_db);
                    }
                    MicStageSnapshot {
                        kind: StageName::Gain,
                        enabled: mc.gain.enabled,
                        available: avail,
                        params,
                    }
                },
                {
                    let avail = avail_map.get(&StageName::Highpass).copied().unwrap_or(true);
                    let mut params = std::collections::BTreeMap::new();
                    if mc.highpass.enabled {
                        params.insert("freq_hz".to_string(), mc.highpass.freq_hz);
                    }
                    MicStageSnapshot {
                        kind: StageName::Highpass,
                        enabled: mc.highpass.enabled,
                        available: avail,
                        params,
                    }
                },
                {
                    let avail = avail_map.get(&StageName::Rnnoise).copied().unwrap_or(false);
                    let mut params = std::collections::BTreeMap::new();
                    if mc.rnnoise.enabled {
                        params.insert("vad_threshold".to_string(), mc.rnnoise.vad_threshold);
                        params.insert("vad_grace_ms".to_string(), mc.rnnoise.vad_grace_ms);
                        params.insert(
                            "vad_retro_grace_ms".to_string(),
                            mc.rnnoise.vad_retro_grace_ms,
                        );
                    }
                    MicStageSnapshot {
                        kind: StageName::Rnnoise,
                        enabled: mc.rnnoise.enabled,
                        available: avail,
                        params,
                    }
                },
                {
                    let avail = avail_map
                        .get(&StageName::Compressor)
                        .copied()
                        .unwrap_or(false);
                    let mut params = std::collections::BTreeMap::new();
                    if mc.compressor.enabled {
                        params.insert("threshold_db".to_string(), mc.compressor.threshold_db);
                        params.insert("ratio".to_string(), mc.compressor.ratio);
                        params.insert("makeup_db".to_string(), mc.compressor.makeup_db);
                    }
                    MicStageSnapshot {
                        kind: StageName::Compressor,
                        enabled: mc.compressor.enabled,
                        available: avail,
                        params,
                    }
                },
                {
                    let avail = avail_map.get(&StageName::Gate).copied().unwrap_or(true);
                    let mut params = std::collections::BTreeMap::new();
                    if mc.gate.enabled {
                        params.insert("threshold".to_string(), mc.gate.threshold);
                    }
                    MicStageSnapshot {
                        kind: StageName::Gate,
                        enabled: mc.gate.enabled,
                        available: avail,
                        params,
                    }
                },
                {
                    let avail = avail_map.get(&StageName::MicEq).copied().unwrap_or(true);
                    MicStageSnapshot {
                        kind: StageName::MicEq,
                        enabled: mc.eq_enabled,
                        available: avail,
                        params: std::collections::BTreeMap::new(),
                    }
                },
            ];
            let eq_bands = mc
                .eq
                .iter()
                .map(|b| EqBandSnapshot {
                    kind: b.kind.clone(),
                    freq_hz: b.freq_hz,
                    q: b.q,
                    gain_db: b.gain_db,
                })
                .collect();
            MicSnapshot {
                enabled: mc.enabled,
                stages,
                eq_bands,
            }
        } else {
            MicSnapshot::default()
        };

        EngineState {
            active_profile: self.config.active_profile.clone(),
            profiles: self.config.profile_names(),
            channels,
            routes,
            device_present: dev.present,
            device_fields: dev.fields,
            mic,
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

    /// Set (or clear) the output device for a single channel in the active profile.
    ///
    /// Updates the in-memory config, persists it atomically, rebuilds that
    /// channel live via `ChannelManager::set_output`, tracks any new child token,
    /// and emits a `ChannelOutputSet` event.
    pub fn set_channel_output(
        &mut self,
        channel_id: &str,
        device: Option<String>,
    ) -> Result<(), EngineError> {
        // Validate channel exists before touching disk
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
            channel.output_device = device.clone();
        }
        // Persist
        self.save_config()?;
        // Apply live: rebuild that channel with the new output device
        {
            let profile = self.config.active()?.clone();
            let channel = profile
                .channels
                .iter()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found: {channel_id}"))
                })?;
            let eq_model = convert::eq_model_for(channel)?;
            let channel_set = convert::channel_set_from_profile(&profile);
            let mut mgr = ChannelManager::new(&mut self.runner, channel_set);
            let handle = mgr.set_output(channel_id, device.clone(), &eq_model)?;
            if let Some(t) = handle.child {
                self.children.track(t);
            }
        }
        // Emit event
        self.emit(Event::ChannelOutputSet {
            channel_id: channel_id.to_string(),
            device,
        });
        Ok(())
    }

    /// Create a new profile by cloning the currently active one under `name`,
    /// make it active, persist the config, reconcile the graph to it, and emit
    /// a `ProfileCreated` event.
    pub fn new_profile(&mut self, name: &str) -> Result<(), EngineError> {
        // new_profile_from_active validates (errors on duplicate name), clones, sets active
        self.config
            .new_profile_from_active(name)
            .map_err(EngineError::Config)?;
        // Persist
        self.save_config()?;
        // Reconcile to the new (identical) profile
        self.reconcile()?;
        // Emit event
        self.emit(Event::ProfileCreated {
            name: name.to_string(),
        });
        Ok(())
    }

    /// Persist the in-memory config via arctis_config::store::save.
    pub fn save_config(&self) -> Result<(), EngineError> {
        arctis_config::store::save(&self.config).map_err(EngineError::Config)
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
            let mut be = arctis_audio::AudioBackend::new(&mut self.runner, spec);
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

        // Step 5: mic source build/teardown (Clean Mic virtual Audio/Source).
        // Build availability from the probe (disjoint borrow from self.runner).
        let (nodes, availability) = convert::mic_chain_nodes(&profile.mic, self.probe.as_ref());
        self.mic_availability = availability;

        if !profile.mic.enabled {
            // Master switch off: remove the source (idempotent).
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, FsPluginProbe, spec);
            if let Err(e) = mic_be.remove() {
                eprintln!("warning: reconcile mic remove failed (ignoring): {e}");
            }
        } else {
            // Master switch on: create (idempotent — no-op if already present).
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, FsPluginProbe, spec);
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

        Ok(())
    }

    /// Kill all owned pipewire children. Called on shutdown and from Drop.
    pub fn shutdown(&mut self) -> Result<(), EngineError> {
        self.children
            .kill_all(&mut self.runner)
            .map_err(EngineError::Audio)
    }

    // ── Mic engine methods ───────────────────────────────────────────────────

    /// Enable or disable a specific mic DSP stage.
    /// Topology change → full recreate (remove + respawn with new node list).
    pub fn mic_set_stage_enabled(&mut self, stage: StageKind, on: bool) -> Result<(), EngineError> {
        // Mutate config
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            match stage {
                StageKind::Gain => profile.mic.gain.enabled = on,
                StageKind::Highpass => profile.mic.highpass.enabled = on,
                StageKind::Rnnoise => profile.mic.rnnoise.enabled = on,
                StageKind::Compressor => profile.mic.compressor.enabled = on,
                StageKind::Gate => profile.mic.gate.enabled = on,
                StageKind::MicEq => profile.mic.eq_enabled = on,
            }
        }
        self.save_config()?;

        // Recreate: remove then create with new node list.
        {
            let profile = self.config.active()?.clone();
            let (nodes, availability) = convert::mic_chain_nodes(&profile.mic, self.probe.as_ref());
            self.mic_availability = availability;
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, FsPluginProbe, spec);
            let handle = mic_be.recreate(&nodes)?;
            if let Some(token) = handle.child {
                self.children.track(token);
            }
        }

        self.emit(Event::MicStageSet {
            stage: crate::state::StageName::from(stage),
            enabled: on,
        });
        Ok(())
    }

    /// Set a single mic DSP parameter live (in-place via `apply_control`).
    /// Validates range against domain bounds; errors on out-of-range.
    pub fn mic_set_param(&mut self, param: MicParam, value: f32) -> Result<(), EngineError> {
        // Validate and mutate config
        let control_name: &'static str;
        let node_name: &'static str;
        {
            let engine_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&engine_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    engine_name.clone(),
                ))
            })?;
            let mic = &mut profile.mic;
            match param {
                MicParam::GainDb => {
                    if !(MIC_GAIN_MIN_DB..=MIC_GAIN_MAX_DB).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "gain_db {value} out of range {MIC_GAIN_MIN_DB}..={MIC_GAIN_MAX_DB}"
                        )));
                    }
                    mic.gain.gain_db = value;
                    node_name = "mic_gain";
                    control_name = "Mult";
                }
                MicParam::HighpassFreq => {
                    if !(MIC_HIGHPASS_MIN_HZ..=MIC_HIGHPASS_MAX_HZ).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "highpass_freq {value} Hz out of range {MIC_HIGHPASS_MIN_HZ}..={MIC_HIGHPASS_MAX_HZ}"
                        )));
                    }
                    mic.highpass.freq_hz = value;
                    node_name = "mic_highpass";
                    control_name = "Freq";
                }
                MicParam::VadThreshold => {
                    if !(MIC_VAD_THRESHOLD_MIN..=MIC_VAD_THRESHOLD_MAX).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "vad_threshold {value} out of range {MIC_VAD_THRESHOLD_MIN}..={MIC_VAD_THRESHOLD_MAX}"
                        )));
                    }
                    mic.rnnoise.vad_threshold = value;
                    node_name = "mic_rnnoise";
                    control_name = "VAD Threshold (%)";
                }
                MicParam::VadGraceMs => {
                    if !(MIC_VAD_GRACE_MIN_MS..=MIC_VAD_GRACE_MAX_MS).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "vad_grace_ms {value} ms out of range {MIC_VAD_GRACE_MIN_MS}..={MIC_VAD_GRACE_MAX_MS}"
                        )));
                    }
                    mic.rnnoise.vad_grace_ms = value;
                    node_name = "mic_rnnoise";
                    control_name = "VAD Grace Period (ms)";
                }
                MicParam::VadRetroGraceMs => {
                    if !(MIC_VAD_RETRO_GRACE_MIN_MS..=MIC_VAD_RETRO_GRACE_MAX_MS).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "vad_retro_grace_ms {value} ms out of range {MIC_VAD_RETRO_GRACE_MIN_MS}..={MIC_VAD_RETRO_GRACE_MAX_MS}"
                        )));
                    }
                    mic.rnnoise.vad_retro_grace_ms = value;
                    node_name = "mic_rnnoise";
                    control_name = "Retroactive VAD Grace (ms)";
                }
                MicParam::GateThreshold => {
                    if !(MIC_GATE_THRESHOLD_MIN..=MIC_GATE_THRESHOLD_MAX).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "gate_threshold {value} out of range {MIC_GATE_THRESHOLD_MIN}..={MIC_GATE_THRESHOLD_MAX}"
                        )));
                    }
                    mic.gate.threshold = value;
                    node_name = "mic_gate";
                    control_name = "Threshold";
                }
                MicParam::CompThresholdDb => {
                    if !(MIC_COMP_THRESHOLD_MIN_DB..=MIC_COMP_THRESHOLD_MAX_DB).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "comp_threshold_db {value} dB out of range {MIC_COMP_THRESHOLD_MIN_DB}..={MIC_COMP_THRESHOLD_MAX_DB}"
                        )));
                    }
                    mic.compressor.threshold_db = value;
                    node_name = "mic_compressor";
                    control_name = "Threshold level (dB)";
                }
                MicParam::CompRatio => {
                    if !(MIC_COMP_RATIO_MIN..=MIC_COMP_RATIO_MAX).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "comp_ratio {value} out of range {MIC_COMP_RATIO_MIN}..={MIC_COMP_RATIO_MAX}"
                        )));
                    }
                    mic.compressor.ratio = value;
                    node_name = "mic_compressor";
                    control_name = "Ratio (1:n)";
                }
                MicParam::CompMakeupDb => {
                    if !(MIC_COMP_MAKEUP_MIN_DB..=MIC_COMP_MAKEUP_MAX_DB).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "comp_makeup_db {value} dB out of range {MIC_COMP_MAKEUP_MIN_DB}..={MIC_COMP_MAKEUP_MAX_DB}"
                        )));
                    }
                    mic.compressor.makeup_db = value;
                    node_name = "mic_compressor";
                    control_name = "Makeup gain (dB)";
                }
            }
        }
        self.save_config()?;

        // Apply live: use MicBackend::apply_control.
        // For the gain stage, value needs to be converted to linear.
        let apply_value = if param == MicParam::GainDb {
            convert::db_to_linear(value)
        } else {
            value
        };
        {
            let profile = self.config.active()?.clone();
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, FsPluginProbe, spec);
            if let Err(e) = mic_be.apply_control(node_name, control_name, apply_value) {
                eprintln!("warning: mic_set_param apply_control failed (post-spawn race?): {e}");
            }
        }

        self.emit(Event::MicParamSet { param, value });
        Ok(())
    }

    /// Set a single mic EQ band live via `apply_control` (no restart).
    pub fn mic_set_eq_band(&mut self, band: usize, cfg: EqBandConfig) -> Result<(), EngineError> {
        // Validate band
        let eq_band = convert::eq_band_from_cfg(&cfg)?;
        // Mutate config
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            while profile.mic.eq.len() <= band {
                profile.mic.eq.push(EqBandConfig {
                    kind: "peaking".to_string(),
                    freq_hz: 1000.0,
                    q: 1.0,
                    gain_db: 0.0,
                });
            }
            profile.mic.eq[band] = cfg.clone();
        }
        self.save_config()?;

        // Apply live via apply_control (3 controls per band node).
        {
            let profile = self.config.active()?.clone();
            let spec = convert::mic_chain_spec(&profile.mic);
            let node_name = convert::mic_eq_band_node_name(band);
            let mut mic_be = MicBackend::new(&mut self.runner, FsPluginProbe, spec);
            if let Err(e) = mic_be.apply_control(&node_name, "Freq", eq_band.freq_hz) {
                eprintln!("warning: mic_set_eq_band Freq apply_control failed: {e}");
            }
            if let Err(e) = mic_be.apply_control(&node_name, "Q", eq_band.q) {
                eprintln!("warning: mic_set_eq_band Q apply_control failed: {e}");
            }
            if let Err(e) = mic_be.apply_control(&node_name, "Gain", eq_band.gain_db) {
                eprintln!("warning: mic_set_eq_band Gain apply_control failed: {e}");
            }
        }

        self.emit(Event::MicEqBandSet { band });
        Ok(())
    }

    /// Set (or clear) the hardware mic capture target.
    /// Capture target change → full recreate.
    pub fn mic_set_hw_mic(&mut self, hw_mic: Option<String>) -> Result<(), EngineError> {
        // Mutate config
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            profile.mic.hw_mic = hw_mic;
        }
        self.save_config()?;

        // Recreate
        let hw_mic_snapshot = {
            let profile = self.config.active()?.clone();
            let (nodes, availability) = convert::mic_chain_nodes(&profile.mic, self.probe.as_ref());
            self.mic_availability = availability;
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, FsPluginProbe, spec);
            let handle = mic_be.recreate(&nodes)?;
            if let Some(token) = handle.child {
                self.children.track(token);
            }
            profile.mic.hw_mic.clone()
        };
        self.emit(Event::MicHwMicSet {
            hw_mic: hw_mic_snapshot,
        });
        Ok(())
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
    use arctis_config::{ChannelConfig, Config, MicChainConfig, Profile, RouteConfig};

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
                mic: MicChainConfig::default(),
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
                mic: MicChainConfig::default(),
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
            .with_output(0, "", "")
            // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
            .with_output(0, &ls, "");

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

        // Total: 3 (up) + 3*(1+10) (apply) + 1 (mic step5 source_exists) = 37
        assert_eq!(calls.len(), 37, "expected 37 total pw-cli calls");

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
            .with_output(0, "", "") // media 10 band sets
            // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
            .with_output(0, &ls_absent, "");

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

        // Total: 3 (phase1 ls) + 3*11 (apply) + 1 (mic step5 source_exists) = 37
        assert_eq!(
            calls.len(),
            37,
            "expected 37 total run calls (no spawns in calls)"
        );

        // spawn_owned goes into `spawned` — 3 pipewire -c invocations (channels only)
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
            .with_output(0, "", "")
            // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
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
                mic: MicChainConfig::default(),
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
            .with_output(0, "", "")
            // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
            .with_output(0, &ls_absent, "");

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

    /// Regression test: post-spawn apply_all must be NON-FATAL when find_node_id fails.
    ///
    /// Simulates the real-PipeWire timing race: channels are freshly spawned (ls absent
    /// in Phase 1), but in Phase 2 the node has not registered yet (ls still absent).
    /// apply_all returns a Parse error; reconcile must return Ok (EQ is conf-baked).
    #[test]
    fn reconcile_ok_when_post_spawn_apply_all_node_not_yet_registered() {
        let ls_absent = ls_all_absent();

        // Phase 1: 3 ls-absent → spawn_owned per channel (tokens tracked)
        // Phase 2: 3 ls-absent for find_node_id → apply_all errors → non-fatal warn
        // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
        let runner = MockRunner::new()
            // Phase 1 ls checks (absent → spawns)
            .with_output(0, &ls_absent, "")
            .with_output(0, &ls_absent, "")
            .with_output(0, &ls_absent, "")
            // Phase 2: find_node_id for each channel — node absent → Parse error
            .with_output(0, &ls_absent, "") // game
            .with_output(0, &ls_absent, "") // chat
            .with_output(0, &ls_absent, "") // media
            // Phase 5: mic source_exists check
            .with_output(0, &ls_absent, "");

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);

        // Must return Ok — the Parse error from find_node_id is non-fatal
        let result = engine.reconcile();
        assert!(
            result.is_ok(),
            "reconcile must return Ok when post-spawn apply_all fails with node-not-yet-registered: {result:?}"
        );

        // 3 channels spawned
        assert_eq!(
            engine.runner.spawned.len(),
            3,
            "3 pipewire instances spawned"
        );
        // 3 tracked tokens
        assert_eq!(engine.children.len(), 3, "3 child tokens tracked");

        // 7 run calls total: 3 (phase1 ls) + 3 (phase2 find_node_id) + 1 (mic step5 source_exists)
        assert_eq!(
            engine.runner.calls.len(),
            7,
            "expected 7 run calls: 3 ls-up + 3 ls-find-node + 1 mic source_exists"
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
            // Phase 5: mic disabled → remove() → source_exists() → 1 ls
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
    /// no-EQ, no-routes config where all sinks are already present AND mic disabled.
    ///
    /// Step 5 (mic): mic disabled → MicBackend::remove() → source_exists() → 1 ls Node
    /// returning the "no mic" output (source absent, remove returns immediately).
    fn queue_reconcile_present(runner: MockRunner) -> MockRunner {
        let ls = ls_all_present();
        let ls_no_mic = ls_all_present(); // "present" for channels but no mic node
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
        // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node found)
        r = r.with_output(0, &ls_no_mic, "");
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
            mic: MicChainConfig::default(),
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

    // ─────────────────────────────────────────────
    // TDD: new features — get-state full EQ, set_channel_output, new_profile
    // ─────────────────────────────────────────────

    /// Config with EQ bands set on the game channel.
    fn make_config_with_eq_bands() -> Config {
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
                        eq: vec![
                            arctis_config::EqBandConfig {
                                kind: "peaking".into(),
                                freq_hz: 100.0,
                                q: 1.0,
                                gain_db: 3.0,
                            },
                            arctis_config::EqBandConfig {
                                kind: "highshelf".into(),
                                freq_hz: 8000.0,
                                q: 0.7,
                                gain_db: -2.0,
                            },
                        ],
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: Some("alsa_output.headphones".into()),
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
                mic: MicChainConfig::default(),
            }],
        }
    }

    #[test]
    fn state_includes_full_eq_band_values_from_config() {
        let cfg = make_config_with_eq_bands();
        let engine = Engine::new(MockRunner::new(), cfg);
        let s = engine.state();

        // Find game channel
        let game = s
            .channels
            .iter()
            .find(|c| c.id == "game")
            .expect("game channel");
        assert_eq!(game.eq_bands.len(), 2, "game should have 2 EQ bands");

        // Verify band values come from config (not just a count)
        let b0 = &game.eq_bands[0];
        assert_eq!(b0.kind, "peaking");
        assert!((b0.freq_hz - 100.0).abs() < f32::EPSILON, "band0 freq_hz");
        assert!((b0.q - 1.0).abs() < f32::EPSILON, "band0 q");
        assert!((b0.gain_db - 3.0).abs() < f32::EPSILON, "band0 gain_db");

        let b1 = &game.eq_bands[1];
        assert_eq!(b1.kind, "highshelf");
        assert!((b1.freq_hz - 8000.0).abs() < f32::EPSILON, "band1 freq_hz");
        assert!((b1.q - 0.7).abs() < f32::EPSILON, "band1 q");
        assert!((b1.gain_db - -2.0).abs() < f32::EPSILON, "band1 gain_db");

        // Chat channel: output_device present, empty eq
        let chat = s
            .channels
            .iter()
            .find(|c| c.id == "chat")
            .expect("chat channel");
        assert_eq!(chat.output_device, Some("alsa_output.headphones".into()));
        assert!(chat.eq_bands.is_empty(), "chat has no configured EQ");
    }

    #[test]
    fn state_channel_snapshot_has_output_device() {
        let cfg = make_config_with_eq_bands();
        let engine = Engine::new(MockRunner::new(), cfg);
        let s = engine.state();
        let chat = s.channels.iter().find(|c| c.id == "chat").unwrap();
        assert_eq!(chat.output_device, Some("alsa_output.headphones".into()));
        let game = s.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(game.output_device, None);
    }

    #[test]
    fn set_channel_output_updates_config_persists_and_emits_event() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("set_ch_out");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();

        // set_channel_output calls ChannelManager::set_output which does:
        //   1. ls Node (find existing handle) + possibly spawn
        // Queue a present ls so set_output succeeds without spawn.
        let ls = ls_all_present();
        // set_output: ls Node to find channel + attempt to set output device
        // ChannelManager::set_output: ls to find node_id, then up + maybe spawn
        // When sinks are present, set_output does: ls (find) → present → no new spawn
        // But it re-creates the channel with new output, which means: ls (exists?) + spawn_owned
        // For simplicity, queue enough outputs so the operation can complete
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // ls for set_output
            .with_output(0, &ls, ""); // extra ls if needed

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        engine
            .set_channel_output("game", Some("alsa_output.speakers".to_string()))
            .expect("set_channel_output should succeed");

        // In-memory config updated
        let active = engine.config().active().unwrap();
        let game_ch = active.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(
            game_ch.output_device,
            Some("alsa_output.speakers".to_string()),
            "in-memory output_device must be updated"
        );

        // Config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("alsa_output.speakers"),
            "persisted config must contain the new output device"
        );

        // Event emitted
        let event = rx.try_recv().expect("ChannelOutputSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::ChannelOutputSet {
                channel_id: "game".to_string(),
                device: Some("alsa_output.speakers".to_string()),
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_channel_output_none_clears_device() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("set_ch_out_none");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Start with a channel that HAS an output device
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].channels[0].output_device = Some("alsa_output.old".into());

        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, &ls, "")
            .with_output(0, &ls, "");

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        engine
            .set_channel_output("game", None)
            .expect("set_channel_output(None) should succeed");

        let active = engine.config().active().unwrap();
        let game_ch = active.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(game_ch.output_device, None, "output_device must be cleared");

        let event = rx.try_recv().expect("ChannelOutputSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::ChannelOutputSet {
                channel_id: "game".to_string(),
                device: None,
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_channel_output_unknown_channel_errors() {
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let result = engine.set_channel_output("nonexistent", Some("some_device".into()));
        assert!(result.is_err(), "unknown channel_id must return an error");
    }

    #[test]
    fn new_profile_creates_clones_active_persists_reconciles_emits_event() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("new_profile");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        // Queue outputs for one reconcile pass (new_profile calls reconcile)
        let runner = queue_reconcile_present(MockRunner::new());

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        engine
            .new_profile("competitive")
            .expect("new_profile should succeed");

        // New profile created and active
        assert_eq!(engine.config().active_profile, "competitive");
        let names = engine.config().profile_names();
        assert!(
            names.contains(&"default".to_string()),
            "original profile preserved"
        );
        assert!(
            names.contains(&"competitive".to_string()),
            "new profile exists"
        );

        // Config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("competitive"),
            "persisted config must contain new profile name"
        );

        // Reconcile was called (pw-cli ls Node issued)
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c == &vec!["pw-cli", "ls", "Node"]),
            "reconcile must be called after new_profile"
        );

        // Event emitted
        let event = rx.try_recv().expect("ProfileCreated event must be sent");
        assert_eq!(
            event,
            crate::state::Event::ProfileCreated {
                name: "competitive".to_string()
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn new_profile_duplicate_name_errors_no_disk_write() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("new_profile_dup");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        let result = engine.new_profile("default"); // "default" already exists
        assert!(result.is_err(), "duplicate profile name must error");
        assert!(!tmp.exists(), "no disk write on error");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    // ─────────────────────────────────────────────
    // TDD: Task 6 — engine.device_set
    // ─────────────────────────────────────────────

    #[test]
    fn device_set_errors_when_worker_not_wired() {
        let cfg = make_config_no_eq_no_routes();
        let engine = Engine::new(MockRunner::new(), cfg);
        // device_tx is None — must return BadRequest
        let result = engine.device_set("sidetone", 2);
        assert!(
            matches!(result, Err(EngineError::BadRequest(_))),
            "must error with BadRequest when worker not running: {result:?}"
        );
    }

    #[test]
    fn device_set_returns_gated_error_when_control_not_enabled() {
        // Wire a fake worker channel backed by a receiver that always replies Err (gate refused).
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<crate::device::DeviceCommand>();
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        engine.set_device_tx(cmd_tx);

        // Spawn a fake worker that drains commands and sends back a gate-refused error.
        let worker = std::thread::spawn(move || {
            while let Ok(crate::device::DeviceCommand::Set { reply, .. }) = cmd_rx.recv() {
                let _ = reply.send(Err(
                    "sidetone is not enabled (no validated OWNER-RUN gate)".into()
                ));
            }
        });

        let result = engine.device_set("sidetone", 2);
        assert!(
            matches!(result, Err(EngineError::Device(_))),
            "gate-refused reply must surface as EngineError::Device: {result:?}"
        );
        if let Err(EngineError::Device(msg)) = result {
            assert!(
                msg.contains("not enabled") || msg.contains("OWNER-RUN"),
                "error message must mention the gate: {msg}"
            );
        }

        // Drop engine (which drops the cmd_tx) to let the worker finish.
        drop(engine);
        worker.join().expect("fake worker must not panic");
    }

    #[test]
    fn device_set_returns_ok_when_worker_accepts() {
        // Wire a fake worker channel that always replies Ok(()).
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<crate::device::DeviceCommand>();
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        engine.set_device_tx(cmd_tx);

        let worker = std::thread::spawn(move || {
            while let Ok(crate::device::DeviceCommand::Set { name, value, reply }) = cmd_rx.recv() {
                assert_eq!(name, "sidetone", "worker received correct control name");
                assert_eq!(value, 2, "worker received correct value");
                let _ = reply.send(Ok(()));
            }
        });

        let result = engine.device_set("sidetone", 2);
        assert!(
            result.is_ok(),
            "worker-accepted write must return Ok: {result:?}"
        );

        drop(engine);
        worker.join().expect("fake worker must not panic");
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

    // ─────────────────────────────────────────────
    // Task 4 TDD: mic source reconcile + engine methods
    // ─────────────────────────────────────────────

    use crate::engine::MicParam;
    use arctis_audio::MockPluginProbe;

    /// LS Node output containing the arctis_clean_mic source.
    fn ls_with_mic() -> String {
        [
            "id 40, type PipeWire:Interface:Node/3\n    node.name = \"alsa_output.pci\"\n",
            "id 71, type PipeWire:Interface:Node/3\n    node.name = \"arctis_clean_mic\"\n",
            "id 72, type PipeWire:Interface:Node/3\n    node.name = \"arctis_clean_mic.capture\"\n",
        ]
        .concat()
    }

    /// LS Node output with NO mic node.
    fn ls_without_mic() -> String {
        "id 40, type PipeWire:Interface:Node/3\n    node.name = \"alsa_output.pci\"\n".to_string()
    }

    /// Build a MicChainConfig with master switch enabled but all stages off (clean passthrough).
    fn mic_enabled_passthrough() -> arctis_config::MicChainConfig {
        arctis_config::MicChainConfig {
            enabled: true,
            hw_mic: Some("alsa_input.hw_mic".to_string()),
            ..Default::default()
        }
    }

    /// Build a 3-channel config with mic enabled passthrough (no stages).
    fn make_config_mic_enabled() -> Config {
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
                mic: mic_enabled_passthrough(),
            }],
        }
    }

    /// Queue outputs for a 3-channel reconcile with mic DISABLED.
    fn queue_reconcile_with_mic_disabled(runner: MockRunner) -> MockRunner {
        queue_reconcile_present(runner)
    }

    /// Queue outputs for a 3-channel reconcile with mic ENABLED and source absent.
    /// Step5: create() → source_exists() (1 ls, absent) → spawn (goes to spawned)
    fn queue_reconcile_with_mic_enabled_absent(runner: MockRunner) -> MockRunner {
        let ls = ls_all_present();
        let ls_mic_absent = ls_without_mic();
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
        // Phase 5: mic enabled → create() → source_exists() (absent) → spawn
        r = r.with_output(0, &ls_mic_absent, "");
        r
    }

    /// Test 1: reconcile with mic disabled calls MicBackend::remove() (source_exists only, no spawn).
    #[test]
    fn reconcile_passthrough_when_mic_disabled_removes_source() {
        let ls = ls_all_present();
        let ls_no_mic = ls_without_mic();

        let runner = queue_reconcile_with_mic_disabled(MockRunner::new());
        let cfg = make_config_no_eq_no_routes(); // mic.enabled = false by default
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.reconcile().expect("reconcile should succeed");

        // Verify step5 ran: last call is ls Node (source_exists check by remove())
        let calls = &engine.runner.calls;
        let last = calls.last().expect("at least one call");
        assert_eq!(
            last,
            &vec!["pw-cli", "ls", "Node"],
            "last call must be mic source_exists ls"
        );

        // No mic spawn (mic disabled → remove path, source absent → no destroy either)
        // spawned is only channel sinks (none in this case since sinks are "present")
        assert!(
            engine.runner.spawned.is_empty(),
            "no spawns when mic is disabled and channels are present"
        );

        let _ = &ls;
        let _ = &ls_no_mic; // suppress warnings
    }

    /// Test 2: reconcile with mic enabled (passthrough) → spawns arctis_clean_mic.conf.
    #[test]
    fn reconcile_builds_clean_mic_when_enabled() {
        let ls_mic_absent = ls_without_mic();

        // Queue outputs for mic-enabled reconcile
        let runner = queue_reconcile_with_mic_enabled_absent(MockRunner::new());
        let cfg = make_config_mic_enabled();
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));

        // Count spawns before reconcile (channels absent → 3 channel spawns if absent)
        // Since channels ARE "present" (queue_reconcile_with_mic_enabled_absent uses ls_all_present),
        // only the mic spawn occurs.
        engine.reconcile().expect("reconcile should succeed");

        let spawned = &engine.runner.spawned;
        // The mic source should be spawned
        assert!(
            spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "expected a spawn of arctis_clean_mic.conf, got: {spawned:?}"
        );
        // children tracks exactly the mic token (channels were already present → no channel tokens)
        assert_eq!(engine.children.len(), 1, "only the mic source token");

        let _ = &ls_mic_absent;
    }

    /// Test 3: rnnoise enabled but probe returns none → spawn still fires (chain minus rnnoise);
    ///         state marks rnnoise unavailable.
    #[test]
    fn reconcile_drops_unavailable_rnnoise_but_still_builds() {
        let mut cfg = make_config_mic_enabled();
        // Enable rnnoise in config
        cfg.profiles[0].mic.rnnoise.enabled = true;

        // Queue for mic-enabled absent reconcile (rnnoise probe is none → dropped from chain)
        let runner = queue_reconcile_with_mic_enabled_absent(MockRunner::new());
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.reconcile().expect("reconcile should succeed");

        // Spawn still fires (chain has passthrough fallback)
        let spawned = &engine.runner.spawned;
        assert!(
            spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "spawn must fire even when rnnoise is unavailable"
        );

        // state reports rnnoise unavailable
        let state = engine.state();
        let rnnoise_stage = state
            .mic
            .stages
            .iter()
            .find(|s| s.kind == crate::state::StageName::Rnnoise);
        let s = rnnoise_stage.expect("rnnoise must appear in stages");
        assert!(!s.available, "rnnoise must be marked unavailable");
        assert!(s.enabled, "rnnoise must show as enabled in config");
    }

    /// Test 4: mic_set_stage_enabled → persists config + recreates (remove + spawn observed).
    #[test]
    fn mic_set_stage_enabled_recreates_and_persists() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_stage_set");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_mic_enabled();
        cfg.profiles[0].mic.gain.enabled = false; // start with gain OFF

        // Queue for mic_set_stage_enabled(Gain, true):
        //   recreate() = remove() + create()
        //   remove(): source_exists() (1 ls, WITH mic) + find_node_id (1 ls) + destroy (1) + pkill (1) = 4 calls
        //   create(): source_exists() (1 ls, absent after remove) + spawn = 1 call
        let ls_with_mic = ls_with_mic();
        let ls_absent = ls_without_mic();

        let runner = MockRunner::new()
            // remove: source_exists (present)
            .with_output(0, &ls_with_mic, "")
            // remove: find_node_id
            .with_output(0, &ls_with_mic, "")
            // remove: pw-cli destroy 71
            .with_output(0, "", "")
            // remove: pkill -f <conf>
            .with_output(0, "", "")
            // create: source_exists (absent after remove)
            .with_output(0, &ls_absent, "");

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_stage_enabled(StageKind::Gain, true)
            .expect("mic_set_stage_enabled should succeed");

        // Config persisted to disk
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        // After reload, gain should be enabled (check raw TOML string)
        assert!(
            saved_str.contains("enabled = true"),
            "persisted config must show gain enabled (enabled = true present)"
        );

        // remove was called (destroy present) and create spawned new conf
        let calls = &engine.runner.calls;
        assert!(
            calls
                .iter()
                .any(|c| c.len() >= 3 && c[0] == "pw-cli" && c[1] == "destroy"),
            "destroy must be called during recreate"
        );
        let spawned = &engine.runner.spawned;
        assert!(
            spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "create must spawn arctis_clean_mic.conf after remove"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Test 5: mic_set_param emits exact pw-cli s <id> Props argv for VAD threshold.
    #[test]
    fn mic_set_param_emits_exact_props() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_param_set");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_mic_enabled();
        cfg.profiles[0].mic.rnnoise.enabled = true;
        cfg.profiles[0].mic.rnnoise.vad_threshold = 40.0;

        let ls_with_mic = ls_with_mic();

        // mic_set_param(VadThreshold, 55.0):
        //   apply_control("mic_rnnoise", "VAD Threshold (%)", 55.0)
        //   → find_node_id (1 ls) + pw-cli s 71 Props … (1 call)
        let runner = MockRunner::new()
            .with_output(0, &ls_with_mic, "") // find_node_id
            .with_output(0, "", ""); // set Props

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_param(MicParam::VadThreshold, 55.0)
            .expect("mic_set_param should succeed");

        let calls = &engine.runner.calls;
        // Last call should be: pw-cli s 71 Props { params = [ "mic_rnnoise:VAD Threshold (%)" 55.0 ] }
        let last = calls.last().expect("at least one call");
        assert_eq!(last[0], "pw-cli");
        assert_eq!(last[1], "s");
        assert_eq!(last[2], "71", "node id must be 71 (from ls fixture)");
        assert_eq!(last[3], "Props");
        assert_eq!(
            last[4], "{ params = [ \"mic_rnnoise:VAD Threshold (%)\" 55.0 ] }",
            "Props JSON must exactly match"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Test 6: shutdown kills the mic source token too.
    #[test]
    fn shutdown_kills_mic_source_too() {
        let ls_absent = ls_without_mic();

        // Reconcile with mic enabled and absent → spawn (mic token tracked in children)
        let runner = queue_reconcile_with_mic_enabled_absent(MockRunner::new());
        let cfg = make_config_mic_enabled();
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.reconcile().expect("reconcile should succeed");

        // channels are present so only the mic spawn occurs
        let initial_children = engine.children.len();
        assert!(initial_children >= 1, "at least mic child tracked");

        let initial_spawned = engine.runner.spawned.len();
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "mic must have been spawned"
        );

        // Shutdown kills all including the mic token
        engine.shutdown().expect("shutdown should succeed");

        assert_eq!(
            engine.runner.killed.len(),
            initial_spawned,
            "shutdown must kill all spawned processes including mic source"
        );
        assert_eq!(
            engine.children.len(),
            0,
            "children must be empty after shutdown"
        );

        let _ = &ls_absent;
    }

    /// Test 7: state().mic.stages reports rnnoise unavailable when probe is missing.
    #[test]
    fn state_reports_mic_stage_availability() {
        let mut cfg = make_config_mic_enabled();
        cfg.profiles[0].mic.rnnoise.enabled = true; // request rnnoise

        // Queue for reconcile with mic enabled, rnnoise probe absent → rnnoise dropped
        let runner = queue_reconcile_with_mic_enabled_absent(MockRunner::new());
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.reconcile().expect("reconcile should succeed");

        let state = engine.state();
        assert!(state.mic.enabled, "mic must show enabled");

        let rnnoise = state
            .mic
            .stages
            .iter()
            .find(|s| s.kind == crate::state::StageName::Rnnoise)
            .expect("rnnoise stage must be in state");

        assert!(rnnoise.enabled, "rnnoise enabled in config");
        assert!(
            !rnnoise.available,
            "rnnoise must be unavailable (probe returns false)"
        );
    }
}
