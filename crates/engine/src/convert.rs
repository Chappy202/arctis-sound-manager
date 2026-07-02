use crate::error::EngineError;
use crate::state::StageAvailability;
use arctis_audio::{
    BandKind, ChainChannels, ChainKind, ChainSpec, ChannelDef, ChannelSetConfig, EqBand, EqModel,
    FilterNode, NodeType, PluginProbe, RouteRule, DEEPFILTER_LABEL_MONO,
    DEEPFILTER_PLUGIN_BASENAME, GATE_LABEL, GATE_PLUGIN_BASENAME, LIMITER_LABEL,
    LIMITER_PLUGIN_BASENAME, RNNOISE_LABEL_MONO, RNNOISE_PLUGIN_BASENAME, SC4M_LABEL,
    SC4M_PLUGIN_BASENAME,
};
use arctis_config::{ChannelConfig, EqBandConfig, MicChainConfig, RouteConfig, SuppressionBackend};

/// Parse a config-layer band kind string into an audio-layer `BandKind`.
pub fn band_kind_from_str(s: &str) -> Result<BandKind, EngineError> {
    match s {
        "peaking" => Ok(BandKind::Peaking),
        "lowshelf" => Ok(BandKind::LowShelf),
        "highshelf" => Ok(BandKind::HighShelf),
        other => Err(EngineError::Reconcile(format!(
            "unknown EQ band kind: {other:?} (expected \"peaking\", \"lowshelf\", or \"highshelf\")"
        ))),
    }
}

/// Inverse of `band_kind_from_str`: an audio-layer `BandKind` → config kind string.
pub fn band_kind_to_str(kind: BandKind) -> &'static str {
    match kind {
        BandKind::Peaking => "peaking",
        BandKind::LowShelf => "lowshelf",
        BandKind::HighShelf => "highshelf",
    }
}

/// Map a single `EqBandConfig` (from the config layer) to an `EqBand` (audio layer).
pub fn eq_band_from_cfg(c: &EqBandConfig) -> Result<EqBand, EngineError> {
    let kind = band_kind_from_str(&c.kind)?;
    Ok(EqBand::new(kind, c.freq_hz, c.q, c.gain_db))
}

/// The canonical dense default 10-band set, in config form (kind strings).
/// Frequencies come from `EqModel::default_10band()` (single source of truth).
pub fn default_eq_band_configs() -> Vec<arctis_config::EqBandConfig> {
    arctis_audio::EqModel::default_10band()
        .bands
        .iter()
        .map(|b| arctis_config::EqBandConfig {
            kind: band_kind_to_str(b.kind).to_string(),
            freq_hz: b.freq_hz,
            q: b.q,
            gain_db: b.gain_db,
        })
        .collect()
}

/// A dense, fixed-length (10) band vector for a channel: canonical defaults with
/// any stored overrides overlaid by index. Never empty, never `1000 Hz` padding.
pub fn dense_eq_bands(channel: &arctis_config::ChannelConfig) -> Vec<arctis_config::EqBandConfig> {
    let mut dense = default_eq_band_configs();
    for (i, b) in channel.eq.iter().enumerate().take(dense.len()) {
        dense[i] = b.clone();
    }
    dense
}

/// Build an `EqModel` for a channel config — ALWAYS the dense 10-band model
/// (canonical defaults overlaid with stored overrides), so the live filter
/// chain has a stable 10 biquads and `set_eq_band` can always target any band.
pub fn eq_model_for(channel: &ChannelConfig) -> Result<EqModel, EngineError> {
    let bands = dense_eq_bands(channel)
        .iter()
        .map(eq_band_from_cfg)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(EqModel { bands })
}

/// Map a `ChannelConfig` (from the config layer) to a `ChannelDef` (audio layer).
pub fn channel_def_from_cfg(c: &ChannelConfig) -> ChannelDef {
    ChannelDef::new(&c.id, &c.node_name, &c.description, c.output_device.clone())
}

/// True when `ch_id` is routed through the surround convolver in this profile:
/// surround is enabled AND the channel is listed in `surround.channels`. This is the
/// single source of truth for surround routing — used both to size the channel sink
/// (8-ch 7.1) AND to keep `reconcile` Step 3 from re-pointing the channel to its
/// `output_device` (the live convolver routing is owned by `apply_surround`; without
/// this guard the two writers fight and silently bypass the HRIR — bug C1).
pub fn surround_routes_channel(surround: &arctis_config::SurroundConfig, ch_id: &str) -> bool {
    surround.enabled && surround.channels.iter().any(|c| c == ch_id)
}

/// The channel-sink layout for a surround-routed channel, by surround mode. It MUST
/// match the convolver/bypass INPUT for that mode so the link is 1:1 (no implicit
/// PipeWire remix): 7.1 → 8-ch, 5.1 → 6-ch, StereoBypass → 2-ch. `Auto` resolves to
/// 7.1 (mirrors `resolve_effective_mode(Auto, None)`).
pub fn surround_channel_layout(mode: arctis_config::SurroundMode) -> ChainChannels {
    use arctis_config::SurroundMode::*;
    match mode {
        Hrir71 | Auto => ChainChannels::Surround71,
        Hrir51 => ChainChannels::Surround51,
        StereoBypass => ChainChannels::Stereo,
    }
}

/// Build the full `ChannelSetConfig` for a profile's channels.
///
/// Reusable, profile-agnostic surround: any channel routed through surround (see
/// [`surround_routes_channel`]) is built as an 8-channel 7.1 sink so a game outputs
/// discrete surround into it, which then feeds the HRIR convolver. Every other
/// channel stays stereo. No game/profile is hard-wired — membership drives it.
pub fn channel_set_from_profile(p: &arctis_config::Profile) -> ChannelSetConfig {
    ChannelSetConfig {
        channels: p
            .channels
            .iter()
            .map(|c| {
                let mut def = channel_def_from_cfg(c);
                if surround_routes_channel(&p.surround, &c.id) {
                    def.channels = surround_channel_layout(p.surround.mode);
                }
                def
            })
            .collect(),
    }
}

/// Build the `RouteRule` vec for a profile's routing config.
pub fn route_rules_from_profile(p: &arctis_config::Profile) -> Vec<RouteRule> {
    p.routes
        .iter()
        .map(|r: &RouteConfig| RouteRule::new(&r.app_binary, &r.target_sink))
        .collect()
}

// ─── Mic chain convert helpers ───────────────────────────────────────────────

/// Convert gain in dB to a linear multiplier.
/// Used for the `linear` builtin node's `Mult` control.
pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Convert a linear amplitude to dB, clamped to [-70, +20] dB.
/// Used for swh gate_1410 Threshold conversion from linear to dB.
pub fn linear_to_db(x: f32) -> f32 {
    if x <= 0.0 {
        -70.0
    } else {
        (20.0 * x.log10()).clamp(-70.0, 20.0)
    }
}

/// Build the `ChainSpec` for the Clean Mic virtual source.
///
/// node_name = `arctis_clean_mic`, capture target = `cfg.hw_mic`, mono.
/// capture.props gets `node.passive = true` + `target.object` (when pinned) and
/// no `media.class`; playback.props gets `media.class = Audio/Source` to expose
/// the virtual source to applications.
pub fn mic_chain_spec(cfg: &MicChainConfig) -> ChainSpec {
    ChainSpec {
        node_name: "arctis_clean_mic".to_string(),
        description: "Clean Mic".to_string(),
        channels: ChainChannels::Mono,
        kind: ChainKind::Source,
        capture_node_name: "arctis_clean_mic.capture".to_string(),
        capture_target: cfg.hw_mic.clone(),
        playback_target: None,
        playback_node_name: "arctis_clean_mic".to_string(),
    }
}

