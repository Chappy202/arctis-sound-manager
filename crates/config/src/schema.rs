use arctis_domain::eq_bounds::{
    EQ_FREQ_MAX_HZ, EQ_FREQ_MIN_HZ, EQ_GAIN_MAX_DB, EQ_GAIN_MIN_DB, EQ_Q_MAX, EQ_Q_MIN,
    MIC_ATTEN_LIMIT_MAX_DB, MIC_ATTEN_LIMIT_MIN_DB, MIC_COMP_MAKEUP_MAX_DB, MIC_COMP_MAKEUP_MIN_DB,
    MIC_COMP_RATIO_MAX, MIC_COMP_RATIO_MIN, MIC_COMP_THRESHOLD_MAX_DB, MIC_COMP_THRESHOLD_MIN_DB,
    MIC_GAIN_MAX_DB, MIC_GAIN_MIN_DB, MIC_GATE_THRESHOLD_MAX, MIC_GATE_THRESHOLD_MIN,
    MIC_HIGHPASS_MAX_HZ, MIC_HIGHPASS_MIN_HZ, MIC_VAD_GRACE_MAX_MS, MIC_VAD_GRACE_MIN_MS,
    MIC_VAD_RETRO_GRACE_MAX_MS, MIC_VAD_RETRO_GRACE_MIN_MS, MIC_VAD_THRESHOLD_MAX,
    MIC_VAD_THRESHOLD_MIN,
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

// ── Mic-chain config ─────────────────────────────────────────────────────────

fn default_hp_hz() -> f32 {
    90.0
}
fn default_vad() -> f32 {
    40.0
}
fn default_grace() -> f32 {
    800.0
}
fn default_retro_grace() -> f32 {
    100.0
}
fn default_gate_thresh() -> f32 {
    0.003
}
fn default_atten_limit() -> f32 {
    100.0
}
fn default_comp_threshold() -> f32 {
    -18.0
}
fn default_comp_ratio() -> f32 {
    2.0
}
fn default_comp_makeup() -> f32 {
    4.0
}

/// Gain stage config: amplify/attenuate the raw mic signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicGainStage {
    /// Whether the gain stage is active. Defaults to false (passthrough).
    #[serde(default)]
    pub enabled: bool,
    /// Gain in dB. 0.0 = unity when active.
    #[serde(default)]
    pub gain_db: f32,
}

impl Default for MicGainStage {
    fn default() -> Self {
        Self {
            enabled: false,
            gain_db: 0.0,
        }
    }
}

/// High-pass filter stage to remove low-frequency rumble.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicHighpassStage {
    /// Whether the highpass stage is active. Defaults to false.
    #[serde(default)]
    pub enabled: bool,
    /// Cutoff frequency in Hz. Conservative default: 90 Hz.
    #[serde(default = "default_hp_hz")]
    pub freq_hz: f32,
}

impl Default for MicHighpassStage {
    fn default() -> Self {
        Self {
            enabled: false,
            freq_hz: default_hp_hz(),
        }
    }
}

/// Which noise-suppression backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SuppressionBackend {
    /// DeepFilterNet LADSPA plugin (default, higher quality).
    #[default]
    DeepFilter,
    /// RNNoise LADSPA plugin (fallback, lower CPU).
    Rnnoise,
}

/// Noise suppression stage — supports DeepFilterNet (default) or RNNoise (fallback).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicSuppressionStage {
    /// Whether the suppression stage is active. Defaults to false.
    #[serde(default)]
    pub enabled: bool,
    /// Which suppression backend to use.
    #[serde(default)]
    pub backend: SuppressionBackend,
    /// DeepFilterNet: attenuation limit in dB (0..=100). Default 100.0 = full suppression.
    #[serde(default = "default_atten_limit")]
    pub attenuation_limit_db: f32,
    /// RNNoise: VAD threshold %. Default 40.
    #[serde(default = "default_vad")]
    pub vad_threshold: f32,
    /// RNNoise: VAD grace period in ms. Default 800 ms.
    #[serde(default = "default_grace")]
    pub vad_grace_ms: f32,
    /// RNNoise: Retroactive VAD grace in ms. Default 100 ms.
    #[serde(default = "default_retro_grace")]
    pub vad_retro_grace_ms: f32,
}

