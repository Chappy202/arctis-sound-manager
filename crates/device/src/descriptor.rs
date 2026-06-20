use arctis_domain::Capability;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    pub name: String,
    pub vendor_id: u16,
    pub product_ids: Vec<u16>,
    pub interface: u8,
    pub report_id: u8,
    pub report_length: usize,
    pub capabilities: Vec<Capability>,
    pub status: StatusSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusSpec {
    /// Bytes appended after the report id to request a status frame.
    pub request: Vec<u8>,
    pub fields: Vec<StatusField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusField {
    pub name: String,
    pub match_prefix: Vec<u8>,
    pub offset: usize,
    pub parser: Parser,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Parser {
    Percentage { min: u8, max: u8 },
    Bool { true_value: u8 },
    Enum { entries: Vec<EnumEntry> },
    Int,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumEntry {
    pub value: u8,
    pub label: String,
}

/// Parse a device descriptor from TOML source.
pub fn parse_descriptor(src: &str) -> Result<DeviceDescriptor, toml::de::Error> {
    toml::from_str(src)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_descriptor() {
        let src = r#"
            name = "Test Device"
            vendor_id = 0x1038
            product_ids = [0x12e0, 0x12e5]
            interface = 4
            report_id = 0x06
            report_length = 64
            capabilities = ["battery"]

            [status]
            request = [0xb0]

            [[status.fields]]
            name = "battery_charge"
            match_prefix = [0x06, 0xb0]
            offset = 6
            parser = { type = "percentage", min = 0, max = 8 }
        "#;
        let d = parse_descriptor(src).expect("should parse");
        assert_eq!(d.name, "Test Device");
        assert_eq!(d.product_ids, vec![0x12e0, 0x12e5]);
        assert_eq!(d.capabilities, vec![Capability::Battery]);
        assert_eq!(d.status.request, vec![0xb0]);
        assert_eq!(d.status.fields[0].offset, 6);
        assert_eq!(
            d.status.fields[0].parser,
            Parser::Percentage { min: 0, max: 8 }
        );
    }
}