/// Build the `FilterNode` list for the mic chain plus stage availability info.
///
/// Walks the fixed stage order: gain → highpass → suppression → gate → compressor →
/// mic-EQ → limiter. The gate sits BEFORE the compressor so it keys on the true
/// noise floor — after the compressor it would key on the makeup-gain-raised floor
/// and never fully close. LADSPA stages (suppression, compressor, limiter) are only
/// included if `probe.ladspa_available(path)`; otherwise they are skipped and
/// recorded as unavailable.
/// Gate uses the builtin noisegate when `builtin_noisegate = true` (PW ≥ 1.6),
/// otherwise falls back to LADSPA gate_1410.
/// The limiter is an always-on −1 dBFS output ceiling (hard_limiter_1413), not a
/// user-toggled stage; it degrades gracefully when the plugin is missing.
///
/// If no enabled+available node results, emits a single passthrough `linear` node so
/// `render_chain_conf` never sees an empty node list.
// 0.7071 is the deliberate Butterworth Q literal (matches presets.rs style).
#[allow(clippy::approx_constant)]
pub fn mic_chain_nodes(
    cfg: &MicChainConfig,
    probe: &dyn PluginProbe,
    builtin_noisegate: bool,
) -> (Vec<FilterNode>, Vec<StageAvailability>) {
    let mut nodes: Vec<FilterNode> = Vec::new();
    let mut availability: Vec<StageAvailability> = Vec::new();

    // ── Gain stage (builtin linear) ──────────────────────────────────────────
    if cfg.gain.enabled {
        let mult = db_to_linear(cfg.gain.gain_db);
        nodes.push(FilterNode {
            name: "mic_gain".to_string(),
            node_type: NodeType::Builtin,
            label: "linear".to_string(),
            plugin: None,
            port_in: "In".to_string(),
            port_out: "Out".to_string(),
            controls: vec![("Mult".to_string(), mult), ("Add".to_string(), 0.0)],
        });
        availability.push(StageAvailability {
            stage: crate::state::StageName::Gain,
            available: true,
            requested: true,
        });
    }

    // ── Highpass stage (builtin bq_highpass) ─────────────────────────────────
    if cfg.highpass.enabled {
        nodes.push(FilterNode {
            name: "mic_highpass".to_string(),
            node_type: NodeType::Builtin,
            label: "bq_highpass".to_string(),
            plugin: None,
            port_in: "In".to_string(),
            port_out: "Out".to_string(),
            controls: vec![
                ("Freq".to_string(), cfg.highpass.freq_hz),
                ("Q".to_string(), 0.7071),
                ("Gain".to_string(), 0.0),
            ],
        });
        availability.push(StageAvailability {
            stage: crate::state::StageName::Highpass,
            available: true,
            requested: true,
        });
    }

    // ── Suppression stage (DeepFilterNet or RNNoise LADSPA) ──────────────────
    if cfg.suppression.enabled {
        let (plugin_bn, label, port_in, port_out, controls): (
            &str,
            &str,
            &str,
            &str,
            Vec<(String, f32)>,
        ) = match cfg.suppression.backend {
            SuppressionBackend::DeepFilter => (
                DEEPFILTER_PLUGIN_BASENAME,
                DEEPFILTER_LABEL_MONO,
                "Audio In",
                "Audio Out",
                vec![
                    (
                        "Attenuation Limit (dB)".to_string(),
                        cfg.suppression.attenuation_limit_db,
                    ),
                    // Slight spectral post-filter for extra suppression on
                    // stationary noise (port verified with analyseplugin;
                    // range 0..=0.05, plugin default 0).
                    ("Post Filter Beta".to_string(), 0.02),
                ],
            ),
            SuppressionBackend::Rnnoise => (
                RNNOISE_PLUGIN_BASENAME,
                RNNOISE_LABEL_MONO,
                "Input",
                "Output",
                vec![
                    (
                        "VAD Threshold (%)".to_string(),
                        cfg.suppression.vad_threshold,
                    ),
                    (
                        "VAD Grace Period (ms)".to_string(),
                        cfg.suppression.vad_grace_ms,
                    ),
                    (
                        "Retroactive VAD Grace (ms)".to_string(),
                        cfg.suppression.vad_retro_grace_ms,
                    ),
                ],
            ),
        };

        if probe.ladspa_available(plugin_bn) {
            nodes.push(FilterNode {
                name: "mic_suppression".to_string(),
                node_type: NodeType::Ladspa,
                label: label.to_string(),
                plugin: Some(plugin_bn.to_string()),
                port_in: port_in.to_string(),
                port_out: port_out.to_string(),
                controls,
            });
            availability.push(StageAvailability {
                stage: crate::state::StageName::Suppression,
                available: true,
                requested: true,
            });
        } else {
            availability.push(StageAvailability {
                stage: crate::state::StageName::Suppression,
                available: false,
                requested: true,
            });
        }
    }

    // ── Gate stage (builtin noisegate ≥1.6 or LADSPA gate_1410 fallback) ─────
    // BEFORE the compressor so the gate keys on the true noise floor, not the
    // makeup-gain-raised one.
    if cfg.gate.enabled {
        if builtin_noisegate {
            nodes.push(FilterNode {
                name: "mic_gate".to_string(),
                node_type: NodeType::Builtin,
                label: "noisegate".to_string(),
                plugin: None,
                port_in: "In".to_string(),
                port_out: "Out".to_string(),
                controls: vec![
                    ("Open Threshold".to_string(), cfg.gate.threshold),
                    // −6 dB hysteresis (0.5×): 0.9× left only ~1 dB between open
                    // and close, so the gate chattered around the threshold.
                    ("Close Threshold".to_string(), cfg.gate.threshold * 0.5),
                    ("Attack (s)".to_string(), 0.005),
                    ("Hold (s)".to_string(), 0.050),
                    ("Release (s)".to_string(), 0.100),
                ],
            });
            availability.push(StageAvailability {
                stage: crate::state::StageName::Gate,
                available: true,
                requested: true,
            });
        } else if probe.ladspa_available(GATE_PLUGIN_BASENAME) {
            nodes.push(FilterNode {
                name: "mic_gate".to_string(),
                node_type: NodeType::Ladspa,
                label: GATE_LABEL.to_string(),
                plugin: Some(GATE_PLUGIN_BASENAME.to_string()),
                port_in: "Input".to_string(),
                port_out: "Output".to_string(),
                controls: vec![
                    (
                        "Threshold (dB)".to_string(),
                        linear_to_db(cfg.gate.threshold),
                    ),
                    ("Attack (ms)".to_string(), 10.0),
                    ("Hold (ms)".to_string(), 100.0),
                    ("Decay (ms)".to_string(), 200.0),
                    // Duck by 30 dB instead of hard-muting (−90 dB): a closed
                    // gate that goes fully silent sounds like a dropped call.
                    ("Range (dB)".to_string(), -30.0),
                    (
                        "Output select (-1 = key listen, 0 = gate, 1 = bypass)".to_string(),
                        0.0,
                    ),
                ],
            });
            availability.push(StageAvailability {
                stage: crate::state::StageName::Gate,
                available: true,
                requested: true,
            });
        } else {
            availability.push(StageAvailability {
                stage: crate::state::StageName::Gate,
                available: false,
                requested: true,
            });
        }
    }

    // ── Compressor stage (LADSPA sc4m), after the gate ───────────────────────
    if cfg.compressor.enabled {
        if probe.ladspa_available(SC4M_PLUGIN_BASENAME) {
            nodes.push(FilterNode {
                name: "mic_compressor".to_string(),
                node_type: NodeType::Ladspa,
                label: SC4M_LABEL.to_string(),
                plugin: Some(SC4M_PLUGIN_BASENAME.to_string()),
                port_in: "Input".to_string(),
                port_out: "Output".to_string(),
                // Every voicing-relevant port is emitted explicitly — the LADSPA
                // hint defaults (attack ~101 ms, release ~401 ms) are far too slow
                // for speech and must never be relied on.
                controls: vec![
                    ("RMS/peak".to_string(), 0.0),
                    ("Attack time (ms)".to_string(), cfg.compressor.attack_ms),
                    ("Release time (ms)".to_string(), cfg.compressor.release_ms),
                    (
                        "Threshold level (dB)".to_string(),
                        cfg.compressor.threshold_db,
                    ),
                    ("Ratio (1:n)".to_string(), cfg.compressor.ratio),
                    ("Knee radius (dB)".to_string(), 3.0),
                    ("Makeup gain (dB)".to_string(), cfg.compressor.makeup_db),
                ],
            });
            availability.push(StageAvailability {
                stage: crate::state::StageName::Compressor,
                available: true,
                requested: true,
            });
        } else {
            availability.push(StageAvailability {
                stage: crate::state::StageName::Compressor,
                available: false,
                requested: true,
            });
        }
    }

    // ── Mic EQ bands (builtin biquad) ────────────────────────────────────────
    if cfg.eq_enabled && !cfg.eq.is_empty() {
        for (i, band) in cfg.eq.iter().enumerate() {
            let label = match band.kind.as_str() {
                "lowshelf" => "bq_lowshelf",
                "highshelf" => "bq_highshelf",
                _ => "bq_peaking",
            };
            nodes.push(FilterNode {
                name: format!("mic_eq_band_{i}"),
                node_type: NodeType::Builtin,
                label: label.to_string(),
                plugin: None,
                port_in: "In".to_string(),
                port_out: "Out".to_string(),
                controls: vec![
                    ("Freq".to_string(), band.freq_hz),
                    ("Q".to_string(), band.q),
                    ("Gain".to_string(), band.gain_db),
                ],
            });
        }
        availability.push(StageAvailability {
            stage: crate::state::StageName::MicEq,
            available: true,
            requested: true,
        });
    }

    // ── Output limiter (always on): −1 dBFS ceiling as the FINAL stage ───────
    // Protects downstream (Discord/OBS/encoders) from clipping regardless of
    // gain/makeup settings. Not user-toggled; degrades gracefully when the
    // plugin is missing (chain builds without it, reported unavailable).
    if probe.ladspa_available(LIMITER_PLUGIN_BASENAME) {
        nodes.push(FilterNode {
            name: "mic_limiter".to_string(),
            node_type: NodeType::Ladspa,
            label: LIMITER_LABEL.to_string(),
            plugin: Some(LIMITER_PLUGIN_BASENAME.to_string()),
            port_in: "Input".to_string(),
            port_out: "Output".to_string(),
            controls: vec![
                ("dB limit".to_string(), -1.0),
                ("Wet level".to_string(), 1.0),
                ("Residue level".to_string(), 0.0),
            ],
        });
        availability.push(StageAvailability {
            stage: crate::state::StageName::Limiter,
            available: true,
            requested: true,
        });
    } else {
        availability.push(StageAvailability {
            stage: crate::state::StageName::Limiter,
            available: false,
            requested: true,
        });
    }

    // ── Passthrough fallback: ensure nodes is never empty ────────────────────
    if nodes.is_empty() {
        nodes.push(FilterNode {
            name: "mic_gain".to_string(),
            node_type: NodeType::Builtin,
            label: "linear".to_string(),
            plugin: None,
            port_in: "In".to_string(),
            port_out: "Out".to_string(),
            controls: vec![("Mult".to_string(), 1.0), ("Add".to_string(), 0.0)],
        });
    }

    (nodes, availability)
}