impl Default for MicSuppressionStage {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: SuppressionBackend::default(),
            attenuation_limit_db: default_atten_limit(),
            vad_threshold: default_vad(),
            vad_grace_ms: default_grace(),
            vad_retro_grace_ms: default_retro_grace(),
        }
    }
}

/// Optional compressor stage (requires `sc4m` LADSPA plugin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicCompressorStage {
    /// Whether the compressor stage is active. Defaults to false.
    #[serde(default)]
    pub enabled: bool,
    /// Compressor threshold in dB. Default -18 dB.
    #[serde(default = "default_comp_threshold")]
    pub threshold_db: f32,
    /// Compression ratio (1:n). Default 2.0.
    #[serde(default = "default_comp_ratio")]
    pub ratio: f32,
    /// Makeup gain in dB. Default 4.0.
    #[serde(default = "default_comp_makeup")]
    pub makeup_db: f32,
}

impl Default for MicCompressorStage {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold_db: default_comp_threshold(),
            ratio: default_comp_ratio(),
            makeup_db: default_comp_makeup(),
        }
    }
}

/// Noise gate stage to silence below-threshold signals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicGateStage {
    /// Whether the noise gate is active. Defaults to false.
    #[serde(default)]
    pub enabled: bool,
    /// Open threshold (linear 0..1). Conservative default 0.003.
    #[serde(default = "default_gate_thresh")]
    pub threshold: f32,
}

impl Default for MicGateStage {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: default_gate_thresh(),
        }
    }
}

/// Per-profile microphone DSP chain configuration.
/// Default = **clean passthrough** — all stages disabled, no external plugin needed.
/// Old configs lacking a `[profiles.*.mic]` block deserialize cleanly to passthrough
/// via `#[serde(default)]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MicChainConfig {
    /// Master switch. false => Clean Mic source not built at all. Defaults to false.
    #[serde(default)]
    pub enabled: bool,
    /// Hardware mic node.name to capture from. None => follow default source.
    #[serde(default)]
    pub hw_mic: Option<String>,
    /// Gain stage.
    #[serde(default)]
    pub gain: MicGainStage,
    /// High-pass filter stage.
    #[serde(default)]
    pub highpass: MicHighpassStage,
    /// Noise suppression stage (DeepFilterNet by default, RNNoise fallback).
    /// `alias = "rnnoise"` preserves backward compatibility with old configs.
    #[serde(default, alias = "rnnoise")]
    pub suppression: MicSuppressionStage,
    /// Optional compressor stage (sc4m LADSPA).
    #[serde(default)]
    pub compressor: MicCompressorStage,
    /// Noise gate stage.
    #[serde(default)]
    pub gate: MicGateStage,
    /// Whether the mic parametric EQ stage is active.
    #[serde(default)]
    pub eq_enabled: bool,
    /// Mic EQ bands. Reuses `EqBandConfig` from the channel EQ. Empty = EQ stage off.
    #[serde(default)]
    pub eq: Vec<EqBandConfig>,
}

impl MicChainConfig {
    /// Explicit passthrough constructor: all stages disabled, empty EQ.
    pub fn passthrough() -> Self {
        Self::default()
    }
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
    /// Mic DSP chain config. Defaults to passthrough (all stages off).
    /// Old configs without this field deserialize cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub mic: MicChainConfig,
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
                mic: MicChainConfig::default(),
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

