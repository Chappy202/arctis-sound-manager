//! Data-driven HID device layer: descriptors, registry, transport, codec.
pub mod descriptor;
pub mod registry;

pub use descriptor::{
    parse_descriptor, DeviceDescriptor, EnumEntry, Parser, StatusField, StatusSpec,
};
pub use registry::{Registry, RegistryError};
