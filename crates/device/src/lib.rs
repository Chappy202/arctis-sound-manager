//! Data-driven HID device layer: descriptors, registry, transport, codec.
pub mod codec;
pub mod descriptor;
pub mod mock;
pub mod registry;
pub mod transport;

pub use codec::{decode_frame, read_status};
pub use descriptor::{
    parse_descriptor, DeviceDescriptor, EnumEntry, Parser, StatusField, StatusSpec,
};
pub use mock::MockTransport;
pub use registry::{Registry, RegistryError};
pub use transport::{Transport, TransportError};