/// Build the mic EQ band node name (stable for live Props addressing).
pub fn mic_eq_band_node_name(index: usize) -> String {
    format!("mic_eq_band_{index}")
}

// ─── Surround convert helpers ────────────────────────────────────────────────

use arctis_audio::SurroundSpec;
use arctis_config::SurroundConfig;
use std::path::PathBuf;

pub const HRIR_BASE_SUBPATH: &str = ".local/share/pipewire/hrir_hesuvi";

/// Return the HRIR base directory derived from the `HOME` environment variable.
///
/// Call sites (engine::state, engine methods) call this ONCE and pass the
/// resulting `PathBuf` down into `resolve_hrir_path` / `available_hrirs`, which
/// are fully injected (no env reads inside).  Tests bypass this function entirely
/// and pass a temp dir directly — eliminating the `$HOME` read from
/// parallel-raced test paths.
pub fn hrir_base_dir() -> Result<PathBuf, crate::error::EngineError> {
    let home = std::env::var("HOME")
        .map_err(|_| crate::error::EngineError::BadRequest("HOME env var not set".into()))?;
    Ok(PathBuf::from(home).join(HRIR_BASE_SUBPATH))
}

/// Resolve the absolute HRIR .wav path from surround config.
///
/// `base_dir` is the HRIR base directory (e.g. `~/.local/share/pipewire/hrir_hesuvi`).
/// Callers supply it via `hrir_base_dir()` in production; tests pass a temp path.
///
/// - If cfg.hrir == Some(stem) → <base_dir>/profiles/<stem>.wav  (error if missing)
/// - If cfg.hrir == None → first *.wav in <base_dir>/profiles/ sorted lexicographically
///   (fallback: <base_dir>/hrir.wav if it exists; else BadRequest error)
pub fn resolve_hrir_path(
    cfg: &SurroundConfig,
    base_dir: &std::path::Path,
) -> Result<PathBuf, crate::error::EngineError> {
    let profiles_dir = base_dir.join("profiles");

    match &cfg.hrir {
        Some(stem) => {
            let path = profiles_dir.join(format!("{stem}.wav"));
            if path.exists() {
                Ok(path)
            } else {
                Err(crate::error::EngineError::BadRequest(format!(
                    "HRIR profile not found: {}",
                    path.display()
                )))
            }
        }
        None => {
            // Try profiles dir first.
            // Prefer the first 48 kHz file (sorted lexicographically); if none
            // is readable as 48 kHz, fall back to the lexicographically-first
            // .wav so the pipeline degrades gracefully rather than erroring.
            if profiles_dir.is_dir() {
                let mut wavs: Vec<PathBuf> = std::fs::read_dir(&profiles_dir)
                    .map_err(|e| {
                        crate::error::EngineError::BadRequest(format!(
                            "cannot read HRIR profiles dir: {e}"
                        ))
                    })?
                    .filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("wav"))
                    .collect();
                wavs.sort();

                if !wavs.is_empty() {
                    // First pass: find a 48 kHz file.
                    let preferred = wavs
                        .iter()
                        .find(|p| {
                            crate::hrir_import::read_wav_info(p)
                                .map(|info| info.sample_rate == 48_000)
                                .unwrap_or(false)
                        })
                        .cloned();

                    if let Some(p) = preferred {
                        return Ok(p);
                    }

                    // Fallback: lexicographically first (may not be 48 kHz).
                    // Log a warning so the mismatch is visible in the console.
                    if let Some(fallback_lex) = wavs.into_iter().next() {
                        let rate_hint = crate::hrir_import::read_wav_info(&fallback_lex)
                            .map(|i| i.sample_rate)
                            .unwrap_or(0);
                        eprintln!(
                            "asm: warning — no 48 kHz HRIR found; falling back to {} (detected rate: {} Hz). Audio quality may be degraded.",
                            fallback_lex.display(),
                            rate_hint
                        );
                        return Ok(fallback_lex);
                    }
                }
            }
            // Fallback: <base_dir>/hrir.wav
            let fallback = base_dir.join("hrir.wav");
            if fallback.exists() {
                return Ok(fallback);
            }
            Err(crate::error::EngineError::BadRequest(
                "no HRIR profiles found — install a .wav file in ~/.local/share/pipewire/hrir_hesuvi/profiles/".into()
            ))
        }
    }
}

