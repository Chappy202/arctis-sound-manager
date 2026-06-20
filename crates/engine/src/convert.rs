use crate::error::EngineError;
use crate::state::StageAvailability;
use arctis_audio::{
    BandKind, ChainChannels, ChainSpec, ChannelDef, ChannelSetConfig, EqBand, EqModel, FilterNode,
    NodeType, PluginProbe, RouteRule, RNNOISE_LABEL_MONO, RNNOISE_PLUGIN, SC4M_LABEL, SC4M_PLUGIN,
};
use arctis_config::{ChannelConfig, EqBandConfig, MicChainConfig, RouteConfig};

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

/// Build an `EqModel` for a channel config.
///
/// - Empty `cfg.eq` → `EqModel::default_10band()` (flat EQ).
/// - Non-empty → map each band via `eq_band_from_cfg`.
pub fn eq_model_for(channel: &ChannelConfig) -> Result<EqModel, EngineError> {
    if channel.eq.is_empty() {
        return Ok(EqModel::default_10band());
    }
    let bands = channel
        .eq
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

/// Build the `ChainSpec` for the Clean Mic virtual source.
///
/// node_name = `arctis_clean_mic`, capture target = `cfg.hw_mic`, mono,
/// both media classes = `Audio/Source` (capture side binds the hw mic, playback
/// side exposes the virtual source).
pub fn mic_chain_spec(cfg: &MicChainConfig) -> ChainSpec {
    ChainSpec {
        node_name: "arctis_clean_mic".to_string(),
        description: "Clean Mic".to_string(),
        channels: ChainChannels::Mono,
        capture_media_class: "Audio/Source".to_string(),
        capture_node_name: "arctis_clean_mic.capture".to_string(),
        capture_target: cfg.hw_mic.clone(),
        playback_media_class: Some("Audio/Source".to_string()),
        playback_passive: false,
        playback_target: None,
        playback_node_name: "arctis_clean_mic".to_string(),
    }
}

/// Build the `FilterNode` list for the mic chain plus stage availability info.
///
/// Walks the fixed stage order: gain → highpass → rnnoise → compressor → gate → mic-EQ.
/// LADSPA stages (rnnoise, compressor) are only included if `probe.ladspa_exists(path)`;
/// otherwise they are skipped and recorded as unavailable.
///
/// If no enabled+available node results, emits a single passthrough `linear` node so
/// `render_chain_conf` never sees an empty node list.
pub fn mic_chain_nodes(
    cfg: &MicChainConfig,
    probe: &dyn PluginProbe,
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

    // ── RNNoise stage (LADSPA noise_suppressor_mono) ─────────────────────────
    if cfg.rnnoise.enabled {
        if probe.ladspa_exists(RNNOISE_PLUGIN) {
            nodes.push(FilterNode {
                name: "mic_rnnoise".to_string(),
                node_type: NodeType::Ladspa,
                label: RNNOISE_LABEL_MONO.to_string(),
                plugin: Some(RNNOISE_PLUGIN.to_string()),
                port_in: "Input".to_string(),
                port_out: "Output".to_string(),
                controls: vec![
                    ("VAD Threshold (%)".to_string(), cfg.rnnoise.vad_threshold),
                    (
                        "VAD Grace Period (ms)".to_string(),
                        cfg.rnnoise.vad_grace_ms,
                    ),
                    (
                        "Retroactive VAD Grace (ms)".to_string(),
                        cfg.rnnoise.vad_retro_grace_ms,
                    ),
                ],
            });
            availability.push(StageAvailability {
                stage: crate::state::StageName::Rnnoise,
                available: true,
                requested: true,
            });
        } else {
            availability.push(StageAvailability {
                stage: crate::state::StageName::Rnnoise,
                available: false,
                requested: true,
            });
        }
    }

    // ── Compressor stage (LADSPA sc4m) ───────────────────────────────────────
    if cfg.compressor.enabled {
        if probe.ladspa_exists(SC4M_PLUGIN) {
            nodes.push(FilterNode {
                name: "mic_compressor".to_string(),
                node_type: NodeType::Ladspa,
                label: SC4M_LABEL.to_string(),
                plugin: Some(SC4M_PLUGIN.to_string()),
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

    // ── Gate stage (builtin noisegate) ───────────────────────────────────────
    if cfg.gate.enabled {
        nodes.push(FilterNode {
            name: "mic_gate".to_string(),
            node_type: NodeType::Builtin,
            label: "noisegate".to_string(),
            plugin: None,
            port_in: "In".to_string(),
            port_out: "Out".to_string(),
            controls: vec![
                ("Threshold".to_string(), cfg.gate.threshold),
                ("Attack".to_string(), 5.0),
                ("Release".to_string(), 150.0),
            ],
        });
        availability.push(StageAvailability {
            stage: crate::state::StageName::Gate,
            available: true,
            requested: true,
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use arctis_config::{ChannelConfig, EqBandConfig, MicChainConfig, Profile, RouteConfig};

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
                },
                ChannelConfig {
                    id: "chat".to_string(),
                    node_name: "Arctis_Chat".to_string(),
                    description: "Chat".to_string(),
                    output_device: None,
                    eq: vec![],
                },
            ],
            routes: vec![],
            mic: MicChainConfig::default(),
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
        };
        let model = eq_model_for(&ch).unwrap();
        assert_eq!(model, EqModel::default_10band());
    }

    #[test]
    fn eq_model_for_non_empty_maps_bands() {
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
        };
        let model = eq_model_for(&ch).unwrap();
        assert_eq!(model.bands.len(), 1);
        assert_eq!(model.bands[0].freq_hz, 500.0);
        assert_eq!(model.bands[0].gain_db, 2.0);
    }

    #[test]
    fn channel_def_from_cfg_maps_all_fields() {
        let ch = ChannelConfig {
            id: "media".into(),
            node_name: "Arctis_Media".into(),
            description: "Media".into(),
            output_device: Some("speakers".into()),
            eq: vec![],
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
        let (nodes, availability) = mic_chain_nodes(&cfg, &probe);
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
        let (nodes, availability) = mic_chain_nodes(&cfg, &probe);
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
        let mut cfg = MicChainConfig::passthrough();
        cfg.rnnoise.enabled = true;
        let probe = MockPluginProbe::none(); // rnnoise plugin absent
        let (nodes, availability) = mic_chain_nodes(&cfg, &probe);
        // rnnoise dropped; fallback passthrough emitted
        assert_eq!(
            nodes.len(),
            1,
            "should fall back to passthrough linear node"
        );
        assert_eq!(nodes[0].label, "linear");
        let rnnoise_avail = availability
            .iter()
            .find(|a| a.stage == crate::state::StageName::Rnnoise);
        let a = rnnoise_avail.expect("rnnoise stage must appear in availability");
        assert!(!a.available, "rnnoise must be marked unavailable");
        assert!(a.requested, "rnnoise was requested");
    }

    #[test]
    fn mic_chain_nodes_rnnoise_present_plugin_included() {
        use arctis_audio::RNNOISE_PLUGIN;
        let mut cfg = MicChainConfig::passthrough();
        cfg.rnnoise.enabled = true;
        let probe = MockPluginProbe::with([RNNOISE_PLUGIN]);
        let (nodes, _) = mic_chain_nodes(&cfg, &probe);
        // Only rnnoise (no gain/highpass/gate/eq enabled) — rnnoise IS present
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "mic_rnnoise");
        assert_eq!(nodes[0].label, "noise_suppressor_mono");
    }

    #[test]
    fn mic_chain_spec_has_correct_shape() {
        let mut cfg = MicChainConfig::passthrough();
        cfg.hw_mic = Some("alsa_input.hw_mic".to_string());
        let spec = mic_chain_spec(&cfg);
        assert_eq!(spec.node_name, "arctis_clean_mic");
        assert_eq!(spec.description, "Clean Mic");
        assert!(matches!(spec.channels, ChainChannels::Mono));
        assert_eq!(spec.capture_media_class, "Audio/Source");
        assert_eq!(spec.capture_node_name, "arctis_clean_mic.capture");
        assert_eq!(spec.capture_target, Some("alsa_input.hw_mic".to_string()));
        assert_eq!(spec.playback_media_class, Some("Audio/Source".to_string()));
        assert!(!spec.playback_passive);
        assert_eq!(spec.playback_node_name, "arctis_clean_mic");
    }
}
