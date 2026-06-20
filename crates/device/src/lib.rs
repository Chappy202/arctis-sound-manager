//! Data-driven HID device layer: descriptors, registry, transport, codec.
pub mod codec;
pub mod controller;
pub mod descriptor;
pub mod error;
pub mod hidraw;
pub mod mock;
pub mod registry;
pub mod transport;

pub use codec::{decode_frame, read_status};
pub use controller::DeviceController;
pub use descriptor::{
    parse_descriptor, CommandSpec, DeviceDescriptor, EnumEntry, Parser, StatusField, StatusSpec,
    ValueEncoding,
};
pub use error::DeviceError;
pub use hidraw::{discover, HidrawTransport};
pub use mock::MockTransport;
pub use registry::{Registry, RegistryError};
pub use transport::{Transport, TransportError};
