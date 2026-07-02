use crate::{children::ChildOwner, convert, error::EngineError, state::Event};
use arctis_audio::{
    move_stream_argv, parse_app_streams, parse_node_volume, query_pw_version,
    richest_surround_input, supports_builtin_noisegate, AppMatch, AudioBackend, ChannelManager,
    CommandRunner, EqModel, FsPluginProbe, MicBackend, PluginProbe, Router, StageKind,
    SurroundBackend, DEEPFILTER_PLUGIN_BASENAME, RNNOISE_PLUGIN_BASENAME,
};
use arctis_config::{Config, EqBandConfig, SurroundMode};
use arctis_domain::{
    db_to_volume_pct_cubic, MIC_ATTEN_LIMIT_MAX_DB, MIC_ATTEN_LIMIT_MIN_DB,
    MIC_COMP_MAKEUP_MAX_DB, MIC_COMP_MAKEUP_MIN_DB, MIC_COMP_RATIO_MAX, MIC_COMP_RATIO_MIN,
    MIC_COMP_THRESHOLD_MAX_DB, MIC_COMP_THRESHOLD_MIN_DB, MIC_GAIN_MAX_DB, MIC_GAIN_MIN_DB,
    MIC_GATE_THRESHOLD_MAX, MIC_GATE_THRESHOLD_MIN, MIC_HIGHPASS_MAX_HZ, MIC_HIGHPASS_MIN_HZ,
    MIC_VAD_GRACE_MAX_MS, MIC_VAD_GRACE_MIN_MS, MIC_VAD_RETRO_GRACE_MAX_MS,
    MIC_VAD_RETRO_GRACE_MIN_MS, MIC_VAD_THRESHOLD_MAX, MIC_VAD_THRESHOLD_MIN,
};
use std::sync::Arc;

pub use crate::state::MicParam;

#[cfg(test)]
mod test_support;

mod channels;
mod device_ctl;
mod eq;
mod mic;
mod profiles;
mod routing;
mod surround;
mod volume;

pub use surround::resolve_effective_mode;
pub use volume::mix_to_chatmix_position;
pub(crate) use surround::{hrir_entries_for, surround_mode_str};

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

