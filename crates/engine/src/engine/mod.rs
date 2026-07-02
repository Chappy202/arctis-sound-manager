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
mod reconcile;
mod routing;
mod surround;
mod volume;

pub use reconcile::{plan_reconcile, ReconcileStep};
pub use surround::resolve_effective_mode;
pub use volume::mix_to_chatmix_position;
pub(crate) use surround::{hrir_entries_for, surround_mode_str};

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
