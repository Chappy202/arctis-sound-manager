use crate::descriptor::{parse_descriptor, DeviceDescriptor};
use arctis_domain::DeviceId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("failed to parse built-in descriptor '{0}': {1}")]
    Parse(String, String),
}

/// Built-in descriptors compiled into the binary.
const NOVA_PRO_WIRELESS: &str = include_str!("../../../devices/steelseries_nova_pro_wireless.toml");

pub struct Registry {
    descriptors: Vec<DeviceDescriptor>,
}

impl Registry {
    pub fn from_descriptors(descriptors: Vec<DeviceDescriptor>) -> Self {
        Self { descriptors }
    }

    /// Load all descriptors compiled into the binary.
    pub fn builtin() -> Result<Self, RegistryError> {
        let nova = parse_descriptor(NOVA_PRO_WIRELESS)
            .map_err(|e| RegistryError::Parse("nova_pro_wireless".into(), e.to_string()))?;
        Ok(Self::from_descriptors(vec![nova]))
    }

    /// Find a descriptor matching a connected device's id.
    pub fn find(&self, id: DeviceId) -> Option<&DeviceDescriptor> {
        self.descriptors
            .iter()
            .find(|d| d.vendor_id == id.vendor_id && d.product_ids.contains(&id.product_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_loads_and_finds_nova_pro_by_either_pid() {
        let reg = Registry::builtin().expect("builtin descriptors must parse");
        let std = reg.find(DeviceId::new(0x1038, 0x12e0));
        let x = reg.find(DeviceId::new(0x1038, 0x12e5));
        assert!(std.is_some(), "should match standard pid 12e0");
        assert!(x.is_some(), "should match X pid 12e5");
        assert_eq!(std.unwrap().name, "Arctis Nova Pro Wireless");
    }

    #[test]
    fn unknown_device_is_not_found() {
        let reg = Registry::builtin().unwrap();
        assert!(reg.find(DeviceId::new(0x1234, 0x5678)).is_none());
    }
}
