use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceId {
    pub vendor_id: u16,
    pub product_id: u16,
}

impl DeviceId {
    pub fn new(vendor_id: u16, product_id: u16) -> Self {
        Self {
            vendor_id,
            product_id,
        }
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04x}:{:04x}", self.vendor_id, self.product_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_displays_as_colon_separated_hex() {
        assert_eq!(DeviceId::new(0x1038, 0x12e5).to_string(), "1038:12e5");
    }
}
