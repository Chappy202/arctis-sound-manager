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
