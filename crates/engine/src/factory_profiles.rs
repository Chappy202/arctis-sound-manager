//! Data-driven factory-profile catalog. Each `FactoryProfileSpec` is the set of
//! overrides a template applies onto a clone of the active profile, so node names,
//! hw_sink, mic chain, and master volume are preserved. Adding a future game =
//! one struct literal, no new code paths.

use arctis_config::{Profile, SurroundMode};
use crate::error::EngineError;

/// One channel's pre-spatial content EQ seed (a named preset applied to a channel).
pub struct ChannelEqSeed {
    pub channel_id: &'static str,
    pub preset_name: &'static str,
}

/// A factory profile template: overrides applied onto a clone of the active profile.
pub struct FactoryProfileSpec {
    pub name: &'static str,
    pub hrir_stem: Option<&'static str>,
    pub mode: SurroundMode,
    pub blocksize: Option<u32>,
    pub surround_channels: &'static [&'static str],
    pub default_sink_channel: Option<&'static str>,
    pub content_eq: Option<ChannelEqSeed>,
    pub output_eq_preset: Option<&'static str>,
}

const DAYZ: FactoryProfileSpec = FactoryProfileSpec {
    name: "DayZ",
    hrir_stem: Some("04-gsx-sennheiser-gsx"),
    mode: SurroundMode::Hrir71,
    blocksize: Some(128),
    surround_channels: &["game"],
    default_sink_channel: Some("game"),
    content_eq: None,
    output_eq_preset: Some("DayZ Spatial"),
};

/// The catalog. DayZ is today's only entry.
pub fn factory_profiles() -> &'static [FactoryProfileSpec] {
    const ALL: &[FactoryProfileSpec] = &[DAYZ];
    ALL
}

/// Look up a template by name, case-insensitive.
pub fn find_factory_profile(name: &str) -> Option<&'static FactoryProfileSpec> {
    factory_profiles().iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

/// Serializable summary of a factory template for the UI listing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FactoryProfileInfo {
    pub name: String,
    pub hrir: Option<String>,
    pub mode: String,
}

/// Resolve a named EQ preset to its bands; unknown names are a hard error.
fn preset_bands(name: &str) -> Result<Vec<arctis_config::EqBandConfig>, EngineError> {
    crate::presets::factory_eq_presets()
        .into_iter()
        .find(|p| p.name == name)
        .map(|p| p.bands)
        .ok_or_else(|| EngineError::BadRequest(format!("unknown factory EQ preset: {name}")))
}

/// Apply a template onto a clone of the active profile, preserving hardware-specific
/// settings (node names, hw_sink, mic chain, master volume).
pub fn apply_factory_spec(active: &Profile, spec: &FactoryProfileSpec) -> Result<Profile, EngineError> {
    let mut profile = active.clone();
    profile.name = spec.name.into();

    if let Some(seed) = &spec.content_eq {
        let bands = preset_bands(seed.preset_name)?;
        if let Some(ch) = profile.channels.iter_mut().find(|c| c.id == seed.channel_id) {
            ch.eq = bands;
        }
    }

    let mut surround = active.surround.clone();
    surround.enabled = true;
    surround.channels = spec.surround_channels.iter().map(|s| s.to_string()).collect();
    surround.mode = spec.mode;
    surround.blocksize = spec.blocksize;
    if let Some(stem) = spec.hrir_stem {
        surround.hrir = Some(stem.into());
    }
    surround.output_eq = match spec.output_eq_preset {
        Some(n) => preset_bands(n)?,
        None => Vec::new(),
    };
    profile.surround = surround;
    profile.default_sink_channel = spec.default_sink_channel.map(|s| s.to_string());
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arctis_config::{ChannelConfig, MicChainConfig, SurroundConfig};

    fn make_active() -> Profile {
        Profile {
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
    fn apply_dayz_spec_sets_surround_and_output_eq() {
        let active = make_active();
        let p = apply_factory_spec(&active, &DAYZ).unwrap();
        assert_eq!(p.name, "DayZ");
        assert!(p.surround.enabled);
        assert_eq!(p.surround.channels, vec!["game".to_string()]);
        assert_eq!(p.surround.hrir, Some("04-gsx-sennheiser-gsx".to_string()));
        assert_eq!(p.surround.mode, SurroundMode::Hrir71);
        assert_eq!(p.surround.blocksize, Some(128));
        assert_eq!(p.surround.output_eq.len(), 10, "DayZ Spatial seeds 10 post bands");
        assert_eq!(p.default_sink_channel, Some("game".into()));
        // content_eq is None for DayZ → game channel EQ stays empty.
        let game = p.channels.iter().find(|c| c.id == "game").unwrap();
        assert!(game.eq.is_empty(), "no pre-convolution channel EQ for DayZ");
    }

    #[test]
    fn apply_dayz_spec_preserves_node_names_and_chat() {
        let active = make_active();
        let p = apply_factory_spec(&active, &DAYZ).unwrap();
        assert_eq!(p.channels.iter().find(|c| c.id == "game").unwrap().node_name, "Arctis_Game");
        assert_eq!(p.channels.iter().find(|c| c.id == "chat").unwrap().node_name, "Arctis_Chat");
        assert!(p.channels.iter().find(|c| c.id == "chat").unwrap().eq.is_empty());
    }

    #[test]
    fn apply_dayz_spec_overrides_hrir_but_preserves_hw_sink() {
        let mut active = make_active();
        active.surround.hw_sink = Some("alsa_output.pci-0000_00_1f.3".into());
        active.surround.hrir = Some("00-default-asm".into());
        let p = apply_factory_spec(&active, &DAYZ).unwrap();
        assert_eq!(p.surround.hw_sink, Some("alsa_output.pci-0000_00_1f.3".into()), "hw_sink preserved");
        assert_eq!(p.surround.hrir, Some("04-gsx-sennheiser-gsx".into()), "hrir pinned by spec");
        assert_eq!(p.surround.channels, vec!["game".to_string()]);
    }

    #[test]
    fn find_factory_profile_is_case_insensitive() {
        assert!(find_factory_profile("dayz").is_some());
        assert!(find_factory_profile("DAYZ").is_some());
        assert!(find_factory_profile("DayZ").is_some());
        assert!(find_factory_profile("nope").is_none());
    }

    #[test]
    fn factory_profile_info_lists_dayz() {
        let infos: Vec<FactoryProfileInfo> = factory_profiles()
            .iter()
            .map(|s| FactoryProfileInfo {
                name: s.name.to_string(),
                hrir: s.hrir_stem.map(|h| h.to_string()),
                mode: format!("{:?}", s.mode),
            })
            .collect();
        assert!(infos
            .iter()
            .any(|i| i.name == "DayZ" && i.hrir.as_deref() == Some("04-gsx-sennheiser-gsx")));
    }

    #[test]
    fn apply_factory_spec_unknown_preset_errors() {
        let active = make_active();
        let bogus = FactoryProfileSpec { output_eq_preset: Some("No Such Preset"), ..DAYZ };
        assert!(matches!(apply_factory_spec(&active, &bogus), Err(EngineError::BadRequest(_))));
    }
}
