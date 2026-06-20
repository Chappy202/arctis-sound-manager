use arctis_domain::eq_bounds::{
    EQ_FREQ_MAX_HZ, EQ_FREQ_MIN_HZ, EQ_GAIN_MAX_DB, EQ_GAIN_MIN_DB, EQ_Q_MAX, EQ_Q_MIN,
};
use serde::{Deserialize, Serialize};

use crate::error::ConfigError;

pub const CURRENT_VERSION: u32 = 1;

/// Frequency-domain EQ band definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqBandConfig {
    pub kind: String, // "peaking" | "lowshelf" | "highshelf"
    pub freq_hz: f32,
    pub q: f32,
    pub gain_db: f32,
}

/// A single virtual audio channel routed through PipeWire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub id: String,        // "game" | "chat" | "media"
    pub node_name: String, // e.g. "Arctis_Game"
    pub description: String,
    #[serde(default)]
    pub output_device: Option<String>, // hardware sink node.name, or None = default
    #[serde(default)]
    pub eq: Vec<EqBandConfig>, // empty => flat 10-band default at apply time
}

/// Application-level routing rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteConfig {
    pub app_binary: String,
    pub target_sink: String,
}

/// Named collection of channel configs and routing rules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub channels: Vec<ChannelConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

/// Root configuration object. Versioned for forward-compatibility checking.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub active_profile: String,
    pub profiles: Vec<Profile>,
}

impl Config {
    /// A valid, ready-to-apply default: one "default" profile with the Sonar 3-channel set,
    /// flat EQ, no routes, following the default output.
    pub fn default_config() -> Self {
        let channels = vec![
            ChannelConfig {
                id: "game".to_string(),
                node_name: "Arctis_Game".to_string(),
                description: "Game audio channel".to_string(),
                output_device: None,
                eq: Vec::new(),
            },
            ChannelConfig {
                id: "chat".to_string(),
                node_name: "Arctis_Chat".to_string(),
                description: "Chat audio channel".to_string(),
                output_device: None,
                eq: Vec::new(),
            },
            ChannelConfig {
                id: "media".to_string(),
                node_name: "Arctis_Media".to_string(),
                description: "Media audio channel".to_string(),
                output_device: None,
                eq: Vec::new(),
            },
        ];

        Config {
            version: CURRENT_VERSION,
            active_profile: "default".to_string(),
            profiles: vec![Profile {
                name: "default".to_string(),
                channels,
                routes: Vec::new(),
            }],
        }
    }

    /// Return a reference to the currently active profile.
    pub fn active(&self) -> Result<&Profile, ConfigError> {
        self.profile(&self.active_profile)
            .ok_or_else(|| ConfigError::ProfileNotFound(self.active_profile.clone()))
    }

