//! Data-driven HID device layer: descriptors, registry, transport, codec.
pub mod descriptor;

pub use descriptor::{
    parse_descriptor, DeviceDescriptor, EnumEntry, Parser, StatusField, StatusSpec,
};