/// How long a `pw-dump` snapshot taken for the live volume read in `state()`
/// stays reusable. Bounds the subprocess rate to at most one per window even
/// when the GUI commits volume at its ~80 ms drag throttle. Kept under the
/// 2 s GUI state-poll interval so each poll still re-reads fresh, and the
/// VolumeSlider's reconcile-guard ignores incoming volume mid-drag, so the
/// bounded staleness is not user-visible.
const VOLUME_DUMP_TTL: std::time::Duration = std::time::Duration::from_millis(1000);

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
    /// Cached `pw-dump` stdout for the live volume read in `state()`, with the instant it was taken.
    /// `None` means no cache exists. Invalidated by TTL expiry only (not on volume writes).
    volume_dump_cache: Option<(std::time::Instant, String)>,
    /// Timestamp of the most recent successful volume write (`set_channel_volume`,
    /// `set_master_volume`, `set_mic_volume`). Used in `state()` to detect when the cached
    /// pw-dump snapshot predates the write, so the just-written config value is reported instead
    /// of the stale live value — prevents a post-commit thumb snap-back without re-spawning
    /// pw-dump on every throttled drag commit (preserves A4 no-subprocess-per-drag property).
    last_volume_write: Option<std::time::Instant>,
    /// Set by `apply_surround` when a pinned HRIR stem was missing and a fallback was
    /// substituted; surfaced in `state().surround.hrir_missing` so the UI can prompt to import.
    surround_hrir_missing: Option<String>,
    /// True when the persisted config could not be read at startup and the engine is
    /// running on defaults. Surfaced via `state().config_degraded` so clients can warn.
    config_degraded: bool,
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
            volume_dump_cache: None,
            last_volume_write: None,
            surround_hrir_missing: None,
            config_degraded: false,
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
            volume_dump_cache: None,
            last_volume_write: None,
            surround_hrir_missing: None,
            config_degraded: false,
        }
    }

    /// Pre-seed the PipeWire version so `ensure_pw_version()` is a no-op during tests.
    /// Avoids adding an extra runner call that exact-call-count tests don't expect.
    #[cfg(test)]
    pub fn seed_pw_version(&mut self, version: (u32, u32, u32)) {
        self.pw_version = Some(version);
        self.builtin_noisegate = supports_builtin_noisegate(version);
    }

    /// Clear the `pw-dump` volume cache so the next `state()` call forces a fresh subprocess.
    /// For tests only — avoids sleeping to expire the TTL.
    #[cfg(test)]
    pub fn expire_volume_cache(&mut self) {
        self.volume_dump_cache = None;
    }

    /// Query (once) and cache the PipeWire version. Sets `builtin_noisegate` based on version.
    fn ensure_pw_version(&mut self) {
        if self.pw_version.is_none() {
            self.pw_version = query_pw_version(&mut self.runner);
            self.builtin_noisegate =
                supports_builtin_noisegate(self.pw_version.unwrap_or((0, 0, 0)));
        }
    }

    /// Mark the engine as running on a default config because the persisted one
    /// was unreadable at startup (see the daemon's load path). Surfaced in state().
    pub fn set_config_degraded(&mut self, degraded: bool) {
        self.config_degraded = degraded;
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

    /// Live `pw-dump` stdout for the volume read, cached for `VOLUME_DUMP_TTL`.
    /// On a fresh cache miss runs `pw-dump`; empties to `""` on failure (callers
    /// fall back to config `volume_pct`). Reuses the cached value within the TTL.
    fn volume_dump(&mut self) -> String {
        if let Some((taken, json)) = &self.volume_dump_cache {
            if taken.elapsed() < VOLUME_DUMP_TTL {
                return json.clone();
            }
        }
        let json = self
            .runner
            .run("pw-dump", &[])
            .ok()
            .filter(|o| o.status == 0)
            .map(|o| o.stdout.clone())
            .unwrap_or_default();
        self.volume_dump_cache = Some((std::time::Instant::now(), json.clone()));
        json
    }

    /// Return a flat UI-agnostic snapshot of the current engine state.
    pub fn state(&mut self) -> crate::state::EngineState {
        use crate::state::{
            ChannelSnapshot, EngineState, EqBandSnapshot, EqPresetSnapshot, MicPresetSnapshot,
            MicSnapshot, MicStageSnapshot, StageName, SuppressionBackend,
        };
        // Best-effort live volume read from pw-dump (completes before any config borrow).
        // Uses a short TTL cache to bound the subprocess rate on the hot path (e.g. drag commits).
        // On failure or empty output, falls back to persisted config values per channel.
        let pw_dump_json = self.volume_dump();

        // Use the live pw-dump channel volume only when the cached snapshot was taken AT/AFTER
        // our last volume write; otherwise the cache is stale w.r.t. a just-applied value, so
        // report the config value (avoids a post-commit thumb snap-back without re-spawning
        // pw-dump on every throttled drag commit — preserves A4 no-subprocess-per-drag property).
        let cache_taken = self.volume_dump_cache.as_ref().map(|(t, _)| *t);
        let use_live = match (cache_taken, self.last_volume_write) {
            (Some(taken), Some(written)) => taken >= written,
            (Some(_), None) => true, // never wrote → live is authoritative
            _ => false,              // no cache → fall back to config
        };

        let active = self.config.active().ok();
        let channels = active
            .map(|p| {
                p.channels
                    .iter()
                    .map(|ch| {
                        // Use live sink volume when pw-dump is authoritative; fallback to config.
                        // When use_live is false the cache predates our last write, so the config
                        // value (just persisted) is more accurate than the stale snapshot.
                        let volume_pct = if use_live {
                            parse_node_volume(&pw_dump_json, &ch.node_name).unwrap_or(ch.volume_pct)
                        } else {
                            ch.volume_pct
                        };
                        ChannelSnapshot {
                            id: ch.id.clone(),
                            node_name: ch.node_name.clone(),
                            output_device: ch.output_device.clone(),
                            eq_bands: convert::dense_eq_bands(ch)
                                .iter()
                                .map(|b| EqBandSnapshot {
                                    kind: b.kind.clone(),
                                    freq_hz: b.freq_hz,
                                    q: b.q,
                                    gain_db: b.gain_db,
                                })
                                .collect(),
                            volume_pct,
                            muted: ch.muted,
                        }
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
                    // Default to AVAILABLE when there's no reconcile entry. A disabled
                    // stage isn't probed during the chain build, so it has no entry —
                    // defaulting to false made a just-disabled stage look "unavailable",
                    // which greyed its card and locked the enable toggle (trapping it off).
                    // A genuinely missing plugin is still recorded as false while enabled.
                    let avail = avail_map
                        .get(&StageName::Suppression)
                        .copied()
                        .unwrap_or(true);
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
                    // See Suppression above: default to available when not probed.
                    let avail = avail_map
                        .get(&StageName::Compressor)
                        .copied()
                        .unwrap_or(true);
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
                    // See Suppression above: default to available when not probed.
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
            // Dense 10-band mic EQ: canonical defaults overlaid with stored mic bands.
            let eq_bands: Vec<EqBandSnapshot> = {
                let mut dense = convert::default_eq_band_configs();
                for (i, b) in mc.eq.iter().enumerate().take(dense.len()) {
                    dense[i] = b.clone();
                }
                dense
                    .iter()
                    .map(|b| EqBandSnapshot {
                        kind: b.kind.clone(),
                        freq_hz: b.freq_hz,
                        q: b.q,
                        gain_db: b.gain_db,
                    })
                    .collect()
            };

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
                volume_pct: mc.volume_pct,
            }
        } else {
            MicSnapshot::default()
        };

        let surround_hrir_missing = self.surround_hrir_missing.clone();
        let surround = if let Ok(p) = self.config.active() {
            let sc = &p.surround;
            let (available_hrirs, available_hrir_entries) = convert::hrir_base_dir()
                .map(|base| {
                    let hrirs = convert::available_hrirs(&base);
                    let entries = hrir_entries_for(&base);
                    (hrirs, entries)
                })
                .unwrap_or_default();
            // Probe the negotiated input layout of whatever app feeds a surround
            // channel (reuses the cached pw-dump; read-only). Richest source wins.
            let surround_sinks: Vec<String> = p
                .channels
                .iter()
                .filter(|c| sc.channels.iter().any(|id| id == &c.id))
                .map(|c| c.node_name.clone())
                .collect();
            let neg = parse_app_streams(&pw_dump_json)
                .ok()
                .and_then(|streams| richest_surround_input(&streams, &surround_sinks));
            let (negotiated_channels, negotiated_surround) = match &neg {
                Some(si) => (Some(si.channels), Some(si.is_true_surround)),
                None => (None, None),
            };
            crate::state::SurroundSnapshot {
                enabled: sc.enabled,
                hrir: sc.hrir.clone(),
                available_hrirs,
                available_hrir_entries,
                channels: sc.channels.clone(),
                hw_sink: sc.hw_sink.clone(),
                mode: surround_mode_str(sc.mode).to_string(),
                // Feed the PROBED negotiated channel count so Auto reports what the
                // DSP actually does (bypass for stereo input, HRIR for surround) —
                // hardcoding None made Auto always claim HRIR 7.1.
                effective_mode: surround_mode_str(resolve_effective_mode(
                    sc.mode,
                    negotiated_channels,
                ))
                .to_string(),
                negotiated_channels,
                negotiated_surround,
                hrir_missing: surround_hrir_missing,
                blocksize: sc.blocksize,
                tailsize: sc.tailsize,
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

        let factory_eq_presets = crate::presets::factory_eq_presets()
            .iter()
            .map(|p| EqPresetSnapshot {
                name: p.name.clone(),
                band_count: p.bands.len(),
            })
            .collect();

        let mic_presets = crate::presets::factory_mic_presets()
            .into_iter()
            .map(|p| MicPresetSnapshot {
                name: p.name,
                description: p.description,
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
            master_volume_pct: active.as_ref().map(|p| p.master_volume_pct).unwrap_or(100),
            factory_eq_presets,
            mic_presets,
            master_mute: active.as_ref().map(|p| p.master_mute).unwrap_or(false),
            chatmix_position: active.as_ref().map(|p| p.chatmix_position).unwrap_or(4),
            default_sink_channel: active.as_ref().and_then(|p| p.default_sink_channel.clone()),
            dial_controls_balance: self.config.dial_controls_balance,
            knob_controls_master: self.config.knob_controls_master,
            config_degraded: self.config_degraded,
        }
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
    use super::test_support::*;
    use arctis_audio::MockRunner;
    use arctis_config::{ChannelConfig, Config, MicChainConfig, Profile, RouteConfig};

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

    #[test]
    fn state_reflects_active_profile() {
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let s = engine.state();
        assert_eq!(s.active_profile, "default");
        // Engine::new calls ensure_standard_channels() which adds "aux" to the 3-channel config.
        assert_eq!(s.channels.len(), 4);
        assert!(s.profiles.contains(&"default".to_string()));
    }

    #[test]
    fn state_includes_full_eq_band_values_from_config() {
        let cfg = make_config_with_eq_bands();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let s = engine.state();

        // Find game channel
        let game = s
            .channels
            .iter()
            .find(|c| c.id == "game")
            .expect("game channel");
        // Dense model: 10 bands (2 config overrides at index 0+1, canonical defaults for 2..9).
        assert_eq!(game.eq_bands.len(), 10, "game should have 10 dense EQ bands");

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
        // Dense model: flat channel reports 10 canonical default bands.
        assert_eq!(chat.eq_bands.len(), 10, "flat channel emits 10 dense default bands");
        assert_eq!(chat.eq_bands[0].freq_hz, 31.0);
        assert_eq!(chat.eq_bands[9].freq_hz, 16000.0);
        assert!(chat.eq_bands.iter().all(|b| b.gain_db == 0.0));
        // Dense defaults use shelves at the extremes, peaking in the middle.
        assert_eq!(chat.eq_bands[0].kind, "lowshelf");
        assert_eq!(chat.eq_bands[9].kind, "highshelf");
        assert!(chat.eq_bands[1..9].iter().all(|b| b.kind == "peaking"));
    }

    #[test]
    fn state_channel_snapshot_has_output_device() {
        let cfg = make_config_with_eq_bands();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let s = engine.state();
        let chat = s.channels.iter().find(|c| c.id == "chat").unwrap();
        assert_eq!(chat.output_device, Some("alsa_output.headphones".into()));
        let game = s.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(game.output_device, None);
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
        let mut engine = Engine::new(arctis_audio::MockRunner::new(), cfg);
        let st = engine.state();
        assert!(st.channels.iter().any(|c| c.id == "aux"), "aux auto-seeded on load");
    }
}
