//! Pure domain types for Arctis Sound Manager. No I/O.
pub mod capability;
pub mod device;
pub mod status;

pub use capability::Capability;
pub use device::DeviceId;
pub use status::{DeviceState, StatusValue};
