//! Factory profile templates. Built from the currently active profile so
//! node names match the live PipeWire graph rather than hard-coded strings.
//!
//! Each template clones the caller's active profile and overrides only the
//! fields it cares about, so future schema additions survive transparently.

use arctis_config::Profile;

/// Build the "DayZ" factory profile from the currently active profile.
///
/// Clones `active` so node names, mic chain, master volume, and any
/// user customisations are preserved. Then overrides:
///
/// - `name = "DayZ"`
/// - Game channel EQ → `"FPS / Footsteps (Competitive)"` bands from the
///   factory EQ preset catalog (competitive footstep preset added in A7).
/// - `surround.enabled = true`, `surround.channels = ["game"]`
///   (all other surround fields — `hrir`, `hw_sink` — are kept from `active`).
/// - `default_sink_channel = Some("game")`.
pub fn dayz_profile(active: &Profile) -> Profile {
    let footstep_bands = crate::presets::factory_eq_presets()
        .into_iter()
        .find(|p| p.name == "FPS / Footsteps (Competitive)")
        .map(|p| p.bands)
        .unwrap_or_default();

    let mut profile = active.clone();
    profile.name = "DayZ".into();

    // Seed the game channel with the footstep EQ; leave all other channels flat.
    if let Some(game_ch) = profile.channels.iter_mut().find(|c| c.id == "game") {
        game_ch.eq = footstep_bands;
    }

    // Mutate a clone of the active surround so any pinned hw_sink / hrir are
    // preserved — only the enable flag and channel list are overridden.
    let mut surround = active.surround.clone();
    surround.enabled = true;
    surround.channels = vec!["game".into()];
    profile.surround = surround;

    profile.default_sink_channel = Some("game".into());
    profile
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
    fn dayz_profile_enables_game_surround_with_footstep_eq() {
        let active = make_active();
        let p = dayz_profile(&active);
        assert_eq!(p.name, "DayZ");
        assert!(p.surround.enabled);
        assert_eq!(p.surround.channels, vec!["game".to_string()]);
        let game = p.channels.iter().find(|c| c.id == "game").unwrap();
        assert!(!game.eq.is_empty(), "game channel seeded with footstep EQ");
        assert_eq!(p.default_sink_channel, Some("game".into()));
    }

    #[test]
    fn dayz_profile_preserves_node_names_and_chat_eq() {
        let active = make_active();
        let p = dayz_profile(&active);
        let game = p.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(game.node_name, "Arctis_Game");
        let chat = p.channels.iter().find(|c| c.id == "chat").unwrap();
        assert_eq!(chat.node_name, "Arctis_Chat");
        // chat channel eq should remain flat (unset by factory template)
        assert!(chat.eq.is_empty(), "chat channel eq must not be changed");
    }

    #[test]
    fn dayz_profile_surround_preserves_hw_sink_and_hrir() {
        let mut active = make_active();
        active.surround.hw_sink = Some("alsa_output.pci-0000_00_1f.3".into());
        active.surround.hrir = Some("00-default-asm".into());
        let p = dayz_profile(&active);
        assert_eq!(
            p.surround.hw_sink,
            Some("alsa_output.pci-0000_00_1f.3".into()),
            "hw_sink must be preserved from active"
        );
        assert_eq!(
            p.surround.hrir,
            Some("00-default-asm".into()),
            "hrir must be preserved from active"
        );
        // channels must be overridden to just ["game"]
        assert_eq!(p.surround.channels, vec!["game".to_string()]);
    }

    #[test]
    fn dayz_profile_game_eq_has_10_bands() {
        let active = make_active();
        let p = dayz_profile(&active);
        let game = p.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(game.eq.len(), 10, "footstep EQ must have 10 bands");
    }
}
