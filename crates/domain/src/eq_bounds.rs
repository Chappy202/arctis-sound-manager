/// Canonical EQ parameter bounds — the single source of truth shared by the
/// audio engine and the config layer.
///
/// Both `arctis-audio` and `arctis-config` derive their validation limits from
/// these constants, guaranteeing that a `Config` which passes `Config::validate()`
/// will never be rejected by the audio layer's own range checks.
pub const EQ_GAIN_MIN_DB: f32 = -12.0;
pub const EQ_GAIN_MAX_DB: f32 = 12.0;
pub const EQ_Q_MIN: f32 = 0.3;
pub const EQ_Q_MAX: f32 = 10.0;
pub const EQ_FREQ_MIN_HZ: f32 = 20.0;
pub const EQ_FREQ_MAX_HZ: f32 = 20_000.0;

// ── Mic-stage bounds ──────────────────────────────────────────────────────────

/// Mic gain stage (linear node, dB; converted to linear Mult at render time).
pub const MIC_GAIN_MIN_DB: f32 = -20.0;
pub const MIC_GAIN_MAX_DB: f32 = 30.0;

/// Highpass cutoff range.
pub const MIC_HIGHPASS_MIN_HZ: f32 = 40.0;
pub const MIC_HIGHPASS_MAX_HZ: f32 = 300.0;

/// RNNoise VAD threshold (%) — mirrors plugin port range.
pub const MIC_VAD_THRESHOLD_MIN: f32 = 0.0;
pub const MIC_VAD_THRESHOLD_MAX: f32 = 99.0;

/// RNNoise VAD grace period (ms) — mirrors plugin port range.
pub const MIC_VAD_GRACE_MIN_MS: f32 = 0.0;
pub const MIC_VAD_GRACE_MAX_MS: f32 = 1000.0;

/// RNNoise retroactive VAD grace (ms) — mirrors plugin port range.
pub const MIC_VAD_RETRO_GRACE_MIN_MS: f32 = 0.0;
pub const MIC_VAD_RETRO_GRACE_MAX_MS: f32 = 200.0;

/// Noise gate open threshold (linear 0..1).
pub const MIC_GATE_THRESHOLD_MIN: f32 = 0.0;
pub const MIC_GATE_THRESHOLD_MAX: f32 = 0.5;

/// sc4m LADSPA compressor threshold (dB).
pub const MIC_COMP_THRESHOLD_MIN_DB: f32 = -30.0;
pub const MIC_COMP_THRESHOLD_MAX_DB: f32 = 0.0;

/// sc4m LADSPA compressor ratio (1:n).
pub const MIC_COMP_RATIO_MIN: f32 = 1.0;
pub const MIC_COMP_RATIO_MAX: f32 = 20.0;

/// sc4m LADSPA compressor makeup gain (dB).
pub const MIC_COMP_MAKEUP_MIN_DB: f32 = 0.0;
pub const MIC_COMP_MAKEUP_MAX_DB: f32 = 24.0;

/// DeepFilterNet attenuation limit (dB). 0 = no suppression, 100 = maximum suppression.
pub const MIC_ATTEN_LIMIT_MIN_DB: f32 = 0.0;
pub const MIC_ATTEN_LIMIT_MAX_DB: f32 = 100.0;

/// Per-channel software volume bounds (dB → linear via 10^(db/20)).
pub const CHANNEL_VOLUME_MIN_DB: f32 = -60.0;
pub const CHANNEL_VOLUME_MAX_DB: f32 = 6.0;