            // Mic-chain validation: only range-check enabled stages.
            let mic = &profile.mic;
            if mic.gain.enabled && !(MIC_GAIN_MIN_DB..=MIC_GAIN_MAX_DB).contains(&mic.gain.gain_db)
            {
                return Err(ConfigError::Invalid(format!(
                    "mic gain_db {} dB out of range {}..={} in profile '{}'",
                    mic.gain.gain_db, MIC_GAIN_MIN_DB, MIC_GAIN_MAX_DB, profile.name
                )));
            }
            if mic.highpass.enabled
                && !(MIC_HIGHPASS_MIN_HZ..=MIC_HIGHPASS_MAX_HZ).contains(&mic.highpass.freq_hz)
            {
                return Err(ConfigError::Invalid(format!(
                    "mic highpass freq_hz {} Hz out of range {}..={} in profile '{}'",
                    mic.highpass.freq_hz, MIC_HIGHPASS_MIN_HZ, MIC_HIGHPASS_MAX_HZ, profile.name
                )));
            }
            if mic.suppression.enabled {
                if !(MIC_ATTEN_LIMIT_MIN_DB..=MIC_ATTEN_LIMIT_MAX_DB)
                    .contains(&mic.suppression.attenuation_limit_db)
                {
                    return Err(ConfigError::Invalid(format!(
                        "mic attenuation_limit_db {} out of range {}..={} in profile '{}'",
                        mic.suppression.attenuation_limit_db,
                        MIC_ATTEN_LIMIT_MIN_DB,
                        MIC_ATTEN_LIMIT_MAX_DB,
                        profile.name
                    )));
                }
                if !(MIC_VAD_THRESHOLD_MIN..=MIC_VAD_THRESHOLD_MAX)
                    .contains(&mic.suppression.vad_threshold)
                {
                    return Err(ConfigError::Invalid(format!(
                        "mic vad_threshold {} out of range {}..={} in profile '{}'",
                        mic.suppression.vad_threshold,
                        MIC_VAD_THRESHOLD_MIN,
                        MIC_VAD_THRESHOLD_MAX,
                        profile.name
                    )));
                }
                if !(MIC_VAD_GRACE_MIN_MS..=MIC_VAD_GRACE_MAX_MS)
                    .contains(&mic.suppression.vad_grace_ms)
                {
                    return Err(ConfigError::Invalid(format!(
                        "mic vad_grace_ms {} ms out of range {}..={} in profile '{}'",
                        mic.suppression.vad_grace_ms,
                        MIC_VAD_GRACE_MIN_MS,
                        MIC_VAD_GRACE_MAX_MS,
                        profile.name
                    )));
                }
                if !(MIC_VAD_RETRO_GRACE_MIN_MS..=MIC_VAD_RETRO_GRACE_MAX_MS)
                    .contains(&mic.suppression.vad_retro_grace_ms)
                {
                    return Err(ConfigError::Invalid(format!(
                        "mic vad_retro_grace_ms {} ms out of range {}..={} in profile '{}'",
                        mic.suppression.vad_retro_grace_ms,
                        MIC_VAD_RETRO_GRACE_MIN_MS,
                        MIC_VAD_RETRO_GRACE_MAX_MS,
                        profile.name
                    )));
                }
            }
            if mic.gate.enabled
                && !(MIC_GATE_THRESHOLD_MIN..=MIC_GATE_THRESHOLD_MAX).contains(&mic.gate.threshold)
            {
                return Err(ConfigError::Invalid(format!(
                    "mic gate threshold {} out of range {}..={} in profile '{}'",
                    mic.gate.threshold,
                    MIC_GATE_THRESHOLD_MIN,
                    MIC_GATE_THRESHOLD_MAX,
                    profile.name
                )));
            }
            if mic.compressor.enabled {
                if !(MIC_COMP_THRESHOLD_MIN_DB..=MIC_COMP_THRESHOLD_MAX_DB)
                    .contains(&mic.compressor.threshold_db)
                {
                    return Err(ConfigError::Invalid(format!(
                        "mic compressor threshold_db {} dB out of range {}..={} in profile '{}'",
                        mic.compressor.threshold_db,
                        MIC_COMP_THRESHOLD_MIN_DB,
                        MIC_COMP_THRESHOLD_MAX_DB,
                        profile.name
                    )));
                }
                if !(MIC_COMP_RATIO_MIN..=MIC_COMP_RATIO_MAX).contains(&mic.compressor.ratio) {
                    return Err(ConfigError::Invalid(format!(
                        "mic compressor ratio {} out of range {}..={} in profile '{}'",
                        mic.compressor.ratio, MIC_COMP_RATIO_MIN, MIC_COMP_RATIO_MAX, profile.name
                    )));
                }
                if !(MIC_COMP_MAKEUP_MIN_DB..=MIC_COMP_MAKEUP_MAX_DB)
                    .contains(&mic.compressor.makeup_db)
                {
                    return Err(ConfigError::Invalid(format!(
                        "mic compressor makeup_db {} dB out of range {}..={} in profile '{}'",
                        mic.compressor.makeup_db,
                        MIC_COMP_MAKEUP_MIN_DB,
                        MIC_COMP_MAKEUP_MAX_DB,
                        profile.name
                    )));
                }
            }
            // Mic EQ bands reuse the existing EQ bounds.
            if mic.eq_enabled {
                for band in &mic.eq {
                    if !(EQ_FREQ_MIN_HZ..=EQ_FREQ_MAX_HZ).contains(&band.freq_hz) {
                        return Err(ConfigError::Invalid(format!(
                            "mic EQ band freq_hz {} Hz out of range {}..={} in profile '{}'",
                            band.freq_hz, EQ_FREQ_MIN_HZ, EQ_FREQ_MAX_HZ, profile.name
                        )));
                    }
                    if !(EQ_Q_MIN..=EQ_Q_MAX).contains(&band.q) {
                        return Err(ConfigError::Invalid(format!(
                            "mic EQ band Q {} out of range {}..={} in profile '{}'",
                            band.q, EQ_Q_MIN, EQ_Q_MAX, profile.name
                        )));
                    }
                    if !(EQ_GAIN_MIN_DB..=EQ_GAIN_MAX_DB).contains(&band.gain_db) {
                        return Err(ConfigError::Invalid(format!(
                            "mic EQ band gain_db {} dB out of range {}..={} in profile '{}'",
                            band.gain_db, EQ_GAIN_MIN_DB, EQ_GAIN_MAX_DB, profile.name
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

    // ── Task 2c: Mic-chain config tests ──────────────────────────────────────

    /// 1. `MicChainConfig::default()` is passthrough: master off, all stages off, empty eq.
    #[test]
    fn mic_default_is_passthrough() {
        let mic = MicChainConfig::default();
        assert!(!mic.enabled, "master switch should be off");
        assert!(!mic.gain.enabled, "gain should be off");
        assert!(!mic.highpass.enabled, "highpass should be off");
        assert!(!mic.suppression.enabled, "suppression should be off");
        assert!(!mic.compressor.enabled, "compressor should be off");
        assert!(!mic.gate.enabled, "gate should be off");
        assert!(!mic.eq_enabled, "eq_enabled should be off");
        assert!(mic.eq.is_empty(), "eq bands should be empty");
        assert_eq!(
            mic,
            MicChainConfig::passthrough(),
            "default == passthrough()"
        );
    }

    /// 2. A TOML profile without any mic block deserializes to passthrough.
    #[test]
    fn old_config_without_mic_block_deserializes_to_passthrough() {
        // This TOML has no [profiles.mic] entry — simulates a pre-mic-chain config.
        let toml_str = r#"
version = 1
active_profile = "default"

[[profiles]]
name = "default"

[[profiles.channels]]
id = "game"
node_name = "Arctis_Game"
description = "Game"

[[profiles.channels]]
id = "chat"
node_name = "Arctis_Chat"
description = "Chat"
"#;
        let cfg: Config = toml::from_str(toml_str).expect("should deserialize old config");
        let profile = cfg.active().expect("active profile");
        assert_eq!(
            profile.mic,
            MicChainConfig::passthrough(),
            "old config without mic block must deserialize to passthrough"
        );
    }

    /// 3. A fully enabled mic chain serializes and deserializes identically (round-trip).
    #[test]
    fn mic_toml_round_trips() {
        let mut cfg = Config::default_config();
        cfg.profiles[0].mic = MicChainConfig {
            enabled: true,
            hw_mic: Some("alsa_input.usb_headset".to_string()),
            gain: MicGainStage {
                enabled: true,
                gain_db: 3.0,
            },
            highpass: MicHighpassStage {
                enabled: true,
                freq_hz: 80.0,
            },
            suppression: MicSuppressionStage {
                enabled: true,
                backend: SuppressionBackend::DeepFilter,
                attenuation_limit_db: 100.0,
                vad_threshold: 40.0,
                vad_grace_ms: 800.0,
                vad_retro_grace_ms: 100.0,
            },
            compressor: MicCompressorStage {
                enabled: false,
                threshold_db: -18.0,
                ratio: 2.0,
                makeup_db: 4.0,
            },
            gate: MicGateStage {
                enabled: true,
                threshold: 0.003,
            },
            eq_enabled: true,
            eq: vec![EqBandConfig {
                kind: "lowshelf".to_string(),
                freq_hz: 120.0,
                q: 0.7,
                gain_db: 3.0,
            }],
        };

        let serialized = toml::to_string(&cfg).expect("serialize");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(
            cfg, deserialized,
            "mic TOML round-trip must preserve config"
        );
    }

    /// 4. Enabled suppression with out-of-range VAD threshold → ConfigError::Invalid.
    #[test]
    fn validate_rejects_out_of_range_enabled_vad() {
        let mut cfg = Config::default_config();
        cfg.profiles[0].mic.suppression.enabled = true;
        cfg.profiles[0].mic.suppression.backend = SuppressionBackend::Rnnoise;
        cfg.profiles[0].mic.suppression.vad_threshold = 150.0; // above max 99.0
        let err = cfg
            .validate()
            .expect_err("out-of-range VAD threshold should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    /// 5. Disabled suppression with out-of-range VAD threshold → Ok (disabled stages not validated).
    #[test]
    fn validate_ignores_out_of_range_disabled_stage() {
        let mut cfg = Config::default_config();
        cfg.profiles[0].mic.suppression.enabled = false;
        cfg.profiles[0].mic.suppression.vad_threshold = 150.0; // would be invalid if enabled
        assert!(
            cfg.validate().is_ok(),
            "disabled stage with out-of-range param should pass validation"
        );
    }

    /// 6. `Config::default_config()` validates and every profile's mic is passthrough.
    #[test]
    fn default_config_includes_mic_passthrough() {
        let cfg = Config::default_config();
        assert!(cfg.validate().is_ok(), "default_config must be valid");
        for profile in &cfg.profiles {
            assert_eq!(
                profile.mic,
                MicChainConfig::passthrough(),
                "profile '{}' mic should be passthrough",
                profile.name
            );
        }
    }

    // ── Compressor validation tests ───────────────────────────────────────────

    fn make_enabled_compressor(threshold_db: f32, ratio: f32, makeup_db: f32) -> Config {
        let mut cfg = Config::default_config();
        cfg.profiles[0].mic.compressor = MicCompressorStage {
            enabled: true,
            threshold_db,
            ratio,
            makeup_db,
        };
        cfg
    }

    /// Enabled compressor with ratio below minimum (0.0 < 1.0) → rejected.
    #[test]
    fn validate_rejects_enabled_compressor_ratio_below_min() {
        let err = make_enabled_compressor(-18.0, 0.0, 4.0)
            .validate()
            .expect_err("ratio=0.0 should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    /// Enabled compressor with threshold_db above maximum (100.0 > 0.0) → rejected.
    #[test]
    fn validate_rejects_enabled_compressor_threshold_above_max() {
        let err = make_enabled_compressor(100.0, 2.0, 4.0)
            .validate()
            .expect_err("threshold_db=100.0 should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    /// Enabled compressor with makeup_db above maximum (25.0 > 24.0) → rejected.
    #[test]
    fn validate_rejects_enabled_compressor_makeup_above_max() {
        let err = make_enabled_compressor(-18.0, 2.0, 25.0)
            .validate()
            .expect_err("makeup_db=25.0 should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    /// Enabled compressor with all params in range → accepted.
    #[test]
    fn validate_accepts_enabled_compressor_in_range() {
        assert!(
            make_enabled_compressor(-18.0, 2.0, 4.0).validate().is_ok(),
            "in-range enabled compressor should pass validation"
        );
    }

    /// Enabled compressor at exact boundary values → accepted.
    #[test]
    fn validate_accepts_enabled_compressor_at_boundaries() {
        // threshold min=-30, ratio min=1, makeup min=0
        assert!(
            make_enabled_compressor(-30.0, 1.0, 0.0).validate().is_ok(),
            "compressor at lower bounds should pass"
        );
        // threshold max=0, ratio max=20, makeup max=24
        assert!(
            make_enabled_compressor(0.0, 20.0, 24.0).validate().is_ok(),
            "compressor at upper bounds should pass"
        );
    }

    /// Disabled compressor with out-of-range params → accepted (disabled stages skipped).
    #[test]
    fn validate_ignores_disabled_compressor_out_of_range() {
        let mut cfg = Config::default_config();
        cfg.profiles[0].mic.compressor = MicCompressorStage {
            enabled: false,
            threshold_db: 100.0, // out of range
            ratio: 0.0,          // out of range
            makeup_db: 99.0,     // out of range
        };
        assert!(
            cfg.validate().is_ok(),
            "disabled compressor with out-of-range params should pass validation"
        );
    }

    // ── VAD threshold boundary tests ──────────────────────────────────────────

    /// VAD threshold at exact max (99.0) → accepted.
    #[test]
    fn validate_accepts_vad_threshold_at_max() {
        let mut cfg = Config::default_config();
        cfg.profiles[0].mic.suppression.enabled = true;
        cfg.profiles[0].mic.suppression.backend = SuppressionBackend::Rnnoise;
        cfg.profiles[0].mic.suppression.vad_threshold = 99.0;
        assert!(
            cfg.validate().is_ok(),
            "vad_threshold=99.0 (max) should be accepted"
        );
    }

    /// VAD threshold just above max (99.01) → rejected.
    #[test]
    fn validate_rejects_vad_threshold_just_above_max() {
        let mut cfg = Config::default_config();
        cfg.profiles[0].mic.suppression.enabled = true;
        cfg.profiles[0].mic.suppression.backend = SuppressionBackend::Rnnoise;
        cfg.profiles[0].mic.suppression.vad_threshold = 99.01;
        let err = cfg
            .validate()
            .expect_err("vad_threshold=99.01 should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    /// attenuation_limit_db out of range (enabled suppression with DeepFilter) → rejected.
    #[test]
    fn validate_rejects_attenuation_limit_out_of_range() {
        let mut cfg = Config::default_config();
        cfg.profiles[0].mic.suppression.enabled = true;
        cfg.profiles[0].mic.suppression.backend = SuppressionBackend::DeepFilter;
        cfg.profiles[0].mic.suppression.attenuation_limit_db = 150.0; // above max 100.0
        let err = cfg
            .validate()
            .expect_err("attenuation_limit_db=150.0 should be rejected");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    /// attenuation_limit_db at max (100.0) → accepted.
    #[test]
    fn validate_accepts_attenuation_limit_at_max() {
        let mut cfg = Config::default_config();
        cfg.profiles[0].mic.suppression.enabled = true;
        cfg.profiles[0].mic.suppression.backend = SuppressionBackend::DeepFilter;
        cfg.profiles[0].mic.suppression.attenuation_limit_db = 100.0;
        assert!(
            cfg.validate().is_ok(),
            "attenuation_limit_db=100.0 (max) should be accepted"
        );
    }

    /// Old config with `rnnoise` key deserializes via alias into `suppression`.
    #[test]
    fn old_rnnoise_key_deserializes_via_alias() {
        let toml_str = r#"
version = 1
active_profile = "default"

[[profiles]]
name = "default"

[[profiles.channels]]
id = "game"
node_name = "Arctis_Game"
description = "Game"

[[profiles.channels]]
id = "chat"
node_name = "Arctis_Chat"
description = "Chat"

[profiles.mic]
enabled = false

[profiles.mic.rnnoise]
enabled = true
vad_threshold = 55.0
"#;
        let cfg: Config = toml::from_str(toml_str).expect("old rnnoise key must deserialize");
        let profile = cfg.active().expect("active profile");
        assert!(
            profile.mic.suppression.enabled,
            "rnnoise.enabled must map to suppression.enabled"
        );
        assert!(
            (profile.mic.suppression.vad_threshold - 55.0).abs() < f32::EPSILON,
            "rnnoise.vad_threshold must map to suppression.vad_threshold"
        );
    }
}
