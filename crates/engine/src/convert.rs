use crate::error::EngineError;
use arctis_audio::RouteRule;
use arctis_audio::{BandKind, ChannelDef, ChannelSetConfig, EqBand, EqModel};
use arctis_config::{ChannelConfig, EqBandConfig, RouteConfig};

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

#[cfg(test)]
mod tests {
    use super::*;
    use arctis_config::{ChannelConfig, EqBandConfig, Profile, RouteConfig};

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
}
