use crate::{children::ChildOwner, convert, error::EngineError, state::Event};
use arctis_audio::{
    move_stream_argv, query_pw_version, supports_builtin_noisegate, AppMatch, AudioBackend,
    ChannelManager, CommandRunner, EqModel, FsPluginProbe, MicBackend, PluginProbe, RouteRule,
    Router, StageKind, SurroundBackend, DEEPFILTER_PLUGIN_BASENAME, RNNOISE_PLUGIN_BASENAME,
};
use arctis_config::{Config, EqBandConfig};
use arctis_domain::{
    CHANNEL_VOLUME_MAX_DB, CHANNEL_VOLUME_MIN_DB, MIC_ATTEN_LIMIT_MAX_DB, MIC_ATTEN_LIMIT_MIN_DB,
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

/// Full attenuation applied to the losing side of the ChatMix dial (dB).
const CHATMIX_FULL_ATTEN_DB: f32 = -40.0;

/// Map a ChatMix position 0..=9 to (game_db, chat_db) attenuations.
/// center = 4.5 is the true midpoint of the 0..=9 range and matches the hardware
/// Nova Pro dial mapping in `crates/cli/src/dial.rs`, so the GUI ChatMix slider and
/// the physical dial behave identically. Because the center is 4.5, no integer
/// position is exactly neutral: position 4 leans slightly toward chat (game ~-4.4 dB)
/// and position 5 slightly toward game; true balance sits between 4 and 5.
/// Endpoints: 9 => full game (chat fully attenuated), 0 => full chat (game attenuated).
fn chatmix_to_volumes(position: i64) -> (f32, f32) {
    let p = position.clamp(0, 9) as f32;
    let center = 4.5_f32;
    if (p - center).abs() < f32::EPSILON {
        return (0.0, 0.0);
    }
    if p > center {
        // bias toward game: attenuate chat proportionally
        let t = (p - center) / (9.0 - center); // 0..1
        (0.0, CHATMIX_FULL_ATTEN_DB * t)
    } else {
        let t = (center - p) / center; // 0..1
        (CHATMIX_FULL_ATTEN_DB * t, 0.0)
    }
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
    /// Whether PipeWire's builtin noisegate is available (PW ≥ 1.6).
    builtin_noisegate: bool,
    /// Cached PipeWire version (queried once, then cached).
    pw_version: Option<(u32, u32, u32)>,
    /// Tracks which channel IDs are currently routed to the surround node.
    /// Used by apply_surround to restore channels removed from the surround list (C1)
    /// and channels whose output_device is None when surround is disabled (C2).
    surround_routed: std::collections::HashSet<String>,
}

impl<R: CommandRunner> Engine<R> {
    pub fn new(runner: R, mut config: Config) -> Self {
        config.ensure_standard_channels();
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
            builtin_noisegate: false,
            pw_version: None,
            surround_routed: std::collections::HashSet::new(),
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
            builtin_noisegate: false,
            pw_version: None,
            surround_routed: std::collections::HashSet::new(),
        }
    }

    /// Pre-seed the PipeWire version so `ensure_pw_version()` is a no-op during tests.
    /// Avoids adding an extra runner call that exact-call-count tests don't expect.
    #[cfg(test)]
    pub fn seed_pw_version(&mut self, version: (u32, u32, u32)) {
        self.pw_version = Some(version);
        self.builtin_noisegate = supports_builtin_noisegate(version);
    }

    /// Query (once) and cache the PipeWire version. Sets `builtin_noisegate` based on version.
    fn ensure_pw_version(&mut self) {
        if self.pw_version.is_none() {
            self.pw_version = query_pw_version(&mut self.runner);
            self.builtin_noisegate =
                supports_builtin_noisegate(self.pw_version.unwrap_or((0, 0, 0)));
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
            ChannelSnapshot, EngineState, EqBandSnapshot, EqPresetSnapshot, MicSnapshot,
            MicStageSnapshot, StageName, SuppressionBackend,
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
                        volume_db: ch.volume_db,
                        muted: ch.muted,
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
                    let avail = avail_map
                        .get(&StageName::Suppression)
                        .copied()
                        .unwrap_or(false);
                    let mut params = std::collections::BTreeMap::new();
                    if mc.suppression.enabled {
                        // Include all params — harmless to include both backends' params
                        params.insert(
                            "attenuation_limit_db".to_string(),
                            mc.suppression.attenuation_limit_db,
                        );
                        params.insert("vad_threshold".to_string(), mc.suppression.vad_threshold);
                        params.insert("vad_grace_ms".to_string(), mc.suppression.vad_grace_ms);
                        params.insert(
                            "vad_retro_grace_ms".to_string(),
                            mc.suppression.vad_retro_grace_ms,
                        );
                    }
                    MicStageSnapshot {
                        kind: StageName::Suppression,
                        enabled: mc.suppression.enabled,
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
                    let avail = avail_map.get(&StageName::Gate).copied().unwrap_or(false);
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

            // Map config SuppressionBackend → state SuppressionBackend
            let suppression_backend = match mc.suppression.backend {
                arctis_config::SuppressionBackend::DeepFilter => SuppressionBackend::DeepFilter,
                arctis_config::SuppressionBackend::Rnnoise => SuppressionBackend::Rnnoise,
            };

            // Report which backends' plugins are available
            let available_suppression_backends: Vec<SuppressionBackend> = {
                let mut backends = Vec::new();
                if self.probe.ladspa_available(DEEPFILTER_PLUGIN_BASENAME) {
                    backends.push(SuppressionBackend::DeepFilter);
                }
                if self.probe.ladspa_available(RNNOISE_PLUGIN_BASENAME) {
                    backends.push(SuppressionBackend::Rnnoise);
                }
                backends
            };

            MicSnapshot {
                enabled: mc.enabled,
                stages,
                eq_bands,
                suppression_backend,
                available_suppression_backends,
                hw_mic: mc.hw_mic.clone(),
            }
        } else {
            MicSnapshot::default()
        };

        let surround = if let Ok(p) = self.config.active() {
            let sc = &p.surround;
            crate::state::SurroundSnapshot {
                enabled: sc.enabled,
                hrir: sc.hrir.clone(),
                available_hrirs: convert::hrir_base_dir()
                    .map(|base| convert::available_hrirs(&base))
                    .unwrap_or_default(),
                channels: sc.channels.clone(),
                hw_sink: sc.hw_sink.clone(),
            }
        } else {
            crate::state::SurroundSnapshot::default()
        };

        let eq_presets = self
            .config
            .eq_presets
            .iter()
            .map(|p| EqPresetSnapshot {
                name: p.name.clone(),
                band_count: p.bands.len(),
            })
            .collect();

        EngineState {
            active_profile: self.config.active_profile.clone(),
            profiles: self.config.profile_names(),
            channels,
            routes,
            device_present: dev.present,
            device_fields: dev.fields,
            mic,
            surround,
            eq_presets,
            master_volume_db: active.as_ref().map(|p| p.master_volume_db).unwrap_or(0.0),
            master_mute: active.as_ref().map(|p| p.master_mute).unwrap_or(false),
            chatmix_position: active.as_ref().map(|p| p.chatmix_position).unwrap_or(4),
            default_sink_channel: active.as_ref().and_then(|p| p.default_sink_channel.clone()),
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

    /// Persist a route rule without doing a live move.
    ///
    /// Upserts the route in the active profile's in-memory config, writes the unified
    /// config to disk, and updates the WirePlumber persistent fragment via Router.
    /// Emits `RouteSet`. Does NOT call `apply_live`.
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
        // Write persistent WirePlumber fragment via Router (no live move)
        {
            let mut router = Router::new(&mut self.runner);
            router.set_rule(RouteRule::new(app_binary, target_sink));
            router.save_persistent()?;
        }
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
    /// Drops the rule from in-memory config + persists, removes the WirePlumber
    /// fragment entry, and attempts a best-effort live clear (moves the stream back
    /// to the default sink by deleting its `target.object` metadata key).
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
        // Update Router (remove from persistent fragment)
        {
            let mut router = Router::new(&mut self.runner);
            router.remove_rule(app_binary);
            router.save_persistent()?;
            // Best-effort live clear (ignore error if app not running)
            let _ = router.clear_live(&AppMatch::Binary(app_binary.to_string()));
        }
        // Emit event
        self.emit(Event::RouteCleared {
            app_binary: app_binary.to_string(),
        });
        Ok(())
    }

    /// Discover running application output streams, resolving each to a channel id
    /// (via its linked sink node.name) and flagging those with a persistent route.
    /// One `pw-dump` per call; pure mapping otherwise. Read-only (no graph mutation).
    pub fn list_streams(&mut self) -> Result<Vec<crate::state::AppStream>, EngineError> {
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
        Ok(parsed
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
            .collect())
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

        // Find the target stream (by node id string or by binary) for its id + binary.
        let streams = self.list_streams()?;
        let target = streams
            .iter()
            .find(|s| s.id.to_string() == stream || s.binary == stream)
            .ok_or_else(|| EngineError::BadRequest(format!("no running stream: {stream}")))?
            .clone();

        // Live move the exact stream node id.
        let argv = move_stream_argv(&target.id.to_string(), &sink)?;
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let out = self.runner.run("pw-metadata", &args)?;
        if out.status != 0 {
            return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                program: "pw-metadata".into(),
                status: out.status,
                stderr: out.stderr,
            }));
        }

        // Persist binary -> sink without a second live move.
        // The exact-id move above already targeted the specific instance; calling
        // set_route would also trigger a best-effort binary-match live move that
        // could silently target the wrong instance when multiple instances of the
        // same binary are running. persist_route writes config + WP fragment +
        // emits RouteSet without any additional pw-dump / pw-metadata calls.
        self.persist_route(&target.binary, &sink)?;
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

    /// Set the software volume for a single channel. Validates range, persists, applies live, emits.
    pub fn set_channel_volume(
        &mut self,
        channel_id: &str,
        volume_db: f32,
    ) -> Result<(), EngineError> {
        if !(CHANNEL_VOLUME_MIN_DB..=CHANNEL_VOLUME_MAX_DB).contains(&volume_db) {
            return Err(EngineError::BadRequest(format!(
                "volume_db {volume_db} out of range {CHANNEL_VOLUME_MIN_DB}..={CHANNEL_VOLUME_MAX_DB}"
            )));
        }
        // Mutate config
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
            channel.volume_db = volume_db;
        }
        self.save_config()?;
        // Apply live
        {
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
            let mut be = AudioBackend::new(&mut self.runner, spec);
            be.apply_volume_mute(volume_db, channel.muted)?;
        }
        self.emit(Event::ChannelVolumeSet {
            channel_id: channel_id.to_string(),
            volume_db,
        });
        Ok(())
    }

    /// Set the mute state for a single channel. Persists, applies live, emits.
    pub fn set_channel_mute(&mut self, channel_id: &str, muted: bool) -> Result<(), EngineError> {
        // Mutate config
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
            channel.muted = muted;
        }
        self.save_config()?;
        // Apply live
        {
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
            let mut be = AudioBackend::new(&mut self.runner, spec);
            be.apply_volume_mute(channel.volume_db, muted)?;
        }
        self.emit(Event::ChannelMuteSet {
            channel_id: channel_id.to_string(),
            muted,
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

    /// Rename a profile. If it was the active profile, also updates `active_profile`.
    /// Saves config and emits `ProfileRenamed`.
    pub fn rename_profile(&mut self, old: &str, new: &str) -> Result<(), EngineError> {
        // Delegate validation to config layer
        self.config.rename_profile(old, new)?;
        // If the renamed profile was active, keep active_profile in sync
        if self.config.active_profile == old {
            self.config.active_profile = new.to_string();
        }
        self.save_config()?;
        self.emit(Event::ProfileRenamed {
            old: old.to_string(),
            new: new.to_string(),
        });
        Ok(())
    }

    /// Delete a profile. Saves config and emits `ProfileDeleted`.
    pub fn delete_profile(&mut self, name: &str) -> Result<(), EngineError> {
        self.config.delete_profile(name)?;
        self.save_config()?;
        self.emit(Event::ProfileDeleted {
            name: name.to_string(),
        });
        Ok(())
    }

    /// Export a profile by name as a TOML string. Read-only — no persist, no event.
    pub fn export_profile(&self, name: &str) -> Result<String, EngineError> {
        let profile = self
            .config
            .profile(name)
            .ok_or_else(|| EngineError::BadRequest(format!("profile not found: {name}")))?;
        toml::to_string(profile)
            .map_err(|e| EngineError::BadRequest(format!("serialize profile: {e}")))
    }

    /// Import a profile from a TOML string. Resolves name collisions by appending
    /// "-imported", then "-imported(2)", "-imported(3)", etc. until unique.
    /// Validates the config after insertion. Returns the resolved name.
    pub fn import_profile(&mut self, toml_str: &str) -> Result<String, EngineError> {
        let mut profile: arctis_config::Profile = toml::from_str(toml_str)
            .map_err(|e| EngineError::BadRequest(format!("invalid profile TOML: {e}")))?;

        // Resolve name collision
        let base_name = profile.name.clone();
        let resolved_name = if self.config.profile(&base_name).is_none() {
            base_name.clone()
        } else {
            let candidate = format!("{base_name}-imported");
            if self.config.profile(&candidate).is_none() {
                candidate
            } else {
                let mut n = 2u32;
                loop {
                    if n > 1000 {
                        return Err(EngineError::BadRequest(
                            "too many name collisions for imported profile".to_string(),
                        ));
                    }
                    let candidate = format!("{base_name}-imported({n})");
                    if self.config.profile(&candidate).is_none() {
                        break candidate;
                    }
                    n += 1;
                }
            }
        };

        profile.name = resolved_name.clone();
        self.config.upsert_profile(profile);
        // Validate the config after insertion
        self.config.validate()?;
        self.save_config()?;
        self.emit(Event::ProfileImported {
            name: resolved_name.clone(),
        });
        Ok(resolved_name)
    }

    /// Save the current EQ bands of `channel_id` in the active profile as a named preset.
    /// Overwrites if a preset with that name already exists.
    pub fn save_eq_preset(&mut self, name: &str, channel_id: &str) -> Result<(), EngineError> {
        // Find channel in active profile
        let bands = {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            profile
                .channels
                .iter()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| EngineError::BadRequest(format!("channel not found: {channel_id}")))?
                .eq
                .clone()
        };

        let preset = arctis_config::EqPreset {
            name: name.to_string(),
            kind_hint: None,
            bands,
        };

        // Overwrite if name exists, otherwise push
        if let Some(existing) = self.config.eq_presets.iter_mut().find(|p| p.name == name) {
            *existing = preset;
        } else {
            self.config.eq_presets.push(preset);
        }

        self.save_config()?;
        self.emit(Event::EqPresetSaved {
            name: name.to_string(),
        });
        Ok(())
    }

    /// Apply a named preset's bands to `channel_id` in the active profile. Live-applies via AudioBackend.
    pub fn apply_eq_preset(&mut self, preset: &str, channel_id: &str) -> Result<(), EngineError> {
        // Find preset
        let preset_bands = self
            .config
            .eq_presets
            .iter()
            .find(|p| p.name == preset)
            .ok_or_else(|| EngineError::BadRequest(format!("EQ preset not found: {preset}")))?
            .bands
            .clone();

        // Mutate channel EQ in active profile
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
            channel.eq = preset_bands;
        }

        self.save_config()?;

        // Live-apply all bands (same pattern as reconcile step 2)
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
            let def = convert::channel_def_from_cfg(channel);
            let spec = def.sink_spec();
            let mut be = arctis_audio::AudioBackend::new(&mut self.runner, spec);
            if let Err(e) = be.apply_all(&eq_model) {
                eprintln!(
                    "warning: apply_eq_preset apply_all for channel '{channel_id}' failed (ignoring): {e}"
                );
            }
        }

        self.emit(Event::EqPresetApplied {
            name: preset.to_string(),
            channel_id: channel_id.to_string(),
        });
        Ok(())
    }

    /// Delete a named EQ preset. Errors if not found.
    pub fn delete_eq_preset(&mut self, name: &str) -> Result<(), EngineError> {
        let pos = self
            .config
            .eq_presets
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| EngineError::BadRequest(format!("EQ preset not found: {name}")))?;
        self.config.eq_presets.remove(pos);
        self.save_config()?;
        self.emit(Event::EqPresetDeleted {
            name: name.to_string(),
        });
        Ok(())
    }

    /// Add a new channel to the active profile with sane defaults.
    ///
    /// `id` must be non-empty, contain no whitespace or path separators, and not
    /// already exist in the active profile. `node_name` is derived as `"Arctis_<Title>"`
    /// and `description` as `"<id> audio channel"`. After adding to config and persisting,
    /// the new channel sink is brought up (reusing AudioBackend::create). Emits `ChannelAdded`.
    pub fn add_channel(&mut self, id: &str) -> Result<(), EngineError> {
        // Derive node_name and description from id
        let title = {
            let mut c = id.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        };
        let node_name = format!("Arctis_{title}");
        let description = format!("{id} audio channel");

        // Mutate config (validates id constraints)
        self.config
            .add_channel(id, &node_name, &description)
            .map_err(EngineError::Config)?;

        // Persist
        self.save_config()?;

        // Bring the new channel sink up (reuse channel-up path)
        {
            let profile = self.config.active()?.clone();
            let channel = profile
                .channels
                .iter()
                .find(|ch| ch.id == id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found after add: {id}"))
                })?;
            let eq_model = convert::eq_model_for(channel)?;
            let def = convert::channel_def_from_cfg(channel);
            let spec = def.sink_spec();
            let mut be = arctis_audio::AudioBackend::new(&mut self.runner, spec);
            match be.create(&eq_model) {
                Ok(handle) => {
                    if let Some(token) = handle.child {
                        self.children.track(token);
                    }
                }
                Err(e) => {
                    eprintln!(
                        "warning: add_channel create sink for '{id}' failed (post-spawn race?): {e}"
                    );
                }
            }
        }

        self.emit(Event::ChannelAdded { id: id.to_string() });
        Ok(())
    }

    /// Remove a channel from the active profile.
    ///
    /// Errors if the channel does not exist or if it is the last remaining channel.
    /// Any channel may be removed, including game/chat/media.
    /// Routes referencing the removed channel become inert (no automatic cleanup).
    ///
    /// Tears down the channel's PipeWire sink (reusing AudioBackend::remove).
    /// Emits `ChannelRemoved`.
    pub fn remove_channel(&mut self, id: &str) -> Result<(), EngineError> {
        // Snapshot the channel def before removal (needed for teardown)
        let channel_def = {
            let profile = self.config.active()?;
            let channel = profile
                .channels
                .iter()
                .find(|ch| ch.id == id)
                .ok_or_else(|| EngineError::BadRequest(format!("channel not found: {id}")))?;
            convert::channel_def_from_cfg(channel)
        };

        // Mutate config (validates last-channel guard)
        self.config
            .remove_channel(id)
            .map_err(EngineError::Config)?;

        // Prune the removed channel from surround.channels so config doesn't reference a
        // deleted channel. Do this after the last-channel guard passes (above).
        {
            let active_name = self.config.active_profile.clone();
            if let Some(profile) = self.config.profile_mut(&active_name) {
                profile.surround.channels.retain(|ch| ch != id);
            }
        }

        // Persist
        self.save_config()?;

        // Tear down the channel's PipeWire sink (reuse channel-down path)
        {
            let spec = channel_def.sink_spec();
            let mut be = arctis_audio::AudioBackend::new(&mut self.runner, spec);
            if let Err(e) = be.remove() {
                eprintln!(
                    "warning: remove_channel sink teardown for '{id}' failed (ignoring): {e}"
                );
            }
        }

        self.emit(Event::ChannelRemoved { id: id.to_string() });
        Ok(())
    }

    /// Set the master output gain (dB) on the headset output via wpctl, persist,
    /// and emit MasterVolumeSet.
    pub fn set_master_volume(&mut self, db: f32) -> Result<(), EngineError> {
        {
            let name = self.config.active_profile.clone();
            let p = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            p.master_volume_db = db;
        }
        self.save_config()?;
        // wpctl set-volume on @DEFAULT_AUDIO_SINK@ using a linear factor.
        let linear = 10f32.powf(db / 20.0);
        let factor = format!("{linear:.4}");
        let out = self
            .runner
            .run("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &factor])?;
        if out.status != 0 {
            return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                program: "wpctl".into(),
                status: out.status,
                stderr: out.stderr,
            }));
        }
        self.emit(Event::MasterVolumeSet { volume_db: db });
        Ok(())
    }

    /// Mute/unmute the master output via wpctl, persist, emit MasterMuteSet.
    pub fn set_master_mute(&mut self, muted: bool) -> Result<(), EngineError> {
        {
            let name = self.config.active_profile.clone();
            let p = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            p.master_mute = muted;
        }
        self.save_config()?;
        let arg = if muted { "1" } else { "0" };
        let out = self
            .runner
            .run("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", arg])?;
        if out.status != 0 {
            return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                program: "wpctl".into(),
                status: out.status,
                stderr: out.stderr,
            }));
        }
        self.emit(Event::MasterMuteSet { muted });
        Ok(())
    }

    /// Set ChatMix position (Game<->Chat balance); applies derived volumes to the
    /// game and chat channels, persists position, emits ChatmixSet.
    pub fn set_chatmix(&mut self, position: i64) -> Result<(), EngineError> {
        let pos = position.clamp(0, 9);
        {
            let name = self.config.active_profile.clone();
            let p = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            p.chatmix_position = pos;
        }
        let (game_db, chat_db) = chatmix_to_volumes(pos);
        // Reuse set_channel_volume (live + persist) for each side; ignore "channel
        // not found" so profiles lacking game/chat don't hard-fail.
        let _ = self.set_channel_volume("game", game_db);
        let _ = self.set_channel_volume("chat", chat_db);
        self.save_config()?;
        self.emit(Event::ChatmixSet { position: pos });
        Ok(())
    }

    /// Set (or clear) which channel's sink is the system default output. When set,
    /// runs `wpctl set-default` on that sink. Persists + emits.
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
            let out = self.runner.run("wpctl", &["set-default", &sink_name])?;
            if out.status != 0 {
                return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                    program: "wpctl".into(),
                    status: out.status,
                    stderr: out.stderr,
                }));
            }
        }
        self.emit(Event::DefaultSinkChannelSet { channel });
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
                if let Err(e) = be2.apply_volume_mute(ch.volume_db, ch.muted) {
                    eprintln!(
                        "warning: reconcile apply_volume_mute for channel '{}' failed (ignoring): {e}",
                        ch.id
                    );
                }
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
                StageKind::Suppression => profile.mic.suppression.enabled = on,
                StageKind::Compressor => profile.mic.compressor.enabled = on,
                StageKind::Gate => profile.mic.gate.enabled = on,
                StageKind::MicEq => profile.mic.eq_enabled = on,
            }
        }
        self.save_config()?;

        // Only perform live graph I/O when the master switch is on.
        // When off, the persisted config change takes effect the next time
        // mic_set_enabled(true) or reconcile() builds the chain.
        if self.config.active()?.mic.enabled {
            self.ensure_pw_version();
            let profile = self.config.active()?.clone();
            let (nodes, availability) =
                convert::mic_chain_nodes(&profile.mic, self.probe.as_ref(), self.builtin_noisegate);
            self.mic_availability = availability;
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
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
    /// `GateThreshold` triggers a full recreate (different control format between builtin/LADSPA).
    pub fn mic_set_param(&mut self, param: MicParam, value: f32) -> Result<(), EngineError> {
        let mut needs_recreate = false;
        let mut control_name_opt: Option<&'static str> = None;
        let mut node_name_opt: Option<&'static str> = None;

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
                    node_name_opt = Some("mic_gain");
                    control_name_opt = Some("Mult");
                }
                MicParam::HighpassFreq => {
                    if !(MIC_HIGHPASS_MIN_HZ..=MIC_HIGHPASS_MAX_HZ).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "highpass_freq {value} Hz out of range {MIC_HIGHPASS_MIN_HZ}..={MIC_HIGHPASS_MAX_HZ}"
                        )));
                    }
                    mic.highpass.freq_hz = value;
                    node_name_opt = Some("mic_highpass");
                    control_name_opt = Some("Freq");
                }
                MicParam::AttenuationLimitDb => {
                    if !(MIC_ATTEN_LIMIT_MIN_DB..=MIC_ATTEN_LIMIT_MAX_DB).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "attenuation_limit_db {value} out of range {MIC_ATTEN_LIMIT_MIN_DB}..={MIC_ATTEN_LIMIT_MAX_DB}"
                        )));
                    }
                    mic.suppression.attenuation_limit_db = value;
                    node_name_opt = Some("mic_suppression");
                    control_name_opt = Some("Attenuation Limit (dB)");
                }
                MicParam::VadThreshold => {
                    if !(MIC_VAD_THRESHOLD_MIN..=MIC_VAD_THRESHOLD_MAX).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "vad_threshold {value} out of range {MIC_VAD_THRESHOLD_MIN}..={MIC_VAD_THRESHOLD_MAX}"
                        )));
                    }
                    mic.suppression.vad_threshold = value;
                    node_name_opt = Some("mic_suppression");
                    control_name_opt = Some("VAD Threshold (%)");
                }
                MicParam::VadGraceMs => {
                    if !(MIC_VAD_GRACE_MIN_MS..=MIC_VAD_GRACE_MAX_MS).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "vad_grace_ms {value} ms out of range {MIC_VAD_GRACE_MIN_MS}..={MIC_VAD_GRACE_MAX_MS}"
                        )));
                    }
                    mic.suppression.vad_grace_ms = value;
                    node_name_opt = Some("mic_suppression");
                    control_name_opt = Some("VAD Grace Period (ms)");
                }
                MicParam::VadRetroGraceMs => {
                    if !(MIC_VAD_RETRO_GRACE_MIN_MS..=MIC_VAD_RETRO_GRACE_MAX_MS).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "vad_retro_grace_ms {value} ms out of range {MIC_VAD_RETRO_GRACE_MIN_MS}..={MIC_VAD_RETRO_GRACE_MAX_MS}"
                        )));
                    }
                    mic.suppression.vad_retro_grace_ms = value;
                    node_name_opt = Some("mic_suppression");
                    control_name_opt = Some("Retroactive VAD Grace (ms)");
                }
                MicParam::GateThreshold => {
                    if !(MIC_GATE_THRESHOLD_MIN..=MIC_GATE_THRESHOLD_MAX).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "gate_threshold {value} out of range {MIC_GATE_THRESHOLD_MIN}..={MIC_GATE_THRESHOLD_MAX}"
                        )));
                    }
                    mic.gate.threshold = value;
                    // Gate threshold changes the gate topology (different control name/units
                    // between builtin noisegate and LADSPA gate_1410) → recreate, not live Props.
                    needs_recreate = true;
                }
                MicParam::CompThresholdDb => {
                    if !(MIC_COMP_THRESHOLD_MIN_DB..=MIC_COMP_THRESHOLD_MAX_DB).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "comp_threshold_db {value} dB out of range {MIC_COMP_THRESHOLD_MIN_DB}..={MIC_COMP_THRESHOLD_MAX_DB}"
                        )));
                    }
                    mic.compressor.threshold_db = value;
                    node_name_opt = Some("mic_compressor");
                    control_name_opt = Some("Threshold level (dB)");
                }
                MicParam::CompRatio => {
                    if !(MIC_COMP_RATIO_MIN..=MIC_COMP_RATIO_MAX).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "comp_ratio {value} out of range {MIC_COMP_RATIO_MIN}..={MIC_COMP_RATIO_MAX}"
                        )));
                    }
                    mic.compressor.ratio = value;
                    node_name_opt = Some("mic_compressor");
                    control_name_opt = Some("Ratio (1:n)");
                }
                MicParam::CompMakeupDb => {
                    if !(MIC_COMP_MAKEUP_MIN_DB..=MIC_COMP_MAKEUP_MAX_DB).contains(&value) {
                        return Err(EngineError::BadRequest(format!(
                            "comp_makeup_db {value} dB out of range {MIC_COMP_MAKEUP_MIN_DB}..={MIC_COMP_MAKEUP_MAX_DB}"
                        )));
                    }
                    mic.compressor.makeup_db = value;
                    node_name_opt = Some("mic_compressor");
                    control_name_opt = Some("Makeup gain (dB)");
                }
            }
        }
        self.save_config()?;

        // Only perform live I/O when the master switch is on.
        if self.config.active()?.mic.enabled {
            if needs_recreate {
                self.ensure_pw_version();
                let profile = self.config.active()?.clone();
                let (nodes, availability) = convert::mic_chain_nodes(
                    &profile.mic,
                    self.probe.as_ref(),
                    self.builtin_noisegate,
                );
                self.mic_availability = availability;
                let spec = convert::mic_chain_spec(&profile.mic);
                let mut mic_be = MicBackend::new(&mut self.runner, spec);
                let handle = mic_be.recreate(&nodes)?;
                if let Some(token) = handle.child {
                    self.children.track(token);
                }
            } else if let (Some(node_name), Some(control_name)) = (node_name_opt, control_name_opt)
            {
                // For gain, convert to linear; all others pass raw value.
                let apply_value = if param == MicParam::GainDb {
                    convert::db_to_linear(value)
                } else {
                    value
                };
                let profile = self.config.active()?.clone();
                let spec = convert::mic_chain_spec(&profile.mic);
                let mut mic_be = MicBackend::new(&mut self.runner, spec);
                if let Err(e) = mic_be.apply_control(node_name, control_name, apply_value) {
                    eprintln!(
                        "warning: mic_set_param apply_control failed (post-spawn race?): {e}"
                    );
                }
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

        // Only perform live apply_control when the master switch is on.
        // When off, the persisted config change takes effect the next time
        // mic_set_enabled(true) or reconcile() builds the chain.
        if self.config.active()?.mic.enabled {
            let profile = self.config.active()?.clone();
            let spec = convert::mic_chain_spec(&profile.mic);
            let node_name = convert::mic_eq_band_node_name(band);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
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

    /// Enable or disable the whole mic chain (master switch). Builds the Clean Mic
    /// source when enabling, removes it when disabling. Persists + emits.
    pub fn mic_set_enabled(&mut self, on: bool) -> Result<(), EngineError> {
        // Mutate config
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            profile.mic.enabled = on;
        }
        self.save_config()?;

        // Apply: mirror reconcile step5 create/remove logic exactly.
        {
            self.ensure_pw_version();
            let profile = self.config.active()?.clone();
            let (nodes, availability) =
                convert::mic_chain_nodes(&profile.mic, self.probe.as_ref(), self.builtin_noisegate);
            self.mic_availability = availability;
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            if on {
                match mic_be.create(&nodes) {
                    Ok(handle) => {
                        if let Some(token) = handle.child {
                            self.children.track(token);
                        }
                    }
                    Err(e) => {
                        eprintln!("warning: mic_set_enabled create failed (post-spawn race?): {e}");
                    }
                }
            } else {
                if let Err(e) = mic_be.remove() {
                    eprintln!("warning: mic_set_enabled remove failed (ignoring): {e}");
                }
            }
        }

        self.emit(Event::MicEnabledSet { enabled: on });
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

        // Only perform live graph I/O (recreate) when the master switch is on.
        // When off, the persisted config change takes effect the next time
        // mic_set_enabled(true) or reconcile() builds the chain.
        let hw_mic_snapshot = self.config.active()?.mic.hw_mic.clone();
        if self.config.active()?.mic.enabled {
            self.ensure_pw_version();
            let profile = self.config.active()?.clone();
            let (nodes, availability) =
                convert::mic_chain_nodes(&profile.mic, self.probe.as_ref(), self.builtin_noisegate);
            self.mic_availability = availability;
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            let handle = mic_be.recreate(&nodes)?;
            if let Some(token) = handle.child {
                self.children.track(token);
            }
        }
        self.emit(Event::MicHwMicSet {
            hw_mic: hw_mic_snapshot,
        });
        Ok(())
    }

    /// Change the active noise-suppression backend. Triggers a full chain recreate when
    /// the master switch is on (topology change: different LADSPA plugin).
    pub fn mic_set_suppression_backend(
        &mut self,
        backend: crate::state::SuppressionBackend,
    ) -> Result<(), EngineError> {
        // Mutate config
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
            })?;
            profile.mic.suppression.backend = match backend {
                crate::state::SuppressionBackend::DeepFilter => {
                    arctis_config::SuppressionBackend::DeepFilter
                }
                crate::state::SuppressionBackend::Rnnoise => {
                    arctis_config::SuppressionBackend::Rnnoise
                }
            };
        }
        self.save_config()?;

        // Recreate the chain if the master switch is on (topology change).
        if self.config.active()?.mic.enabled {
            self.ensure_pw_version();
            let profile = self.config.active()?.clone();
            let (nodes, availability) =
                convert::mic_chain_nodes(&profile.mic, self.probe.as_ref(), self.builtin_noisegate);
            self.mic_availability = availability;
            let spec = convert::mic_chain_spec(&profile.mic);
            let mut mic_be = MicBackend::new(&mut self.runner, spec);
            let handle = mic_be.recreate(&nodes)?;
            if let Some(token) = handle.child {
                self.children.track(token);
            }
        }

        self.emit(Event::MicSuppressionBackendSet { backend });
        Ok(())
    }

    /// Apply the surround config to the live graph (self-correcting, non-thrashing).
    ///
    /// Tracks which channels are currently routed to the surround node via
    /// `self.surround_routed`. On each call:
    ///
    /// **Disabled path**: removes the surround sink (idempotent), then restores ALL
    ///   channels in `surround_routed` to their configured `output_device` (fixes C2:
    ///   channels with `output_device=None` were left with a stale pointer to the
    ///   destroyed node). Drains `surround_routed` on completion.
    ///
    /// **Enabled path**: recreates the surround sink (so HRIR/hw_sink changes take
    ///   effect), then computes:
    ///   - `to_restore`: channels previously routed to surround but no longer in the
    ///     desired set → restore to their configured `output_device` (fixes C1).
    ///   - `to_route`: channels in the desired set not yet tracked → route to surround.
    ///
    ///   Channels already in `surround_routed` ∩ `desired` are left untouched (no thrash).
    ///
    /// Warn-and-continue on transient errors (mirrors reconcile step pattern).
    fn apply_surround(
        &mut self,
        profile: &arctis_config::Profile,
    ) -> Result<(), crate::error::EngineError> {
        let sc = &profile.surround;
        let spec = convert::surround_spec(sc);

        if !sc.enabled {
            // Remove surround sink (idempotent).
            let mut surround_be = SurroundBackend::new(&mut self.runner, spec);
            if let Err(e) = surround_be.remove() {
                eprintln!("warning: apply_surround remove failed (ignoring): {e}");
            }
            // Restore ALL channels that were surround-routed (fixes C2).
            // Drain first so the set is empty regardless of errors below.
            let previously_routed: Vec<String> = self.surround_routed.drain().collect();
            if !previously_routed.is_empty() {
                let channel_set = convert::channel_set_from_profile(profile);
                for ch_id in &previously_routed {
                    if let Some(ch) = profile.channels.iter().find(|c| &c.id == ch_id) {
                        let eq_model = convert::eq_model_for(ch)?;
                        // M1: create ChannelManager per iteration; drop before track to release borrow.
                        let handle = {
                            let mut mgr =
                                ChannelManager::new(&mut self.runner, channel_set.clone());
                            mgr.set_output(ch_id, ch.output_device.clone(), &eq_model)
                        };
                        match handle {
                            Ok(h) => {
                                if let Some(t) = h.child {
                                    self.children.track(t);
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "warning: apply_surround restore channel '{ch_id}' failed (ignoring): {e}"
                                );
                            }
                        }
                    }
                }
            }
            return Ok(());
        }

        // Enabled path: resolve HRIR first.
        let hrir_path = match convert::hrir_base_dir()
            .and_then(|base| convert::resolve_hrir_path(sc, &base))
        {
            Ok(p) => p,
            Err(e) => {
                eprintln!("warning: apply_surround HRIR resolve failed (skipping surround): {e}");
                return Ok(());
            }
        };

        // Recreate surround sink (handles both first-time and HRIR/hw_sink change).
        {
            let mut surround_be = SurroundBackend::new(&mut self.runner, spec);
            match surround_be.recreate(&hrir_path) {
                Ok(handle) => {
                    if let Some(t) = handle.child {
                        self.children.track(t);
                    }
                }
                Err(e) => {
                    eprintln!("warning: apply_surround recreate failed (ignoring): {e}");
                }
            }
        }

        // Compute desired surround-channel set (from new config).
        let desired: std::collections::HashSet<String> = sc.channels.iter().cloned().collect();

        // Channels to restore: were surround-routed, no longer in desired set (fixes C1).
        let to_restore: Vec<String> = self
            .surround_routed
            .iter()
            .filter(|id| !desired.contains(*id))
            .cloned()
            .collect();

        // Channels to route to surround: in desired set, not already tracked (avoid thrash).
        let to_route: Vec<String> = desired
            .iter()
            .filter(|id| !self.surround_routed.contains(*id))
            .cloned()
            .collect();

        let channel_set = convert::channel_set_from_profile(profile);
        let surround_target = "effect_input.arctis_surround".to_string();

        // Restore removed channels to their configured output_device.
        for ch_id in &to_restore {
            if let Some(ch) = profile.channels.iter().find(|c| &c.id == ch_id) {
                let eq_model = convert::eq_model_for(ch)?;
                let handle = {
                    let mut mgr = ChannelManager::new(&mut self.runner, channel_set.clone());
                    mgr.set_output(ch_id, ch.output_device.clone(), &eq_model)
                };
                match handle {
                    Ok(h) => {
                        if let Some(t) = h.child {
                            self.children.track(t);
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "warning: apply_surround restore channel '{ch_id}' failed (ignoring): {e}"
                        );
                    }
                }
            }
            self.surround_routed.remove(ch_id);
        }

        // Route new channels to surround.
        for ch_id in &to_route {
            if let Some(ch) = profile.channels.iter().find(|c| &c.id == ch_id) {
                let eq_model = convert::eq_model_for(ch)?;
                let handle = {
                    let mut mgr = ChannelManager::new(&mut self.runner, channel_set.clone());
                    mgr.set_output(ch_id, Some(surround_target.clone()), &eq_model)
                };
                match handle {
                    Ok(h) => {
                        if let Some(t) = h.child {
                            self.children.track(t);
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "warning: apply_surround reroute channel '{ch_id}' failed (ignoring): {e}"
                        );
                    }
                }
                self.surround_routed.insert(ch_id.clone());
            }
        }

        Ok(())
    }

    // ── Surround engine methods ──────────────────────────────────────────────────

    /// Enable or disable virtual surround. When enabling, resolves HRIR first (errors if none found).
    pub fn surround_set_enabled(&mut self, on: bool) -> Result<(), crate::error::EngineError> {
        // When enabling, validate HRIR exists first (before mutating config).
        if on {
            let profile = self.config.active()?.clone();
            let base = convert::hrir_base_dir()?;
            convert::resolve_hrir_path(&profile.surround, &base)?;
        }
        // Mutate config
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    name.clone(),
                ))
            })?;
            profile.surround.enabled = on;
        }
        self.save_config()?;
        // Apply via the canonical path (handles sink create/remove AND channel routing).
        // When enabling: apply_surround recreates sink + routes desired channels → surround_routed.
        // When disabling: apply_surround removes sink + restores all surround_routed channels.
        {
            let profile = self.config.active()?.clone();
            self.apply_surround(&profile)?;
        }
        self.emit(crate::state::Event::SurroundEnabledSet { enabled: on });
        Ok(())
    }

    /// Set the HRIR profile stem. Validates file exists, persists, recreates if enabled.
    pub fn surround_set_hrir(&mut self, stem: String) -> Result<(), crate::error::EngineError> {
        // Validate: update config temporarily to check path existence.
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    name.clone(),
                ))
            })?;
            let old = profile.surround.hrir.clone();
            profile.surround.hrir = Some(stem.clone());
            // Validate the resolved path exists.
            let base = convert::hrir_base_dir()?;
            if let Err(e) = convert::resolve_hrir_path(&profile.surround, &base) {
                // Roll back mutation.
                profile.surround.hrir = old;
                return Err(e);
            }
        }
        self.save_config()?;
        // Apply via the canonical path (recreates sink with new HRIR if enabled;
        // no-op remove if disabled; channel routing is a no-op since channels are
        // already tracked in surround_routed).
        {
            let profile = self.config.active()?.clone();
            self.apply_surround(&profile)?;
        }
        let hrir_snapshot = self.config.active()?.surround.hrir.clone();
        self.emit(crate::state::Event::SurroundHrirSet {
            hrir: hrir_snapshot,
        });
        Ok(())
    }

    /// Set the channels routed through surround. Persists, recreates routing if enabled.
    pub fn surround_set_channels(
        &mut self,
        channels: Vec<String>,
    ) -> Result<(), crate::error::EngineError> {
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    name.clone(),
                ))
            })?;
            profile.surround.channels = channels;
        }
        self.save_config()?;
        // Reapply routing if enabled.
        if self.config.active()?.surround.enabled {
            let profile = self.config.active()?.clone();
            self.apply_surround(&profile)?;
        }
        let channels_snapshot = self.config.active()?.surround.channels.clone();
        self.emit(crate::state::Event::SurroundChannelsSet {
            channels: channels_snapshot,
        });
        Ok(())
    }

    /// Set (or clear) the hardware sink for the surround output tail. Recreates if enabled.
    pub fn surround_set_hw_sink(
        &mut self,
        hw_sink: Option<String>,
    ) -> Result<(), crate::error::EngineError> {
        {
            let name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&name).ok_or_else(|| {
                crate::error::EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    name.clone(),
                ))
            })?;
            profile.surround.hw_sink = hw_sink;
        }
        self.save_config()?;
        // Apply via the canonical path (recreates sink with new hw_sink baked into conf if
        // enabled; no-op remove if disabled; channel routing is a no-op since channels are
        // already tracked in surround_routed).
        {
            let profile = self.config.active()?.clone();
            self.apply_surround(&profile)?;
        }
        let hw_sink_snapshot = self.config.active()?.surround.hw_sink.clone();
        self.emit(crate::state::Event::SurroundHwSinkSet {
            hw_sink: hw_sink_snapshot,
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
                        volume_db: 0.0,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                ],
                routes: vec![],
                mic: MicChainConfig::default(),
                surround: arctis_config::SurroundConfig::default(),
                master_volume_db: 0.0,
                master_mute: false,
                chatmix_position: 4,
                default_sink_channel: None,
            }],
            eq_presets: vec![],
            dial_controls_balance: true,
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
                        volume_db: 0.0,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: Some("alsa_output.speakers".into()),
                        eq: vec![],
                        volume_db: 0.0,
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
                master_mute: false,
                chatmix_position: 4,
                default_sink_channel: None,
            }],
            eq_presets: vec![],
            dial_controls_balance: true,
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
            "id 13\n    node.name = \"Arctis_Aux\"\n",
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
        // Engine::new seeds "aux" via ensure_standard_channels() → 4 channels total.
        // Per channel: Phase 2 (1 ls + 10 bands) then Phase 2b (1 ls + 1 Props), interleaved.
        // Reconcile processes each channel fully before moving to the next.
        let ls = ls_all_present(); // includes Arctis_Game/Chat/Media/Aux

        // Queue outputs (interleaved per channel):
        // Phase 1 (channels up): 4 × 1 ls-Node
        // Per channel (4 channels): (1 ls + 10 bands) + (1 ls + 1 Props) = 13 × 4 = 52
        // Phase 5: 1 ls (mic disabled)
        // Phase 6: 1 ls (surround disabled)
        // Total: 4 + 52 + 1 + 1 = 58
        let runner = MockRunner::new()
            // Phase 1: channel up — game[0], chat[1], media[2], aux[3] (all present)
            .with_output(0, &ls, "") // [0] game
            .with_output(0, &ls, "") // [1] chat
            .with_output(0, &ls, "") // [2] media
            .with_output(0, &ls, "") // [3] aux
            // game: Phase 2 (EQ) + Phase 2b (vol/mute)
            .with_output(0, &ls, "") // [4] game EQ ls
            .with_output(0, "", "") // [5]
            .with_output(0, "", "") // [6]
            .with_output(0, "", "") // [7]
            .with_output(0, "", "") // [8]
            .with_output(0, "", "") // [9]
            .with_output(0, "", "") // [10]
            .with_output(0, "", "") // [11]
            .with_output(0, "", "") // [12]
            .with_output(0, "", "") // [13]
            .with_output(0, "", "") // [14] game 10 band sets
            .with_output(0, &ls, "") // [15] game vol find_node_id
            .with_output(0, "", "") // [16] game vol Props set
            // chat: Phase 2 (EQ) + Phase 2b (vol/mute)
            .with_output(0, &ls, "") // [17] chat EQ ls
            .with_output(0, "", "") // [18]
            .with_output(0, "", "") // [19]
            .with_output(0, "", "") // [20]
            .with_output(0, "", "") // [21]
            .with_output(0, "", "") // [22]
            .with_output(0, "", "") // [23]
            .with_output(0, "", "") // [24]
            .with_output(0, "", "") // [25]
            .with_output(0, "", "") // [26]
            .with_output(0, "", "") // [27] chat 10 band sets
            .with_output(0, &ls, "") // [28] chat vol find_node_id
            .with_output(0, "", "") // [29] chat vol Props set
            // media: Phase 2 (EQ) + Phase 2b (vol/mute)
            .with_output(0, &ls, "") // [30] media EQ ls
            .with_output(0, "", "") // [31]
            .with_output(0, "", "") // [32]
            .with_output(0, "", "") // [33]
            .with_output(0, "", "") // [34]
            .with_output(0, "", "") // [35]
            .with_output(0, "", "") // [36]
            .with_output(0, "", "") // [37]
            .with_output(0, "", "") // [38]
            .with_output(0, "", "") // [39]
            .with_output(0, "", "") // [40] media 10 band sets
            .with_output(0, &ls, "") // [41] media vol find_node_id
            .with_output(0, "", "") // [42] media vol Props set
            // aux: Phase 2 (EQ) + Phase 2b (vol/mute)
            .with_output(0, &ls, "") // [43] aux EQ ls
            .with_output(0, "", "") // [44]
            .with_output(0, "", "") // [45]
            .with_output(0, "", "") // [46]
            .with_output(0, "", "") // [47]
            .with_output(0, "", "") // [48]
            .with_output(0, "", "") // [49]
            .with_output(0, "", "") // [50]
            .with_output(0, "", "") // [51]
            .with_output(0, "", "") // [52]
            .with_output(0, "", "") // [53] aux 10 band sets
            .with_output(0, &ls, "") // [54] aux vol find_node_id
            .with_output(0, "", "") // [55] aux vol Props set
            // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
            .with_output(0, &ls, "") // [56]
            // Phase 6: surround disabled → remove() → source_exists() → 1 ls (surround absent)
            .with_output(0, &ls, ""); // [57]

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        let calls = &engine.runner.calls;

        // Phase 1: 4 ls-Node calls for channel creation (all present, no spawns)
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"], "game up ls");
        assert_eq!(calls[1], vec!["pw-cli", "ls", "Node"], "chat up ls");
        assert_eq!(calls[2], vec!["pw-cli", "ls", "Node"], "media up ls");
        assert_eq!(calls[3], vec!["pw-cli", "ls", "Node"], "aux up ls");

        // Phase 2: apply_all game — ls Node then 10 pw-cli s Props calls
        assert_eq!(
            calls[4],
            vec!["pw-cli", "ls", "Node"],
            "game eq find_node_id"
        );
        assert_eq!(calls[5][0], "pw-cli", "game band 0 set");
        assert_eq!(calls[5][1], "s");
        assert_eq!(calls[5][3], "Props");

        // Phase 2b: apply_volume_mute game — index 15
        assert_eq!(
            calls[15],
            vec!["pw-cli", "ls", "Node"],
            "game vol find_node_id"
        );

        // Phase 2 chat: EQ starts at index 17
        assert_eq!(
            calls[17],
            vec!["pw-cli", "ls", "Node"],
            "chat eq find_node_id"
        );

        // Phase 2b chat: volume starts at index 28
        assert_eq!(
            calls[28],
            vec!["pw-cli", "ls", "Node"],
            "chat vol find_node_id"
        );

        // Phase 2 media: EQ starts at index 30
        assert_eq!(
            calls[30],
            vec!["pw-cli", "ls", "Node"],
            "media eq find_node_id"
        );

        // Phase 2b media: volume starts at index 41
        assert_eq!(
            calls[41],
            vec!["pw-cli", "ls", "Node"],
            "media vol find_node_id"
        );

        // Phase 2 aux: EQ starts at index 43
        assert_eq!(
            calls[43],
            vec!["pw-cli", "ls", "Node"],
            "aux eq find_node_id"
        );

        // Phase 2b aux: volume starts at index 54
        assert_eq!(
            calls[54],
            vec!["pw-cli", "ls", "Node"],
            "aux vol find_node_id"
        );

        // Total: 4 (up) + 4*(1+10+1+1) (apply_all+vol/mute) + 1 (mic step5) + 1 (surround step6) = 58
        assert_eq!(calls.len(), 58, "expected 58 total pw-cli calls");

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

        let runner = MockRunner::new()
            // Phase 1: 4 ls calls only (spawn_owned does not consume queued outputs)
            .with_output(0, &ls_absent, "") // [0] game ls (absent)
            .with_output(0, &ls_absent, "") // [1] chat ls (absent)
            .with_output(0, &ls_absent, "") // [2] media ls (absent)
            .with_output(0, &ls_absent, "") // [3] aux ls (absent, seeded by Engine::new)
            // game: Phase 2 (EQ apply) + Phase 2b (vol/mute apply)
            .with_output(0, &ls_present, "") // [4] game EQ find_node_id
            .with_output(0, "", "") // [5]
            .with_output(0, "", "") // [6]
            .with_output(0, "", "") // [7]
            .with_output(0, "", "") // [8]
            .with_output(0, "", "") // [9]
            .with_output(0, "", "") // [10]
            .with_output(0, "", "") // [11]
            .with_output(0, "", "") // [12]
            .with_output(0, "", "") // [13]
            .with_output(0, "", "") // [14] game 10 band sets
            .with_output(0, &ls_present, "") // [15] game vol find_node_id
            .with_output(0, "", "") // [16] game vol Props set
            // chat: Phase 2 (EQ apply) + Phase 2b (vol/mute apply)
            .with_output(0, &ls_present, "") // [17] chat EQ find_node_id
            .with_output(0, "", "") // [18]
            .with_output(0, "", "") // [19]
            .with_output(0, "", "") // [20]
            .with_output(0, "", "") // [21]
            .with_output(0, "", "") // [22]
            .with_output(0, "", "") // [23]
            .with_output(0, "", "") // [24]
            .with_output(0, "", "") // [25]
            .with_output(0, "", "") // [26]
            .with_output(0, "", "") // [27] chat 10 band sets
            .with_output(0, &ls_present, "") // [28] chat vol find_node_id
            .with_output(0, "", "") // [29] chat vol Props set
            // media: Phase 2 (EQ apply) + Phase 2b (vol/mute apply)
            .with_output(0, &ls_present, "") // [30] media EQ find_node_id
            .with_output(0, "", "") // [31]
            .with_output(0, "", "") // [32]
            .with_output(0, "", "") // [33]
            .with_output(0, "", "") // [34]
            .with_output(0, "", "") // [35]
            .with_output(0, "", "") // [36]
            .with_output(0, "", "") // [37]
            .with_output(0, "", "") // [38]
            .with_output(0, "", "") // [39]
            .with_output(0, "", "") // [40] media 10 band sets
            .with_output(0, &ls_present, "") // [41] media vol find_node_id
            .with_output(0, "", "") // [42] media vol Props set
            // aux: Phase 2 (EQ apply) + Phase 2b (vol/mute apply)
            .with_output(0, &ls_present, "") // [43] aux EQ find_node_id
            .with_output(0, "", "") // [44]
            .with_output(0, "", "") // [45]
            .with_output(0, "", "") // [46]
            .with_output(0, "", "") // [47]
            .with_output(0, "", "") // [48]
            .with_output(0, "", "") // [49]
            .with_output(0, "", "") // [50]
            .with_output(0, "", "") // [51]
            .with_output(0, "", "") // [52]
            .with_output(0, "", "") // [53] aux 10 band sets
            .with_output(0, &ls_present, "") // [54] aux vol find_node_id
            .with_output(0, "", "") // [55] aux vol Props set
            // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node)
            .with_output(0, &ls_absent, "") // [56]
            // Phase 6: surround disabled → remove() → source_exists() → 1 ls (surround absent)
            .with_output(0, &ls_absent, ""); // [57]

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(runner, cfg);
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        let calls = &engine.runner.calls;

        // Phase 1: only the 4 ls-Node existence checks (spawn_owned goes to `spawned`)
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"], "game up ls");
        assert_eq!(calls[1], vec!["pw-cli", "ls", "Node"], "chat up ls");
        assert_eq!(calls[2], vec!["pw-cli", "ls", "Node"], "media up ls");
        assert_eq!(calls[3], vec!["pw-cli", "ls", "Node"], "aux up ls");

        // Phase 2: apply game EQ starts at index 4 (right after phase1 ls calls)
        assert_eq!(
            calls[4],
            vec!["pw-cli", "ls", "Node"],
            "game eq find_node_id"
        );
        assert_eq!(calls[5][0], "pw-cli", "game band 0 program");
        assert_eq!(calls[5][1], "s", "game band 0 sub-cmd");
        assert_eq!(calls[5][3], "Props", "game band 0 Props");

        // Phase 2b: apply game vol at index 15
        assert_eq!(
            calls[15],
            vec!["pw-cli", "ls", "Node"],
            "game vol find_node_id"
        );

        // Phase 2 chat EQ: index 17
        assert_eq!(
            calls[17],
            vec!["pw-cli", "ls", "Node"],
            "chat eq find_node_id"
        );

        // Phase 2b chat vol: index 28
        assert_eq!(
            calls[28],
            vec!["pw-cli", "ls", "Node"],
            "chat vol find_node_id"
        );

        // Phase 2 media EQ: index 30
        assert_eq!(
            calls[30],
            vec!["pw-cli", "ls", "Node"],
            "media eq find_node_id"
        );

        // Phase 2b media vol: index 41
        assert_eq!(
            calls[41],
            vec!["pw-cli", "ls", "Node"],
            "media vol find_node_id"
        );

        // Phase 2 aux EQ: index 43
        assert_eq!(
            calls[43],
            vec!["pw-cli", "ls", "Node"],
            "aux eq find_node_id"
        );

        // Phase 2b aux vol: index 54
        assert_eq!(
            calls[54],
            vec!["pw-cli", "ls", "Node"],
            "aux vol find_node_id"
        );

        // Total: 4 (phase1 ls) + 4*(1+10+1+1) (EQ+vol) + 1 (mic step5) + 1 (surround step6) = 58
        assert_eq!(
            calls.len(),
            58,
            "expected 58 total run calls (no spawns in calls)"
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
                        muted: false,
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
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
                master_mute: false,
                chatmix_position: 4,
                default_sink_channel: None,
            }],
            eq_presets: vec![],
            dial_controls_balance: true,
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
        // spawns all 4 (game/chat/media/aux) via spawn_owned. Then shutdown must kill all 4.
        // Engine::new calls ensure_standard_channels() → aux is seeded automatically.
        let ls_absent = ls_all_absent();
        let ls_present = ls_all_present();

        let runner = MockRunner::new()
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

        // 14 run calls total: 4 (phase1 ls) + 4 (phase2 find_node_id) + 4 (phase2b find_node_id) + 1 (mic step5) + 1 (surround step6)
        assert_eq!(
            engine.runner.calls.len(),
            14,
            "expected 14 run calls: 4 ls-up + 4 ls-find-node + 4 ls-vol-find-node + 1 mic source_exists + 1 surround source_exists"
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

    /// Queue enough MockRunner outputs to survive `reconcile()` on a 4-channel
    /// (game/chat/media/aux), no-EQ, no-routes config where all sinks are already
    /// present AND mic disabled. Engine::new calls ensure_standard_channels() which
    /// adds "aux" to any 3-channel config, making this a 4-channel reconcile.
    ///
    /// Step 5 (mic): mic disabled → MicBackend::remove() → source_exists() → 1 ls Node
    /// returning the "no mic" output (source absent, remove returns immediately).
    fn queue_reconcile_present(runner: MockRunner) -> MockRunner {
        let ls = ls_all_present();
        let ls_no_mic = ls_all_present(); // "present" for channels but no mic node
        let mut r = runner;
        // Phase 1: 4 ls (all present, including aux seeded by Engine::new)
        for _ in 0..4 {
            r = r.with_output(0, &ls, "");
        }
        // Phase 2 + 2b interleaved: per channel (4 channels), EQ apply then volume/mute apply
        for _ in 0..4 {
            // Phase 2: EQ apply (1 ls + 10 band sets)
            r = r.with_output(0, &ls, "");
            for _ in 0..10 {
                r = r.with_output(0, "", "");
            }
            // Phase 2b: volume/mute apply (1 ls + 1 Props set)
            r = r.with_output(0, &ls, ""); // find_node_id
            r = r.with_output(0, "", ""); // Props set
        }
        // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node found)
        r = r.with_output(0, &ls_no_mic, "");
        // Phase 6: surround disabled → remove() → source_exists() → 1 ls (surround absent)
        r = r.with_output(0, &ls, "");
        r
    }

    #[test]
    fn state_reflects_active_profile() {
        let cfg = make_config_no_eq_no_routes();
        let engine = Engine::new(MockRunner::new(), cfg);
        let s = engine.state();
        assert_eq!(s.active_profile, "default");
        // Engine::new calls ensure_standard_channels() which adds "aux" to the 3-channel config.
        assert_eq!(s.channels.len(), 4);
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
                    volume_db: 0.0,
                    muted: false,
                },
                ChannelConfig {
                    id: "chat".into(),
                    node_name: "Arctis_Chat".into(),
                    description: "Chat".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    muted: false,
                },
                ChannelConfig {
                    id: "media".into(),
                    node_name: "Arctis_Media".into(),
                    description: "Media".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    muted: false,
                },
            ],
            routes: vec![],
            mic: MicChainConfig::default(),
            surround: arctis_config::SurroundConfig::default(),
            master_volume_db: 0.0,
            master_mute: false,
            chatmix_position: 4,
            default_sink_channel: None,
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
                        volume_db: 0.0,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: Some("alsa_output.headphones".into()),
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                ],
                routes: vec![],
                mic: MicChainConfig::default(),
                surround: arctis_config::SurroundConfig::default(),
                master_volume_db: 0.0,
                master_mute: false,
                chatmix_position: 4,
                default_sink_channel: None,
            }],
            eq_presets: vec![],
            dial_controls_balance: true,
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
            .set_channel_volume("game", -6.0)
            .expect("set_channel_volume should succeed");

        // Persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config must be persisted");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("volume_db = -6"),
            "config.toml must contain volume_db = -6, got: {saved_str}"
        );

        // In-memory state updated
        let state = engine.state();
        let ch = state.channels.iter().find(|c| c.id == "game").unwrap();
        assert!(
            (ch.volume_db - (-6.0)).abs() < f32::EPSILON,
            "volume_db must be -6.0"
        );

        // Event emitted
        let event = rx
            .try_recv()
            .expect("ChannelVolumeSet event must be emitted");
        assert!(
            matches!(
                event,
                crate::state::Event::ChannelVolumeSet {
                    ref channel_id,
                    volume_db,
                } if channel_id == "game" && (volume_db - (-6.0)).abs() < f32::EPSILON
            ),
            "event must be ChannelVolumeSet{{channel_id: game, volume_db: -6.0}}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_channel_volume_rejects_out_of_range() {
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let err = engine
            .set_channel_volume("game", 100.0)
            .expect_err("100 dB should be rejected");
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
            .set_channel_volume("nonexistent", 0.0)
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
    // TDD: F3 profile management — rename active, EQ preset unit tests
    // ─────────────────────────────────────────────

    #[test]
    fn rename_active_profile_updates_active_profile_field() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("rename_active");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        // Verify initial active profile
        assert_eq!(engine.state().active_profile, "default");

        engine
            .rename_profile("default", "my-renamed")
            .expect("rename_profile should succeed");

        // active_profile in state must reflect the new name
        assert_eq!(
            engine.state().active_profile,
            "my-renamed",
            "state().active_profile must be updated after renaming the active profile"
        );
        // Old name must be gone, new name must exist
        let names = engine.config().profile_names();
        assert!(
            !names.contains(&"default".to_string()),
            "old profile name must not exist"
        );
        assert!(
            names.contains(&"my-renamed".to_string()),
            "new profile name must exist"
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
                        volume_db: 0.0,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                ],
                routes: vec![],
                mic: mic_enabled_passthrough(),
                surround: arctis_config::SurroundConfig::default(),
                master_volume_db: 0.0,
                master_mute: false,
                chatmix_position: 4,
                default_sink_channel: None,
            }],
            eq_presets: vec![],
            dial_controls_balance: true,
        }
    }

    /// Queue outputs for a 3-channel reconcile with mic DISABLED.
    fn queue_reconcile_with_mic_disabled(runner: MockRunner) -> MockRunner {
        queue_reconcile_present(runner)
    }

    /// Queue outputs for a 4-channel reconcile with mic ENABLED and source absent.
    /// Engine::new seeds "aux" → 4 channels (game/chat/media/aux).
    /// Step5: create() → source_exists() (1 ls, absent) → spawn (goes to spawned)
    fn queue_reconcile_with_mic_enabled_absent(runner: MockRunner) -> MockRunner {
        let ls = ls_all_present();
        let ls_mic_absent = ls_without_mic();
        let mut r = runner;
        // Phase 1: 4 ls (all present, including aux seeded by Engine::new)
        for _ in 0..4 {
            r = r.with_output(0, &ls, "");
        }
        // Phase 2 + 2b interleaved: per channel (4), EQ apply then volume/mute apply
        for _ in 0..4 {
            // Phase 2: EQ apply (1 ls + 10 band sets)
            r = r.with_output(0, &ls, "");
            for _ in 0..10 {
                r = r.with_output(0, "", "");
            }
            // Phase 2b: volume/mute apply (1 ls + 1 Props set)
            r = r.with_output(0, &ls, ""); // find_node_id
            r = r.with_output(0, "", ""); // Props set
        }
        // Phase 5: mic enabled → create() → source_exists() (absent) → spawn
        r = r.with_output(0, &ls_mic_absent, "");
        // Phase 6: surround disabled → remove() → source_exists() → 1 ls (surround absent)
        r = r.with_output(0, &ls, "");
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

    /// Test 3: suppression enabled but probe returns none → spawn still fires (chain minus suppression);
    ///         state marks suppression unavailable.
    #[test]
    fn reconcile_drops_unavailable_rnnoise_but_still_builds() {
        let mut cfg = make_config_mic_enabled();
        // Enable suppression (default backend = DeepFilter) in config
        cfg.profiles[0].mic.suppression.enabled = true;

        // Queue for mic-enabled absent reconcile (deepfilter probe is none → dropped from chain)
        let runner = queue_reconcile_with_mic_enabled_absent(MockRunner::new());
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        // Spawn still fires (chain has passthrough fallback)
        let spawned = &engine.runner.spawned;
        assert!(
            spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "spawn must fire even when suppression is unavailable"
        );

        // state reports suppression unavailable
        let state = engine.state();
        let suppression_stage = state
            .mic
            .stages
            .iter()
            .find(|s| s.kind == crate::state::StageName::Suppression);
        let s = suppression_stage.expect("suppression must appear in stages");
        assert!(!s.available, "suppression must be marked unavailable");
        assert!(s.enabled, "suppression must show as enabled in config");
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
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));
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
        // Use Rnnoise backend so VadThreshold hits the VAD control path
        cfg.profiles[0].mic.suppression.enabled = true;
        cfg.profiles[0].mic.suppression.backend = arctis_config::SuppressionBackend::Rnnoise;
        cfg.profiles[0].mic.suppression.vad_threshold = 40.0;

        let ls_with_mic = ls_with_mic();

        // mic_set_param(VadThreshold, 55.0):
        //   apply_control("mic_suppression", "VAD Threshold (%)", 55.0)
        //   → find_node_id (1 ls) + pw-cli s 71 Props … (1 call)
        let runner = MockRunner::new()
            .with_output(0, &ls_with_mic, "") // find_node_id
            .with_output(0, "", ""); // set Props

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_param(MicParam::VadThreshold, 55.0)
            .expect("mic_set_param should succeed");

        let calls = &engine.runner.calls;
        // Last call should be: pw-cli s 71 Props { params = [ "mic_suppression:VAD Threshold (%)" 55.0 ] }
        let last = calls.last().expect("at least one call");
        assert_eq!(last[0], "pw-cli");
        assert_eq!(last[1], "s");
        assert_eq!(last[2], "71", "node id must be 71 (from ls fixture)");
        assert_eq!(last[3], "Props");
        assert_eq!(
            last[4], "{ params = [ \"mic_suppression:VAD Threshold (%)\" 55.0 ] }",
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

    /// Test 7: state().mic.stages reports suppression unavailable when probe is missing.
    #[test]
    fn state_reports_mic_stage_availability() {
        let mut cfg = make_config_mic_enabled();
        cfg.profiles[0].mic.suppression.enabled = true; // request suppression

        // Queue for reconcile with mic enabled, deepfilter probe absent → suppression dropped
        let runner = queue_reconcile_with_mic_enabled_absent(MockRunner::new());
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));
        engine.reconcile().expect("reconcile should succeed");

        let state = engine.state();
        assert!(state.mic.enabled, "mic must show enabled");

        let suppression = state
            .mic
            .stages
            .iter()
            .find(|s| s.kind == crate::state::StageName::Suppression)
            .expect("suppression stage must be in state");

        assert!(suppression.enabled, "suppression enabled in config");
        assert!(
            !suppression.available,
            "suppression must be unavailable (probe returns false)"
        );
    }

    // ─────────────────────────────────────────────
    // Task 5b TDD: mic_set_enabled (master switch)
    // ─────────────────────────────────────────────

    /// Build a 3-channel config with mic master switch DISABLED (default).
    fn make_config_mic_disabled() -> Config {
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
                        muted: false,
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                ],
                routes: vec![],
                mic: arctis_config::MicChainConfig {
                    enabled: false,
                    hw_mic: Some("alsa_input.hw_mic".to_string()),
                    ..Default::default()
                },
                surround: arctis_config::SurroundConfig::default(),
                master_volume_db: 0.0,
                master_mute: false,
                chatmix_position: 4,
                default_sink_channel: None,
            }],
            eq_presets: vec![],
            dial_controls_balance: true,
        }
    }

    /// Test 5b-1: mic_set_enabled(true) from master-off spawns the mic source and persists.
    #[test]
    fn mic_set_enabled_true_builds_source_and_persists() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_set_enabled_true");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_disabled(); // mic.enabled = false

        // mic_set_enabled(true) → create():
        //   source_exists() (1 ls, absent) → spawn
        let ls_absent = ls_without_mic();
        let runner = MockRunner::new().with_output(0, &ls_absent, "");

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_enabled(true)
            .expect("mic_set_enabled(true) should succeed");

        // Config persisted to disk with mic.enabled = true
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("enabled = true"),
            "persisted config must show mic.enabled = true"
        );

        // Reload and confirm
        let reloaded = arctis_config::store::load().expect("reload must succeed");
        assert!(
            reloaded
                .active()
                .expect("active profile must exist")
                .mic
                .enabled,
            "reloaded config must show mic.enabled = true"
        );

        // Mic source was spawned
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "mic source must be spawned when enabling"
        );
        // One child tracked
        assert_eq!(engine.children.len(), 1, "one mic child must be tracked");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Test 5b-2: mic_set_enabled(false) from master-on takes the remove path.
    #[test]
    fn mic_set_enabled_false_removes_source() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_set_enabled_false");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_enabled(); // mic.enabled = true

        // mic_set_enabled(false) → remove():
        //   source_exists() (1 ls, absent, so no destroy needed)
        let ls_absent = ls_without_mic();
        let runner = MockRunner::new().with_output(0, &ls_absent, "");

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_enabled(false)
            .expect("mic_set_enabled(false) should succeed");

        // Config persisted with mic.enabled = false
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");

        // No spawn (remove path, source already absent)
        assert!(
            engine.runner.spawned.is_empty(),
            "no spawn on disable when source already absent"
        );
        assert_eq!(engine.children.len(), 0, "no child tracked on disable");

        // Reload and confirm
        let reloaded = arctis_config::store::load().expect("reload must succeed");
        assert!(
            !reloaded
                .active()
                .expect("active profile must exist")
                .mic
                .enabled,
            "reloaded config must show mic.enabled = false"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Fix 2+3 test A: mic_set_stage_enabled while master is OFF → config persisted, no spawn,
    /// children unchanged, event emitted.
    #[test]
    fn mic_set_stage_enabled_when_master_off_persists_only_no_spawn() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("stage_master_off");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Master switch is OFF; gain stage starts OFF.
        let cfg = make_config_mic_disabled();

        // No runner outputs needed — master off path skips all graph I/O.
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine =
            Engine::with_probe(MockRunner::new(), cfg, Box::new(MockPluginProbe::none()));
        engine.set_event_sink(tx);

        engine
            .mic_set_stage_enabled(StageKind::Gain, true)
            .expect("mic_set_stage_enabled should succeed when master is off");

        // Config persisted with gain.enabled = true
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("enabled = true"),
            "persisted config must show gain enabled"
        );

        // No spawn: master off → no recreate
        assert!(
            engine.runner.spawned.is_empty(),
            "no spawn when master is off"
        );
        // No graph I/O calls (no pw-cli destroy / no create ls)
        assert!(
            engine.runner.calls.is_empty(),
            "no pw-cli calls when master is off"
        );
        // No child tokens tracked
        assert_eq!(engine.children.len(), 0, "no children when master is off");

        // Event must still be emitted
        let event = rx.try_recv().expect("MicStageSet event must be emitted");
        assert_eq!(
            event,
            crate::state::Event::MicStageSet {
                stage: crate::state::StageName::Gain,
                enabled: true,
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Fix 2+3 test B: mic_set_param while master is OFF → config persisted, no pw-cli s Props call,
    /// event emitted.
    #[test]
    fn mic_set_param_when_master_off_persists_only_no_props() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("param_master_off");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_mic_disabled(); // master off
        cfg.profiles[0].mic.highpass.enabled = true;
        cfg.profiles[0].mic.highpass.freq_hz = 80.0;

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine =
            Engine::with_probe(MockRunner::new(), cfg, Box::new(MockPluginProbe::none()));
        engine.set_event_sink(tx);

        engine
            .mic_set_param(MicParam::HighpassFreq, 120.0)
            .expect("mic_set_param should succeed when master is off");

        // Config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");

        // No pw-cli s ... Props call
        assert!(
            engine.runner.calls.is_empty(),
            "no pw-cli calls (no apply_control) when master is off"
        );

        // Event must still be emitted
        let event = rx.try_recv().expect("MicParamSet event must be emitted");
        assert_eq!(
            event,
            crate::state::Event::MicParamSet {
                param: MicParam::HighpassFreq,
                value: 120.0,
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    // ── Task 2 new engine tests ───────────────────────────────────────────────

    /// Task 2 test A: mic_set_suppression_backend persists, recreates, emits event.
    #[test]
    fn mic_set_suppression_backend_persists_recreates_emits() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("suppression_backend");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_enabled(); // mic.enabled = true
        let ls_with_mic = ls_with_mic();
        let ls_absent = ls_without_mic();

        // recreate() = remove() + create():
        // remove: source_exists (present), find_node_id, destroy, pkill = 4 calls
        // create: source_exists (absent) = 1 call + spawn
        let runner = MockRunner::new()
            .with_output(0, &ls_with_mic, "") // remove: source_exists
            .with_output(0, &ls_with_mic, "") // remove: find_node_id
            .with_output(0, "", "") // remove: destroy
            .with_output(0, "", "") // remove: pkill
            .with_output(0, &ls_absent, ""); // create: source_exists

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.set_event_sink(tx);
        // Pre-seed pw_version so ensure_pw_version() is a no-op (no extra runner call).
        engine.seed_pw_version((1, 6, 0));

        engine
            .mic_set_suppression_backend(crate::state::SuppressionBackend::Rnnoise)
            .expect("mic_set_suppression_backend should succeed");

        // Config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("rnnoise"),
            "persisted config must contain rnnoise backend"
        );

        // Recreate occurred: destroy was called
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c.len() >= 3 && c[1] == "destroy"),
            "destroy must be called during recreate"
        );

        // Event emitted
        let event = rx
            .try_recv()
            .expect("MicSuppressionBackendSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::MicSuppressionBackendSet {
                backend: crate::state::SuppressionBackend::Rnnoise,
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Task 2 test B: mic_set_param(AttenuationLimitDb) emits Props for mic_suppression.
    #[test]
    fn mic_set_param_attenuation_limit_emits_exact_props() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("atten_limit");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let mut cfg = make_config_mic_enabled();
        cfg.profiles[0].mic.suppression.enabled = true;
        cfg.profiles[0].mic.suppression.backend = arctis_config::SuppressionBackend::DeepFilter;
        cfg.profiles[0].mic.suppression.attenuation_limit_db = 100.0;

        let ls_with_mic = ls_with_mic();
        let runner = MockRunner::new()
            .with_output(0, &ls_with_mic, "") // find_node_id
            .with_output(0, "", ""); // set Props

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_param(MicParam::AttenuationLimitDb, 80.0)
            .expect("AttenuationLimitDb must succeed");

        let last = engine.runner.calls.last().expect("at least one call");
        assert_eq!(last[3], "Props");
        assert!(
            last[4].contains("mic_suppression:Attenuation Limit (dB)"),
            "Props must reference mic_suppression:Attenuation Limit (dB), got: {}",
            last[4]
        );
        assert!(
            last[4].contains("80"),
            "Props must contain the value 80, got: {}",
            last[4]
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Task 2 test C: state().mic.suppression_backend and available_suppression_backends.
    #[test]
    fn state_reports_suppression_backend_and_availability() {
        use crate::state::SuppressionBackend;
        use arctis_audio::MockPluginProbe;
        // Use a probe that reports DeepFilter available but not RNNoise
        let probe = MockPluginProbe::with([arctis_audio::DEEPFILTER_PLUGIN_BASENAME]);
        let mut cfg = make_config_mic_enabled();
        cfg.profiles[0].mic.suppression.backend = arctis_config::SuppressionBackend::DeepFilter;
        let engine = Engine::with_probe(MockRunner::new(), cfg, Box::new(probe));
        let state = engine.state();
        assert_eq!(
            state.mic.suppression_backend,
            SuppressionBackend::DeepFilter,
            "suppression_backend must match config"
        );
        assert!(
            state
                .mic
                .available_suppression_backends
                .contains(&SuppressionBackend::DeepFilter),
            "DeepFilter must appear in available backends when probe reports it present"
        );
        assert!(
            !state
                .mic
                .available_suppression_backends
                .contains(&SuppressionBackend::Rnnoise),
            "Rnnoise must not appear in available backends when probe reports it absent"
        );
    }

    /// Task 2 test D: gate threshold change recreates (not live Props).
    #[test]
    fn mic_set_param_gate_threshold_recreates_not_live_props() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("gate_threshold_recreate");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_enabled(); // gate disabled by default

        // recreate(): remove (source absent → no-op 1 ls) + create (absent → spawn)
        let ls_absent = ls_without_mic();
        let runner = MockRunner::new()
            .with_output(0, &ls_absent, "") // remove: source_exists (absent, no-op)
            .with_output(0, &ls_absent, ""); // create: source_exists (absent → spawn)

        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine
            .mic_set_param(MicParam::GateThreshold, 0.005)
            .expect("GateThreshold must succeed");

        // Must have spawned (recreate happened), no direct Props set call
        assert!(
            engine.runner.spawned.iter().any(|argv| argv
                .get(2)
                .map(|s| s.ends_with("arctis_clean_mic.conf"))
                .unwrap_or(false)),
            "recreate must spawn arctis_clean_mic.conf for gate threshold change"
        );
        // No Props set call (no pw-cli s ... Props call)
        assert!(
            !engine
                .runner
                .calls
                .iter()
                .any(|c| c.len() >= 4 && c[1] == "s" && c[3] == "Props"),
            "gate threshold must NOT emit Props (must recreate)"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    /// Task 2 test E: mic_set_enabled emits Event::MicEnabledSet.
    #[test]
    fn mic_set_enabled_emits_event() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("mic_set_enabled_event");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_mic_disabled();

        let ls_absent = ls_without_mic();
        let runner = MockRunner::new().with_output(0, &ls_absent, "");

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::with_probe(runner, cfg, Box::new(MockPluginProbe::none()));
        engine.set_event_sink(tx);

        engine
            .mic_set_enabled(true)
            .expect("mic_set_enabled should succeed");

        let event = rx.try_recv().expect("MicEnabledSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::MicEnabledSet { enabled: true },
            "event must be MicEnabledSet {{ enabled: true }}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
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

    // ─────────────────────────────────────────────
    // F1.3 TDD: reconcile step6 (surround)
    // ─────────────────────────────────────────────

    /// Build a config with surround enabled, pointing to a real temp HRIR file.
    fn make_config_surround_enabled(hrir_stem: &str) -> Config {
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
                        muted: false,
                    },
                    ChannelConfig {
                        id: "chat".into(),
                        node_name: "Arctis_Chat".into(),
                        description: "Chat".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                    ChannelConfig {
                        id: "media".into(),
                        node_name: "Arctis_Media".into(),
                        description: "Media".into(),
                        output_device: None,
                        eq: vec![],
                        volume_db: 0.0,
                        muted: false,
                    },
                ],
                routes: vec![],
                mic: MicChainConfig::default(),
                surround: arctis_config::SurroundConfig {
                    enabled: true,
                    hrir: Some(hrir_stem.into()),
                    channels: vec!["game".into(), "media".into()],
                    hw_sink: None,
                },
                master_volume_db: 0.0,
                master_mute: false,
                chatmix_position: 4,
                default_sink_channel: None,
            }],
            eq_presets: vec![],
            dial_controls_balance: true,
        }
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
        // Total 58 calls: 4 (phase1) + 4*(1+10+1+1) (EQ+vol per channel) + 1 (mic) + 1 (surround)
        assert_eq!(calls.len(), 58, "expected 58 total pw-cli calls");
    }

    /// Queue outputs for reconcile with surround ENABLED and surround node absent.
    /// Engine::new seeds "aux" → 4 channels (game/chat/media/aux).
    /// Step6 enabled (apply_surround uses recreate in enabled path):
    ///   1. recreate() = remove() + create():
    ///      remove: source_exists() → 1 ls (absent, no destroy)
    ///      create: source_exists() → 1 ls absent → spawn (goes to spawned, not calls)
    ///   2. reroute "game": set_output → ls (source_exists: present) → find_node_id → destroy → pkill + ls (source absent → spawn)
    ///   3. reroute "media": same
    fn queue_reconcile_surround_enabled_absent(runner: MockRunner) -> MockRunner {
        let ls_channels = ls_all_present();
        let ls_surround_absent = ls_all_absent(); // no surround node
        let mut r = runner;
        // Phase 1: 4 ls (all channels present, including aux seeded by Engine::new)
        for _ in 0..4 {
            r = r.with_output(0, &ls_channels, "");
        }
        // Phase 2 + 2b interleaved: per channel (4), EQ apply then volume/mute apply
        for _ in 0..4 {
            // Phase 2: EQ apply (1 ls + 10 band sets)
            r = r.with_output(0, &ls_channels, "");
            for _ in 0..10 {
                r = r.with_output(0, "", "");
            }
            // Phase 2b: volume/mute apply (1 ls + 1 Props set)
            r = r.with_output(0, &ls_channels, ""); // find_node_id
            r = r.with_output(0, "", ""); // Props set
        }
        // Phase 5 (mic disabled): source_exists → 1 ls (no mic)
        r = r.with_output(0, &ls_surround_absent, "");
        // Phase 6 (surround enabled, absent) — recreate():
        //   remove: source_exists() → 1 ls (absent, no destroy needed)
        r = r.with_output(0, &ls_surround_absent, "");
        //   create: source_exists() → 1 ls absent → spawn (in spawned, not calls)
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
        };
        let engine = Engine::new(MockRunner::new(), cfg);
        let s = engine.state();

        assert!(!s.surround.enabled);
        assert_eq!(s.surround.hrir.as_deref(), Some("aa-first"));
        assert_eq!(s.surround.channels, vec!["game"]);
        assert_eq!(s.surround.hw_sink.as_deref(), Some("alsa_output.pci"));
        assert_eq!(s.surround.available_hrirs, vec!["aa-first", "zz-last"]);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("HOME");
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
        // recreate surround (absent):
        //   remove: source_exists → 1 ls (absent, no destroy)
        //   create: source_exists → 1 ls (absent) → spawn
        //
        // restore media (Arctis_Media present → destroy + pkill, then create absent → spawn):
        //   remove: sink_exists → 1 ls (present), find_node_id → 1 ls, destroy, pkill
        //   create: sink_exists → 1 ls (absent) → spawn
        let ls_channels = ls_all_present();
        let ls_absent = ls_all_absent();

        let runner = MockRunner::new()
            // recreate surround: remove source_exists (absent)
            .with_output(0, &ls_absent, "")
            // recreate surround: create source_exists (absent → spawn)
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
        };

        // Phase 1: surround_set_enabled(true) → apply_surround(enabled):
        //   recreate surround: remove (absent) + create (absent → spawn) = 2 ls + spawn
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
            // recreate surround: remove source_exists (absent)
            .with_output(0, &ls_absent, "")
            // recreate surround: create source_exists (absent → spawn)
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
        // recreate surround (absent):
        //   remove: source_exists → 1 ls (absent)
        //   create: source_exists → 1 ls (absent) → spawn
        // No channel operations.
        let ls_absent = ls_all_absent();

        let runner = MockRunner::new()
            // recreate surround: remove source_exists (absent)
            .with_output(0, &ls_absent, "")
            // recreate surround: create source_exists (absent → spawn)
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
    // Minor fix tests: surround tracker + prune
    // ─────────────────────────────────────────────

    /// Fix: remove_channel prunes surround.channels and surround_routed stays consistent.
    ///
    /// Setup: config has channels = ["game", "chat", "media"], surround.channels = ["game", "media"].
    /// surround_routed = {"game", "media"} (simulating prior enable).
    ///
    /// After remove_channel("media"):
    ///   - surround.channels must not contain "media"
    ///   - surround_routed tracker is unaffected by remove_channel itself (reconcile handles it),
    ///     but a subsequent apply_surround driven from the new config must not try to route "media"
    ///     (it no longer exists in channels) and must restore it from surround_routed → tracker
    ///     drains "media" correctly.
    #[test]
    fn remove_channel_prunes_surround_channels_and_tracker_stays_consistent() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("rm_ch_surr_prune");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Config: surround enabled, surround.channels = ["game", "media"],
        // actual channels = ["game", "chat", "media"].
        let cfg = make_config_surround_enabled("unused-hrir"); // channels: game, chat, media; surround.channels: game, media

        // remove_channel("media") → AudioBackend::remove:
        //   sink_exists → 1 ls (present), find_node_id → 1 ls, destroy, pkill
        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // sink_exists
            .with_output(0, &ls, "") // find_node_id
            .with_output(0, "", "") // pw-cli destroy
            .with_output(1, "", ""); // pkill (exit 1 — ignored)

        let mut engine = Engine::new(runner, cfg);
        // Simulate prior surround enable: both channels tracked.
        engine.surround_routed.insert("game".into());
        engine.surround_routed.insert("media".into());

        engine
            .remove_channel("media")
            .expect("remove_channel must succeed");

        // surround.channels must no longer reference the deleted channel.
        let surr_channels = &engine.config.active().unwrap().surround.channels;
        assert!(
            !surr_channels.contains(&"media".to_string()),
            "surround.channels must not reference deleted channel 'media': {surr_channels:?}"
        );
        assert!(
            surr_channels.contains(&"game".to_string()),
            "surround.channels must still contain 'game': {surr_channels:?}"
        );

        // The actual channels list must also not contain "media".
        let profile = engine.config.active().unwrap();
        assert!(
            !profile.channels.iter().any(|c| c.id == "media"),
            "channels list must not contain removed channel"
        );

        // surround_routed tracker: remove_channel doesn't touch it (reconcile/apply_surround
        // will clean it up on next pass). That is the correct contract — verify it's still
        // populated so a subsequent apply_surround can restore cleanly.
        assert!(
            engine.surround_routed.contains("media"),
            "surround_routed retains 'media' until next apply_surround pass"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
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
        let dump = include_str!("../../audio/tests/fixtures/pw_dump_app_streams.json");
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
        let dump = include_str!("../../audio/tests/fixtures/pw_dump_app_streams.json");
        // Queue: (1) list_streams pw-dump, (2) pw-metadata move.
        // set_route's apply_live pw-dump (call 3) gets the MockRunner default empty
        // output (status=0, stdout="") — parse fails silently, best-effort ignored.
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
                .any(|c| c.get(0).map(|s| s.as_str()) == Some("pw-metadata")),
            "pw-metadata must be called for live move"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&tmp_home);
        std::env::remove_var("ASM_CONFIG_HOME");
        std::env::remove_var("HOME");
    }

    #[test]
    fn move_stream_unknown_channel_errors() {
        let cfg = make_config_no_eq_no_routes();
        let dump = include_str!("../../audio/tests/fixtures/pw_dump_app_streams.json");
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

    // ─────────────────────────────────────────────
    // Task 8 — ensure_standard_channels seed + mixer methods
    // ─────────────────────────────────────────────

    #[test]
    fn engine_new_seeds_standard_channels_for_old_profile() {
        // Simulate a legacy profile that only has game/chat/media (no aux).
        let mut cfg = make_config_no_eq_no_routes(); // game/chat/media
        cfg.profiles[0].channels.retain(|c| c.id != "aux");
        // Engine::new calls ensure_standard_channels() → aux is added.
        let engine = Engine::new(arctis_audio::MockRunner::new(), cfg);
        let st = engine.state();
        assert!(st.channels.iter().any(|c| c.id == "aux"), "aux auto-seeded on load");
    }

    #[test]
    fn set_master_volume_persists_and_reports() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("master_vol");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        // wpctl call for the gain (status 0).
        let runner = arctis_audio::MockRunner::new().with_output(0, "", "");
        let mut engine = Engine::new(runner, cfg);
        engine.set_master_volume(-6.0).unwrap();
        assert_eq!(engine.state().master_volume_db, -6.0);

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
}