/// Bundled dry HRIR used when a pinned stem is missing.
pub const FALLBACK_HRIR_STEM: &str = "07-oal+++-openal-max";

/// Resolve the HRIR path; if a pinned stem is missing, fall back to a bundled dry HRIR
/// (then any available) and report the missing stem so the UI can prompt to import.
/// Returns `(path, missing_stem)`. `missing_stem = Some(stem)` only when a fallback was used.
pub fn resolve_hrir_path_or_fallback(
    cfg: &SurroundConfig,
    base_dir: &std::path::Path,
) -> Result<(std::path::PathBuf, Option<String>), crate::error::EngineError> {
    match resolve_hrir_path(cfg, base_dir) {
        Ok(p) => Ok((p, None)),
        Err(e) => {
            if let Some(stem) = &cfg.hrir {
                for fb in [Some(FALLBACK_HRIR_STEM.to_string()), None] {
                    let fb_cfg = SurroundConfig { hrir: fb, ..cfg.clone() };
                    if let Ok(p) = resolve_hrir_path(&fb_cfg, base_dir) {
                        return Ok((p, Some(stem.clone())));
                    }
                }
            }
            Err(e)
        }
    }
}

/// Return sorted HRIR stems (no .wav) from the profiles directory. Empty if dir missing.
///
/// `base_dir` is the HRIR base directory. Callers supply it via `hrir_base_dir()` in
/// production; tests pass a temp path directly to avoid env reads.
pub fn available_hrirs(base_dir: &std::path::Path) -> Vec<String> {
    let profiles_dir = base_dir.join("profiles");
    if !profiles_dir.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&profiles_dir) else {
        return Vec::new();
    };
    let mut stems: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("wav"))
        .filter_map(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        })
        .collect();
    stems.sort();
    stems
}

/// Build a SurroundSpec from a SurroundConfig.
pub fn surround_spec(cfg: &SurroundConfig) -> SurroundSpec {
    SurroundSpec {
        node_name_base: "arctis_surround".into(),
        description: "Arctis Surround Sink".into(),
        hw_sink: cfg.hw_sink.clone(),
    }
}

