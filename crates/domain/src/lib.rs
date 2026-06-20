//! Pure domain types for Arctis Sound Manager. No I/O.
pub mod capability;
pub mod device;
pub mod eq_bounds;
pub mod status;

pub use capability::Capability;
pub use device::DeviceId;
pub use eq_bounds::{
    EQ_FREQ_MAX_HZ, EQ_FREQ_MIN_HZ, EQ_GAIN_MAX_DB, EQ_GAIN_MIN_DB, EQ_Q_MAX, EQ_Q_MIN,
    MIC_GAIN_MAX_DB, MIC_GAIN_MIN_DB, MIC_GATE_THRESHOLD_MAX, MIC_GATE_THRESHOLD_MIN,
    MIC_HIGHPASS_MAX_HZ, MIC_HIGHPASS_MIN_HZ, MIC_VAD_GRACE_MAX_MS, MIC_VAD_GRACE_MIN_MS,
    MIC_VAD_RETRO_GRACE_MAX_MS, MIC_VAD_RETRO_GRACE_MIN_MS, MIC_VAD_THRESHOLD_MAX,
    MIC_VAD_THRESHOLD_MIN,
};
pub use status::{DeviceState, StatusValue};