    /// Look up a profile by name.
    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// Look up a profile by name (mutable).
    pub fn profile_mut(&mut self, name: &str) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| p.name == name)
    }

    /// Structural validation: version supported, active_profile exists, channel ids unique,
    /// EQ within audio bounds. Pure, no I/O.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Version check
        if self.version > CURRENT_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                found: self.version,
                max: CURRENT_VERSION,
            });
        }

        // Active profile must exist
        if self.profile(&self.active_profile).is_none() {
            return Err(ConfigError::ProfileNotFound(self.active_profile.clone()));
        }

        // Per-profile validation
        for profile in &self.profiles {
            // Channel ids must be unique within a profile
            let mut seen_ids = std::collections::HashSet::new();
            for channel in &profile.channels {
                if !seen_ids.insert(channel.id.as_str()) {
                    return Err(ConfigError::Invalid(format!(
                        "duplicate channel id '{}' in profile '{}'",
                        channel.id, profile.name
                    )));
                }

                // EQ band validation: use the same bounds as the audio engine
                // (single source of truth from arctis-domain::eq_bounds).
                for band in &channel.eq {
                    if !(EQ_FREQ_MIN_HZ..=EQ_FREQ_MAX_HZ).contains(&band.freq_hz) {
                        return Err(ConfigError::Invalid(format!(
                            "EQ band freq_hz {} Hz out of range {}..={} in channel '{}'",
                            band.freq_hz, EQ_FREQ_MIN_HZ, EQ_FREQ_MAX_HZ, channel.id
                        )));
                    }
                    if !(EQ_Q_MIN..=EQ_Q_MAX).contains(&band.q) {
                        return Err(ConfigError::Invalid(format!(
                            "EQ band Q {} out of range {}..={} in channel '{}'",
                            band.q, EQ_Q_MIN, EQ_Q_MAX, channel.id
                        )));
                    }
                    if !(EQ_GAIN_MIN_DB..=EQ_GAIN_MAX_DB).contains(&band.gain_db) {
                        return Err(ConfigError::Invalid(format!(
                            "EQ band gain_db {} dB out of range {}..={} in channel '{}'",
                            band.gain_db, EQ_GAIN_MIN_DB, EQ_GAIN_MAX_DB, channel.id
                        )));
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_channel_with_band(band: EqBandConfig) -> Config {
        let mut cfg = Config::default_config();
        cfg.profiles[0].channels[0].eq = vec![band];
        cfg
    }

    fn band(freq_hz: f32, q: f32, gain_db: f32) -> EqBandConfig {
        EqBandConfig {
            kind: "peaking".to_string(),
            freq_hz,
            q,
            gain_db,
        }
    }

    #[test]
    fn default_config_is_valid() {
        let cfg = Config::default_config();
        assert!(cfg.validate().is_ok(), "default_config should be valid");
        assert_eq!(cfg.version, CURRENT_VERSION);
        assert_eq!(cfg.active_profile, "default");
        let profile = cfg.active().expect("active profile should exist");
        assert_eq!(profile.channels.len(), 3);
        let ids: Vec<&str> = profile.channels.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"game"), "should have 'game' channel");
        assert!(ids.contains(&"chat"), "should have 'chat' channel");
        assert!(ids.contains(&"media"), "should have 'media' channel");
    }

    #[test]
    fn validate_rejects_unknown_active() {
        let mut cfg = Config::default_config();
        cfg.active_profile = "nonexistent".to_string();
        let err = cfg
            .validate()
            .expect_err("should fail with ProfileNotFound");
        assert!(
            matches!(err, ConfigError::ProfileNotFound(_)),
            "expected ProfileNotFound, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_bad_version() {
        let mut cfg = Config::default_config();
        cfg.version = 999;
        let err = cfg
            .validate()
            .expect_err("should fail with UnsupportedVersion");
        assert!(
            matches!(err, ConfigError::UnsupportedVersion { found: 999, .. }),
            "expected UnsupportedVersion {{ found: 999 }}, got: {err}"
        );
    }

    #[test]
    fn toml_round_trips() {
        let cfg = Config::default_config();
        let serialized = toml::to_string(&cfg).expect("serialize to TOML");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize from TOML");
        assert_eq!(cfg, deserialized, "TOML round-trip must preserve config");
    }

    // --- EQ gain bounds ---

    #[test]
    fn eq_gain_positive_boundary_passes() {
        // +12.0 dB is valid
        assert!(make_channel_with_band(band(1000.0, 1.0, 12.0))
            .validate()
            .is_ok());
    }

    #[test]
    fn eq_gain_negative_boundary_passes() {
        // -12.0 dB is valid
        assert!(make_channel_with_band(band(1000.0, 1.0, -12.0))
            .validate()
            .is_ok());
    }

    #[test]
    fn eq_gain_just_above_max_rejected() {
        let err = make_channel_with_band(band(1000.0, 1.0, 12.1))
            .validate()
            .expect_err("12.1 dB should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    #[test]
    fn eq_gain_just_below_min_rejected() {
        let err = make_channel_with_band(band(1000.0, 1.0, -12.1))
            .validate()
            .expect_err("-12.1 dB should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    // --- EQ Q bounds ---

    #[test]
    fn eq_q_min_boundary_passes() {
        assert!(make_channel_with_band(band(1000.0, 0.3, 0.0))
            .validate()
            .is_ok());
    }

    #[test]
    fn eq_q_max_boundary_passes() {
        assert!(make_channel_with_band(band(1000.0, 10.0, 0.0))
            .validate()
            .is_ok());
    }

    #[test]
    fn eq_q_just_below_min_rejected() {
        let err = make_channel_with_band(band(1000.0, 0.29, 0.0))
            .validate()
            .expect_err("Q=0.29 should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    #[test]
    fn eq_q_just_above_max_rejected() {
        let err = make_channel_with_band(band(1000.0, 10.1, 0.0))
            .validate()
            .expect_err("Q=10.1 should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    // --- EQ freq bounds ---

    #[test]
    fn eq_freq_min_boundary_passes() {
        assert!(make_channel_with_band(band(20.0, 1.0, 0.0))
            .validate()
            .is_ok());
    }

    #[test]
    fn eq_freq_max_boundary_passes() {
        assert!(make_channel_with_band(band(20_000.0, 1.0, 0.0))
            .validate()
            .is_ok());
    }

    #[test]
    fn eq_freq_just_below_min_rejected() {
        let err = make_channel_with_band(band(19.9, 1.0, 0.0))
            .validate()
            .expect_err("19.9 Hz should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    #[test]
    fn eq_freq_just_above_max_rejected() {
        let err = make_channel_with_band(band(20_000.1, 1.0, 0.0))
            .validate()
            .expect_err("20000.1 Hz should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }
}
