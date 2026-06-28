use crate::error::EngineError;
use crate::state::StageAvailability;
use arctis_audio::{
    BandKind, ChainChannels, ChainKind, ChainSpec, ChannelDef, ChannelSetConfig, EqBand, EqModel,
    FilterNode, NodeType, PluginProbe, RouteRule, DEEPFILTER_LABEL_MONO,
    DEEPFILTER_PLUGIN_BASENAME, GATE_LABEL, GATE_PLUGIN_BASENAME, RNNOISE_LABEL_MONO,
    RNNOISE_PLUGIN_BASENAME, SC4M_LABEL, SC4M_PLUGIN_BASENAME,
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
            kind: "peaking".to_string(),
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

/// Build the full `ChannelSetConfig` for a profile's channels.
pub fn channel_set_from_profile(p: &arctis_config::Profile) -> ChannelSetConfig {
    ChannelSetConfig {
        channels: p.channels.iter().map(channel_def_from_cfg).collect(),
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
/// Walks the fixed stage order: gain → highpass → suppression → compressor → gate → mic-EQ.
/// LADSPA stages (suppression, compressor) are only included if `probe.ladspa_available(path)`;
/// otherwise they are skipped and recorded as unavailable.
/// Gate uses the builtin noisegate when `builtin_noisegate = true` (PW ≥ 1.6),
/// otherwise falls back to LADSPA gate_1410.
///
/// If no enabled+available node results, emits a single passthrough `linear` node so
/// `render_chain_conf` never sees an empty node list.
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
                ("Q".to_string(), 0.7),
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
                vec![(
                    "Attenuation Limit (dB)".to_string(),
                    cfg.suppression.attenuation_limit_db,
                )],
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

    // ── Compressor stage (LADSPA sc4m) ───────────────────────────────────────
    if cfg.compressor.enabled {
        if probe.ladspa_available(SC4M_PLUGIN_BASENAME) {
            nodes.push(FilterNode {
                name: "mic_compressor".to_string(),
                node_type: NodeType::Ladspa,
                label: SC4M_LABEL.to_string(),
                plugin: Some(SC4M_PLUGIN_BASENAME.to_string()),
                port_in: "Input".to_string(),
                port_out: "Output".to_string(),
                controls: vec![
                    (
                        "Threshold level (dB)".to_string(),
                        cfg.compressor.threshold_db,
                    ),
                    ("Ratio (1:n)".to_string(), cfg.compressor.ratio),
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

    // ── Gate stage (builtin noisegate ≥1.6 or LADSPA gate_1410 fallback) ─────
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
                    ("Close Threshold".to_string(), cfg.gate.threshold * 0.9),
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
                    ("Range (dB)".to_string(), -90.0),
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
            // Try profiles dir first (sorted lexicographically)
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
                if let Some(first) = wavs.into_iter().next() {
                    return Ok(first);
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
        assert!(v.iter().all(|b| b.kind == "peaking" && b.gain_db == 0.0 && b.q == 1.0));
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
        assert!(
            availability.is_empty(),
            "passthrough has no stage availability entries"
        );
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
        assert_eq!(availability.len(), 1);
        assert!(availability[0].available);
        assert!(availability[0].requested);
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
        assert_eq!(n.controls.len(), 1);
        assert_eq!(n.controls[0].0, "Attenuation Limit (dB)");
        assert!((n.controls[0].1 - 80.0).abs() < 1e-6);
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
