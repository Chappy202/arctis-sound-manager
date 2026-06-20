//! Pure domain types for Arctis Sound Manager. No I/O.
pub mod capability;
pub mod device;
pub mod eq_bounds;
pub mod status;

pub use capability::Capability;
pub use device::DeviceId;
pub use eq_bounds::{
    EQ_FREQ_MAX_HZ, EQ_FREQ_MIN_HZ, EQ_GAIN_MAX_DB, EQ_GAIN_MIN_DB, EQ_Q_MAX, EQ_Q_MIN,
};
pub use status::{DeviceState, StatusValue};