/// For each channel whose output_device is None, set it to `default_device`.
/// Channels with an explicit output_device are left untouched.
pub fn overlay_default_output(channels: &mut [arctis_config::ChannelConfig], default_device: &str) {
    for ch in channels.iter_mut() {
        if ch.output_device.is_none() {
            ch.output_device = Some(default_device.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arctis_config::{
        ChannelConfig, EqBandConfig, MicChainConfig, Profile, RouteConfig, SurroundConfig,
    };

    fn profile_default() -> Profile {
        Profile {
            name: "default".to_string(),
            channels: vec![
                ChannelConfig {
                    id: "game".to_string(),
                    node_name: "Arctis_Game".to_string(),
                    description: "Game".to_string(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
                ChannelConfig {
                    id: "chat".to_string(),
                    node_name: "Arctis_Chat".to_string(),
                    description: "Chat".to_string(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
            ],
            routes: vec![],
            mic: MicChainConfig::default(),
            surround: SurroundConfig::default(),
            master_volume_db: 0.0,
            master_volume_pct: 100,
            master_mute: false,
            chatmix_position: 4,
            default_sink_channel: None,
        }
    }

    #[test]
    fn band_kind_peaking_parses() {
        assert_eq!(band_kind_from_str("peaking").unwrap(), BandKind::Peaking);
    }

    #[test]
    fn band_kind_lowshelf_parses() {
        assert_eq!(band_kind_from_str("lowshelf").unwrap(), BandKind::LowShelf);
    }

    #[test]
    fn band_kind_highshelf_parses() {
        assert_eq!(
            band_kind_from_str("highshelf").unwrap(),
            BandKind::HighShelf
        );
    }

    #[test]
    fn band_kind_unknown_is_reconcile_error() {
        let err = band_kind_from_str("linear").unwrap_err();
        assert!(matches!(err, EngineError::Reconcile(_)));
    }

    #[test]
    fn eq_band_from_cfg_maps_all_fields() {
        let cfg = EqBandConfig {
            kind: "peaking".to_string(),
            freq_hz: 1000.0,
            q: 1.5,
            gain_db: -3.0,
        };
        let band = eq_band_from_cfg(&cfg).unwrap();
        assert_eq!(band.kind, BandKind::Peaking);
        assert_eq!(band.freq_hz, 1000.0);
        assert_eq!(band.q, 1.5);
        assert_eq!(band.gain_db, -3.0);
    }

    #[test]
    fn eq_model_for_empty_eq_gives_default_10band() {
        let ch = ChannelConfig {
            id: "game".into(),
            node_name: "Arctis_Game".into(),
            description: "Game".into(),
            output_device: None,
            eq: vec![],
            volume_db: 0.0,
            volume_pct: 100,
            muted: false,
        };
        let model = eq_model_for(&ch).unwrap();
        assert_eq!(model, EqModel::default_10band());
    }

    #[test]
    fn eq_model_for_non_empty_maps_bands() {
        // A single override at index 0 → still yields a dense 10-band model.
        let ch = ChannelConfig {
            id: "game".into(),
            node_name: "Arctis_Game".into(),
            description: "Game".into(),
            output_device: None,
            eq: vec![EqBandConfig {
                kind: "peaking".into(),
                freq_hz: 500.0,
                q: 1.0,
                gain_db: 2.0,
            }],
            volume_db: 0.0,
            volume_pct: 100,
            muted: false,
        };
        let model = eq_model_for(&ch).unwrap();
        // Dense model: 10 bands total (override at index 0 + canonical defaults for 1..9).
        assert_eq!(model.bands.len(), 10);
        assert_eq!(model.bands[0].freq_hz, 500.0);
        assert_eq!(model.bands[0].gain_db, 2.0);
        // Canonical defaults fill the remaining bands.
        assert_eq!(model.bands[9].freq_hz, 16000.0);
        assert_eq!(model.bands[9].gain_db, 0.0);
    }

    #[test]
    fn default_eq_band_configs_is_ten_canonical_flat_bands() {
        let v = default_eq_band_configs();
        assert_eq!(v.len(), 10);
        let freqs: Vec<f32> = v.iter().map(|b| b.freq_hz).collect();
        assert_eq!(
            freqs,
            vec![31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0]
        );
        assert!(v.iter().all(|b| b.gain_db == 0.0));
        // Q derives from the audio default: constant-Q 1.41 peaking mids,
        // Butterworth 0.707 shelves at the extremes.
        assert!(v[1..9].iter().all(|b| b.q == arctis_audio::DEFAULT_PEAKING_Q));
        assert_eq!(v[0].q, arctis_audio::DEFAULT_SHELF_Q);
        assert_eq!(v[9].q, arctis_audio::DEFAULT_SHELF_Q);
        // Kinds derive from the audio default: shelves at the extremes, peaking middle.
        assert_eq!(v[0].kind, "lowshelf");
        assert_eq!(v[9].kind, "highshelf");
        assert!(v[1..9].iter().all(|b| b.kind == "peaking"));
    }

    #[test]
    fn band_kind_to_str_round_trips_with_from_str() {
        for s in ["peaking", "lowshelf", "highshelf"] {
            assert_eq!(band_kind_to_str(band_kind_from_str(s).unwrap()), s);
        }
    }

    #[test]
    fn dense_eq_bands_overlays_overrides_on_defaults() {
        let mut ch = arctis_config::ChannelConfig {
            id: "game".into(), node_name: "Arctis_Game".into(), description: "g".into(),
            output_device: None, eq: vec![], volume_db: 0.0, volume_pct: 100, muted: false,
        };
        // Sparse override: only band index 2 set (a +3 dB highshelf at 300 Hz).
        ch.eq = vec![
            arctis_config::EqBandConfig { kind: "peaking".into(), freq_hz: 31.0, q: 1.0, gain_db: 0.0 },
            arctis_config::EqBandConfig { kind: "peaking".into(), freq_hz: 62.0, q: 1.0, gain_db: 0.0 },
            arctis_config::EqBandConfig { kind: "highshelf".into(), freq_hz: 300.0, q: 1.0, gain_db: 3.0 },
        ];
        let dense = dense_eq_bands(&ch);
        assert_eq!(dense.len(), 10);
        assert_eq!(dense[2].kind, "highshelf");
        assert_eq!(dense[2].freq_hz, 300.0);
        assert_eq!(dense[2].gain_db, 3.0);
        // Untouched slots keep canonical defaults.
        assert_eq!(dense[9].freq_hz, 16000.0);
        assert_eq!(dense[9].gain_db, 0.0);
    }

    #[test]
    fn dense_eq_bands_empty_config_is_ten_defaults() {
        let ch = arctis_config::ChannelConfig {
            id: "chat".into(), node_name: "Arctis_Chat".into(), description: "c".into(),
            output_device: None, eq: vec![], volume_db: 0.0, volume_pct: 100, muted: false,
        };
        assert_eq!(dense_eq_bands(&ch), default_eq_band_configs());
    }

    #[test]
    fn surround_routes_channel_reflects_enabled_and_membership() {
        // The single source of truth for "is this channel routed through the surround
        // convolver" — used both to size the channel sink (8-ch) AND to keep reconcile
        // Step 3 from clobbering the convolver routing (C1).
        let mut s = arctis_config::SurroundConfig {
            enabled: true,
            channels: vec!["game".into()],
            ..Default::default()
        };
        assert!(surround_routes_channel(&s, "game"), "enabled + member → routed");
        assert!(!surround_routes_channel(&s, "chat"), "non-member → not routed");
        s.enabled = false;
        assert!(
            !surround_routes_channel(&s, "game"),
            "surround disabled → no channel is surround-routed"
        );
    }

    #[test]
    fn channel_set_marks_surround_routed_channel_8ch_else_stereo() {
        // Reusable, profile-agnostic surround: any channel listed in surround.channels
        // (when surround is enabled) becomes an 8-channel 7.1 sink so a game outputs
        // discrete surround into it; every other channel stays stereo.
        let mut p = profile_default();
        p.surround.enabled = true;
        p.surround.channels = vec!["game".into()];
        let set = channel_set_from_profile(&p);
        let game = set.channels.iter().find(|c| c.id == "game").unwrap();
        let chat = set.channels.iter().find(|c| c.id == "chat").unwrap();
        assert_eq!(
            game.channels,
            ChainChannels::Surround71,
            "surround-routed channel must be 8-channel 7.1"
        );
        assert_eq!(
            chat.channels,
            ChainChannels::Stereo,
            "non-surround channel must stay stereo"
        );
    }

    #[test]
    fn channel_set_surround_channel_layout_follows_mode() {
        // The channel sink's channel count must match the convolver INPUT for the
        // active mode (8-ch for 7.1, 6-ch for 5.1) — and StereoBypass must stay 2-ch
        // so apps don't see a phantom 7.1 device. Always-8ch (bug H2) mismatches the
        // 5.1 convolver and defeats StereoBypass.
        let mut p = profile_default();
        p.surround.enabled = true;
        p.surround.channels = vec!["game".into()];
        let game_layout = |p: &Profile| {
            channel_set_from_profile(p)
                .channels
                .iter()
                .find(|c| c.id == "game")
                .unwrap()
                .channels
        };

        p.surround.mode = arctis_config::SurroundMode::Hrir71;
        assert_eq!(game_layout(&p), ChainChannels::Surround71, "7.1 → 8-ch");
        p.surround.mode = arctis_config::SurroundMode::Auto;
        assert_eq!(game_layout(&p), ChainChannels::Surround71, "Auto resolves to 7.1 → 8-ch");
        p.surround.mode = arctis_config::SurroundMode::Hrir51;
        assert_eq!(game_layout(&p), ChainChannels::Surround51, "5.1 → 6-ch");
        p.surround.mode = arctis_config::SurroundMode::StereoBypass;
        assert_eq!(game_layout(&p), ChainChannels::Stereo, "StereoBypass → 2-ch");
    }

    #[test]
    fn channel_set_all_stereo_when_surround_disabled() {
        let mut p = profile_default();
        p.surround.enabled = false;
        p.surround.channels = vec!["game".into()]; // listed, but surround OFF
        let set = channel_set_from_profile(&p);
        assert!(
            set.channels
                .iter()
                .all(|c| c.channels == ChainChannels::Stereo),
            "surround disabled → every channel stays stereo"
        );
    }

    #[test]
    fn channel_def_from_cfg_maps_all_fields() {
        let ch = ChannelConfig {
            id: "media".into(),
            node_name: "Arctis_Media".into(),
            description: "Media".into(),
            output_device: Some("speakers".into()),
            eq: vec![],
            volume_db: 0.0,
            volume_pct: 100,
            muted: false,
        };
        let def = channel_def_from_cfg(&ch);
        assert_eq!(def.id, "media");
        assert_eq!(def.node_name, "Arctis_Media");
        assert_eq!(def.description, "Media");
        assert_eq!(def.output_device.as_deref(), Some("speakers"));
    }

    #[test]
    fn channel_set_from_profile_has_all_channels() {
        let p = profile_default();
        let set = channel_set_from_profile(&p);
        assert_eq!(set.channels.len(), 2);
        assert_eq!(set.channels[0].id, "game");
        assert_eq!(set.channels[1].id, "chat");
    }

    #[test]
    fn route_rules_from_profile_empty_routes() {
        let p = profile_default();
        let rules = route_rules_from_profile(&p);
        assert!(rules.is_empty());
    }

    #[test]
    fn route_rules_from_profile_maps_routes() {
        let mut p = profile_default();
        p.routes = vec![
            RouteConfig {
                app_binary: "firefox".into(),
                target_sink: "Arctis_Media".into(),
            },
            RouteConfig {
                app_binary: "discord".into(),
                target_sink: "Arctis_Chat".into(),
            },
        ];
        let rules = route_rules_from_profile(&p);
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].app_binary, "firefox");
        assert_eq!(rules[0].target_sink, "Arctis_Media");
        assert_eq!(rules[1].app_binary, "discord");
        assert_eq!(rules[1].target_sink, "Arctis_Chat");
    }

    // ── linear_to_db unit tests ───────────────────────────────────────────────

    #[test]
    fn linear_to_db_zero_is_floor() {
        assert!(
            (linear_to_db(0.0) - (-70.0)).abs() < 1e-6,
            "0.0 → -70 dB floor"
        );
    }

    #[test]
    fn linear_to_db_one_is_zero_db() {
        assert!(linear_to_db(1.0).abs() < 1e-4, "1.0 → 0 dB");
    }

    #[test]
    fn linear_to_db_0_003_is_around_minus_50_db() {
        // 20 * log10(0.003) ≈ -50.46 dB
        let db = linear_to_db(0.003);
        assert!(
            db < -49.0 && db > -52.0,
            "0.003 should be ≈ -50 dB, got {db}"
        );
    }

    #[test]
    fn linear_to_db_clamps_above_20_db() {
        let db = linear_to_db(1000.0); // 60 dB, clamped to 20
        assert!((db - 20.0).abs() < 1e-6, "large linear clamped to 20 dB");
    }

    // ── db_to_linear unit tests ───────────────────────────────────────────────

    #[test]
    fn db_to_linear_zero_db_is_unity() {
        let v = db_to_linear(0.0);
        assert!((v - 1.0).abs() < 1e-6, "0 dB must map to 1.0, got {v}");
    }

    #[test]
    fn db_to_linear_positive_20db_is_10() {
        let v = db_to_linear(20.0);
        assert!((v - 10.0).abs() < 1e-4, "+20 dB must map to 10.0, got {v}");
    }

    #[test]
    fn db_to_linear_negative_20db_is_0_1() {
        let v = db_to_linear(-20.0);
        assert!((v - 0.1).abs() < 1e-6, "-20 dB must map to 0.1, got {v}");
    }

    #[test]
    fn db_to_linear_6db_is_approx_2() {
        let v = db_to_linear(6.0);
        // 10^(6/20) = 10^0.3 ≈ 1.995
        assert!((v - 1.995).abs() < 0.001, "+6 dB must be ~2.0, got {v}");
    }

    // ── mic_chain_nodes unit tests ─────────────────────────────────────────────

    use arctis_audio::MockPluginProbe;

    #[test]
    fn mic_chain_nodes_passthrough_gives_single_linear_node() {
        let cfg = MicChainConfig::passthrough();
        let probe = MockPluginProbe::none();
        let (nodes, availability) = mic_chain_nodes(&cfg, &probe, true);
        assert_eq!(nodes.len(), 1, "passthrough must yield exactly one node");
        assert_eq!(nodes[0].name, "mic_gain");
        assert_eq!(nodes[0].label, "linear");
        assert_eq!(
            nodes[0].controls,
            vec![("Mult".to_string(), 1.0), ("Add".to_string(), 0.0)]
        );
        // The always-on limiter is still reported (unavailable here: no plugin).
        assert_eq!(availability.len(), 1, "only the limiter entry is reported");
        assert_eq!(availability[0].stage, crate::state::StageName::Limiter);
        assert!(!availability[0].available, "no plugin in MockPluginProbe::none()");
    }

    #[test]
    fn mic_chain_nodes_gain_enabled_uses_db_to_linear() {
        let mut cfg = MicChainConfig::passthrough();
        cfg.gain.enabled = true;
        cfg.gain.gain_db = 6.0;
        let probe = MockPluginProbe::none();
        let (nodes, availability) = mic_chain_nodes(&cfg, &probe, true);
        // Only the gain node (no other stages enabled)
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "mic_gain");
        let mult = nodes[0].controls[0].1;
        assert!(
            (mult - db_to_linear(6.0)).abs() < 1e-6,
            "Mult must be db_to_linear(6.0)"
        );
        // gain + the always-on limiter entry (unavailable with an empty probe).
        assert_eq!(availability.len(), 2);
        assert!(availability[0].available);
        assert!(availability[0].requested);
        assert_eq!(availability[1].stage, crate::state::StageName::Limiter);
    }

    #[test]
    fn mic_chain_nodes_rnnoise_missing_plugin_dropped_and_marked_unavailable() {
        use arctis_config::SuppressionBackend;
        let mut cfg = MicChainConfig::passthrough();
        cfg.suppression.enabled = true;
        cfg.suppression.backend = SuppressionBackend::Rnnoise;
        let probe = MockPluginProbe::none(); // rnnoise plugin absent
        let (nodes, availability) = mic_chain_nodes(&cfg, &probe, true);
        // suppression dropped; fallback passthrough emitted
        assert_eq!(
            nodes.len(),
            1,
            "should fall back to passthrough linear node"
        );
        assert_eq!(nodes[0].label, "linear");
        let suppression_avail = availability
            .iter()
            .find(|a| a.stage == crate::state::StageName::Suppression);
        let a = suppression_avail.expect("suppression stage must appear in availability");
        assert!(!a.available, "suppression must be marked unavailable");
        assert!(a.requested, "suppression was requested");
    }

    #[test]
    fn mic_chain_nodes_rnnoise_present_plugin_included() {
        use arctis_audio::RNNOISE_PLUGIN_BASENAME;
        use arctis_config::SuppressionBackend;
        let mut cfg = MicChainConfig::passthrough();
        cfg.suppression.enabled = true;
        cfg.suppression.backend = SuppressionBackend::Rnnoise;
        let probe = MockPluginProbe::with([RNNOISE_PLUGIN_BASENAME]);
        let (nodes, _) = mic_chain_nodes(&cfg, &probe, true);
        // Only suppression (no gain/highpass/gate/eq enabled) — rnnoise IS present
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "mic_suppression");
        assert_eq!(nodes[0].label, "noise_suppressor_mono");
        // plugin field must be the basename
        assert_eq!(nodes[0].plugin.as_deref(), Some(RNNOISE_PLUGIN_BASENAME));
    }

    // ── New mic_chain_nodes tests (Task 2) ────────────────────────────────────

    #[test]
    fn mic_chain_nodes_deepfilter_backend_uses_correct_ports_and_control() {
        use arctis_audio::DEEPFILTER_PLUGIN_BASENAME;
        use arctis_config::SuppressionBackend;
        let mut cfg = MicChainConfig::passthrough();
        cfg.suppression.enabled = true;
        cfg.suppression.backend = SuppressionBackend::DeepFilter;
        cfg.suppression.attenuation_limit_db = 80.0;
        let probe = MockPluginProbe::with([DEEPFILTER_PLUGIN_BASENAME]);
        let (nodes, avail) = mic_chain_nodes(&cfg, &probe, true);
        assert_eq!(nodes.len(), 1);
        let n = &nodes[0];
        assert_eq!(n.name, "mic_suppression");
        assert_eq!(n.label, "deep_filter_mono");
        assert_eq!(n.port_in, "Audio In");
        assert_eq!(n.port_out, "Audio Out");
        assert_eq!(n.controls.len(), 2);
        assert_eq!(n.controls[0].0, "Attenuation Limit (dB)");
        assert!((n.controls[0].1 - 80.0).abs() < 1e-6);
        // Spectral post-filter, port verified with analyseplugin (0..=0.05).
        assert_eq!(n.controls[1].0, "Post Filter Beta");
        assert!((n.controls[1].1 - 0.02).abs() < 1e-6);
        let s = avail
            .iter()
            .find(|a| a.stage == crate::state::StageName::Suppression)
            .expect("suppression must be in avail");
        assert!(s.available);
        assert!(s.requested);
    }

    #[test]
    fn mic_chain_nodes_rnnoise_backend_uses_vad_controls() {
        use arctis_audio::RNNOISE_PLUGIN_BASENAME;
        use arctis_config::SuppressionBackend;
        let mut cfg = MicChainConfig::passthrough();
        cfg.suppression.enabled = true;
        cfg.suppression.backend = SuppressionBackend::Rnnoise;
        cfg.suppression.vad_threshold = 55.0;
        cfg.suppression.vad_grace_ms = 600.0;
        cfg.suppression.vad_retro_grace_ms = 120.0;
        let probe = MockPluginProbe::with([RNNOISE_PLUGIN_BASENAME]);
        let (nodes, _) = mic_chain_nodes(&cfg, &probe, true);
        assert_eq!(nodes.len(), 1);
        let n = &nodes[0];
        assert_eq!(n.label, "noise_suppressor_mono");
        assert_eq!(n.port_in, "Input");
        assert_eq!(n.port_out, "Output");
        assert_eq!(n.controls.len(), 3);
        assert_eq!(n.controls[0].0, "VAD Threshold (%)");
        assert!((n.controls[0].1 - 55.0).abs() < 1e-6);
        assert_eq!(n.controls[1].0, "VAD Grace Period (ms)");
        assert!((n.controls[1].1 - 600.0).abs() < 1e-6);
        assert_eq!(n.controls[2].0, "Retroactive VAD Grace (ms)");
        assert!((n.controls[2].1 - 120.0).abs() < 1e-6);
    }

    #[test]
    fn mic_chain_nodes_builtin_noisegate_false_uses_ladspa_gate() {
        use arctis_audio::GATE_PLUGIN_BASENAME;
        let mut cfg = MicChainConfig::passthrough();
        cfg.gate.enabled = true;
        cfg.gate.threshold = 0.003;
        let probe = MockPluginProbe::with([GATE_PLUGIN_BASENAME]);
        let (nodes, avail) = mic_chain_nodes(&cfg, &probe, false);
        assert_eq!(nodes.len(), 1);
        let n = &nodes[0];
        assert_eq!(n.name, "mic_gate");
        assert_eq!(n.label, "gate");
        assert_eq!(n.port_in, "Input");
        assert_eq!(n.port_out, "Output");
        // First control is Threshold (dB) = linear_to_db(0.003)
        assert_eq!(n.controls[0].0, "Threshold (dB)");
        let expected_db = linear_to_db(0.003);
        assert!(
            (n.controls[0].1 - expected_db).abs() < 1e-4,
            "Threshold (dB) must be linear_to_db(0.003)"
        );
        // Last control is Output select = 0.0
        let last = n.controls.last().expect("controls not empty");
        assert_eq!(
            last.0,
            "Output select (-1 = key listen, 0 = gate, 1 = bypass)"
        );
        assert!((last.1 - 0.0).abs() < 1e-6);
        let s = avail
            .iter()
            .find(|a| a.stage == crate::state::StageName::Gate)
            .expect("gate in avail");
        assert!(s.available);
    }

    #[test]
    fn mic_chain_nodes_builtin_noisegate_true_uses_builtin() {
        let mut cfg = MicChainConfig::passthrough();
        cfg.gate.enabled = true;
        cfg.gate.threshold = 0.003;
        let probe = MockPluginProbe::none(); // no LADSPA needed
        let (nodes, avail) = mic_chain_nodes(&cfg, &probe, true);
        assert_eq!(nodes.len(), 1);
        let n = &nodes[0];
        assert_eq!(n.name, "mic_gate");
        assert_eq!(n.label, "noisegate");
        assert_eq!(n.port_in, "In");
        assert_eq!(n.port_out, "Out");
        assert_eq!(n.controls[0].0, "Open Threshold");
        assert!((n.controls[0].1 - 0.003).abs() < 1e-7);
        let s = avail
            .iter()
            .find(|a| a.stage == crate::state::StageName::Gate)
            .expect("gate in avail");
        assert!(s.available);
    }

    #[test]
    fn mic_chain_nodes_suppression_backend_unavailable_dropped_chain_still_builds() {
        use arctis_audio::DEEPFILTER_PLUGIN_BASENAME;
        use arctis_config::SuppressionBackend;
        let mut cfg = MicChainConfig::passthrough();
        cfg.suppression.enabled = true;
        cfg.suppression.backend = SuppressionBackend::DeepFilter;
        cfg.gain.enabled = true;
        let probe = MockPluginProbe::none(); // deepfilter absent
        let (nodes, avail) = mic_chain_nodes(&cfg, &probe, true);
        // gain present, suppression dropped → still 1 node (gain)
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "mic_gain");
        let s = avail
            .iter()
            .find(|a| a.stage == crate::state::StageName::Suppression)
            .expect("suppression in avail");
        assert!(!s.available);
        let _ = DEEPFILTER_PLUGIN_BASENAME;
    }

    #[test]
    fn mic_chain_spec_has_correct_shape() {
        let mut cfg = MicChainConfig::passthrough();
        cfg.hw_mic = Some("alsa_input.hw_mic".to_string());
        let spec = mic_chain_spec(&cfg);
        // (no change to mic_chain_spec — just confirming it still compiles)
        assert_eq!(spec.node_name, "arctis_clean_mic");
        assert_eq!(spec.description, "Clean Mic");
        assert!(matches!(spec.channels, ChainChannels::Mono));
        assert!(matches!(spec.kind, ChainKind::Source));
        assert_eq!(spec.capture_node_name, "arctis_clean_mic.capture");
        assert_eq!(spec.capture_target, Some("alsa_input.hw_mic".to_string()));
        assert_eq!(spec.playback_node_name, "arctis_clean_mic");
    }

    // ── Batch C mic-chain voicing tests ───────────────────────────────────────

    /// The gate must sit BEFORE the compressor (keying on the true noise floor)
    /// and the always-on limiter must close the chain.
    #[test]
    fn mic_chain_order_is_gate_compressor_eq_limiter() {
        use arctis_audio::{GATE_PLUGIN_BASENAME, LIMITER_PLUGIN_BASENAME, SC4M_PLUGIN_BASENAME};
        let mut cfg = MicChainConfig::passthrough();
        cfg.gain.enabled = true;
        cfg.highpass.enabled = true;
        cfg.gate.enabled = true;
        cfg.compressor.enabled = true;
        cfg.eq_enabled = true;
        cfg.eq = vec![EqBandConfig { kind: "peaking".into(), freq_hz: 1000.0, q: 1.0, gain_db: 2.0 }];
        let probe = MockPluginProbe::with([
            GATE_PLUGIN_BASENAME,
            SC4M_PLUGIN_BASENAME,
            LIMITER_PLUGIN_BASENAME,
        ]);
        let (nodes, _) = mic_chain_nodes(&cfg, &probe, true);
        let names: Vec<&str> = nodes.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["mic_gain", "mic_highpass", "mic_gate", "mic_compressor", "mic_eq_band_0", "mic_limiter"],
            "chain order must be gain → highpass → GATE → COMPRESSOR → EQ → LIMITER"
        );
    }

    /// sc4m: attack/release/knee/RMS are emitted explicitly — never left to the
    /// LADSPA hint defaults (~101/401 ms, far too slow for speech).
    #[test]
    fn mic_compressor_emits_explicit_attack_release_knee_rms() {
        use arctis_audio::SC4M_PLUGIN_BASENAME;
        let mut cfg = MicChainConfig::passthrough();
        cfg.compressor.enabled = true;
        let probe = MockPluginProbe::with([SC4M_PLUGIN_BASENAME]);
        let (nodes, _) = mic_chain_nodes(&cfg, &probe, true);
        let comp = nodes.iter().find(|n| n.name == "mic_compressor").expect("compressor present");
        let ctl = |name: &str| {
            comp.controls.iter().find(|(k, _)| k == name).unwrap_or_else(|| panic!("{name} emitted")).1
        };
        assert!((ctl("RMS/peak") - 0.0).abs() < 1e-6);
        assert!((ctl("Attack time (ms)") - 10.0).abs() < 1e-6, "config default 10 ms");
        assert!((ctl("Release time (ms)") - 150.0).abs() < 1e-6, "config default 150 ms");
        assert!((ctl("Knee radius (dB)") - 3.0).abs() < 1e-6);
    }

    /// Configured attack/release flow through to the sc4m controls.
    #[test]
    fn mic_compressor_attack_release_come_from_config() {
        use arctis_audio::SC4M_PLUGIN_BASENAME;
        let mut cfg = MicChainConfig::passthrough();
        cfg.compressor.enabled = true;
        cfg.compressor.attack_ms = 25.0;
        cfg.compressor.release_ms = 300.0;
        let probe = MockPluginProbe::with([SC4M_PLUGIN_BASENAME]);
        let (nodes, _) = mic_chain_nodes(&cfg, &probe, true);
        let comp = nodes.iter().find(|n| n.name == "mic_compressor").unwrap();
        assert!(comp.controls.iter().any(|(k, v)| k == "Attack time (ms)" && (*v - 25.0).abs() < 1e-6));
        assert!(comp.controls.iter().any(|(k, v)| k == "Release time (ms)" && (*v - 300.0).abs() < 1e-6));
    }

    /// Builtin gate: −6 dB hysteresis (close = 0.5 × open, not 0.9 ×).
    #[test]
    fn mic_builtin_gate_close_threshold_is_half_open() {
        let mut cfg = MicChainConfig::passthrough();
        cfg.gate.enabled = true;
        cfg.gate.threshold = 0.004;
        let probe = MockPluginProbe::none();
        let (nodes, _) = mic_chain_nodes(&cfg, &probe, true);
        let gate = nodes.iter().find(|n| n.name == "mic_gate").expect("gate present");
        let close = gate.controls.iter().find(|(k, _)| k == "Close Threshold").unwrap().1;
        assert!((close - 0.002).abs() < 1e-7, "close must be 0.5 × open, got {close}");
    }

    /// LADSPA gate: Range −30 dB (duck, don't hard-mute at −90 dB).
    #[test]
    fn mic_ladspa_gate_range_is_minus_30() {
        use arctis_audio::GATE_PLUGIN_BASENAME;
        let mut cfg = MicChainConfig::passthrough();
        cfg.gate.enabled = true;
        let probe = MockPluginProbe::with([GATE_PLUGIN_BASENAME]);
        let (nodes, _) = mic_chain_nodes(&cfg, &probe, false);
        let gate = nodes.iter().find(|n| n.name == "mic_gate").unwrap();
        let range = gate.controls.iter().find(|(k, _)| k == "Range (dB)").unwrap().1;
        assert!((range - (-30.0)).abs() < 1e-6, "Range must be -30 dB, got {range}");
    }

    /// Limiter: −1 dBFS ceiling, fully wet, appended even for a passthrough chain.
    #[test]
    fn mic_limiter_appended_when_available_with_minus_1_ceiling() {
        use arctis_audio::{LIMITER_LABEL, LIMITER_PLUGIN_BASENAME};
        let cfg = MicChainConfig::passthrough();
        let probe = MockPluginProbe::with([LIMITER_PLUGIN_BASENAME]);
        let (nodes, avail) = mic_chain_nodes(&cfg, &probe, true);
        let lim = nodes.last().expect("chain not empty");
        assert_eq!(lim.name, "mic_limiter", "limiter must be the FINAL node");
        assert_eq!(lim.label, LIMITER_LABEL);
        assert_eq!(lim.plugin.as_deref(), Some(LIMITER_PLUGIN_BASENAME));
        assert_eq!(lim.port_in, "Input");
        assert_eq!(lim.port_out, "Output");
        assert!(lim.controls.iter().any(|(k, v)| k == "dB limit" && (*v - (-1.0)).abs() < 1e-6));
        assert!(lim.controls.iter().any(|(k, v)| k == "Wet level" && (*v - 1.0).abs() < 1e-6));
        assert!(lim.controls.iter().any(|(k, v)| k == "Residue level" && v.abs() < 1e-6));
        let l = avail.iter().find(|a| a.stage == crate::state::StageName::Limiter).unwrap();
        assert!(l.available && l.requested);
    }

    /// Highpass emits Q = 0.7071 (Butterworth).
    #[test]
    #[allow(clippy::approx_constant)]
    fn mic_highpass_q_is_butterworth() {
        let mut cfg = MicChainConfig::passthrough();
        cfg.highpass.enabled = true;
        let probe = MockPluginProbe::none();
        let (nodes, _) = mic_chain_nodes(&cfg, &probe, true);
        let hp = nodes.iter().find(|n| n.name == "mic_highpass").unwrap();
        let q = hp.controls.iter().find(|(k, _)| k == "Q").unwrap().1;
        assert!((q - 0.7071).abs() < 1e-6, "highpass Q must be 0.7071, got {q}");
    }

    // ── resolve_hrir_path 48 kHz preference tests ─────────────────────────────

    fn surround_cfg_none() -> SurroundConfig {
        SurroundConfig {
            hrir: None,
            ..SurroundConfig::default()
        }
    }

    #[test]
    fn default_resolution_prefers_48k_over_lexicographically_first_44k() {
        let d = tempfile::tempdir().unwrap();
        let profiles = d.path().join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        // 00-a.wav is lex-first but 44.1 kHz — should NOT be chosen.
        crate::hrir_import::tests::write_wav(&profiles.join("00-a.wav"), 14, 44_100);
        // 07-b.wav is lex-second but 48 kHz — should be chosen.
        crate::hrir_import::tests::write_wav(&profiles.join("07-b.wav"), 14, 48_000);

        let cfg = surround_cfg_none();
        let result = resolve_hrir_path(&cfg, d.path()).unwrap();
        assert!(
            result.ends_with("07-b.wav"),
            "expected 07-b.wav (48 kHz), got: {}",
            result.display()
        );
    }

    #[test]
    fn default_resolution_falls_back_to_first_when_no_48k() {
        let d = tempfile::tempdir().unwrap();
        let profiles = d.path().join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        // Only 44.1 kHz files — should gracefully fall back to lex-first, no error.
        crate::hrir_import::tests::write_wav(&profiles.join("00-a.wav"), 14, 44_100);

        let cfg = surround_cfg_none();
        let result = resolve_hrir_path(&cfg, d.path()).unwrap();
        assert!(
            result.ends_with("00-a.wav"),
            "expected graceful fallback to 00-a.wav, got: {}",
            result.display()
        );
    }

    #[test]
    fn all_48k_default_picks_lexicographic_first() {
        let d = tempfile::tempdir().unwrap();
        let profiles = d.path().join("profiles");
        std::fs::create_dir_all(&profiles).unwrap();
        // Both 48 kHz — lex-first (00-a.wav) should be returned.
        crate::hrir_import::tests::write_wav(&profiles.join("00-a.wav"), 14, 48_000);
        crate::hrir_import::tests::write_wav(&profiles.join("07-b.wav"), 14, 48_000);

        let cfg = surround_cfg_none();
        let result = resolve_hrir_path(&cfg, d.path()).unwrap();
        assert!(
            result.ends_with("00-a.wav"),
            "expected lex-first 00-a.wav when all are 48 kHz, got: {}",
            result.display()
        );
    }

    #[test]
    fn overlay_default_output_sets_none_leaves_explicit() {
        let mut channels = vec![
            arctis_config::ChannelConfig {
                id: "game".to_string(),
                node_name: "Arctis_Game".to_string(),
                description: "Game".to_string(),
                output_device: None,
                eq: vec![],
                volume_db: 0.0,
                volume_pct: 100,
                muted: false,
            },
            arctis_config::ChannelConfig {
                id: "chat".to_string(),
                node_name: "Arctis_Chat".to_string(),
                description: "Chat".to_string(),
                output_device: Some("speakers".to_string()),
                eq: vec![],
                volume_db: 0.0,
                volume_pct: 100,
                muted: false,
            },
        ];
        overlay_default_output(
            &mut channels,
            "alsa_output.usb-SteelSeries_Arctis_Nova_Pro_Wireless-00.analog-stereo",
        );
        assert_eq!(
            channels[0].output_device.as_deref(),
            Some("alsa_output.usb-SteelSeries_Arctis_Nova_Pro_Wireless-00.analog-stereo"),
            "None channel should be set to headset"
        );
        assert_eq!(
            channels[1].output_device.as_deref(),
            Some("speakers"),
            "explicit channel must remain unchanged"
        );
    }

}
