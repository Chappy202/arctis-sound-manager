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
    /// Declarative write commands, keyed by control name. Default empty so
    /// existing read-only descriptors parse unchanged.
    #[serde(default)]
    pub commands: std::collections::BTreeMap<String, CommandSpec>,
    /// The documented save/commit opcode bytes (after report id), e.g. [0x09].
    /// Sent as its own single report when a command has `save = true`.
    #[serde(default)]
    pub save_command: Option<Vec<u8>>,
    /// Ordered raw init reports sent ONCE on attach (each is a FULL report
    /// including the leading report-id byte; padded to `report_length` before
    /// write). Unlike `commands`, these are not value-encoded or per-command
    /// gated — they are a fixed, owner-validated device-init sequence (see the
    /// descriptor TOML for per-opcode provenance). Empty by default so other
    /// descriptors parse unchanged.
    #[serde(default)]
    pub init_writes: Vec<Vec<u8>>,
}

/// A single declarative write command: which opcode bytes to send and how to
/// encode the user-supplied value into the final wire byte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    /// Bytes appended after the report id, BEFORE the encoded value byte.
    /// e.g. [0x39] for sidetone -> wire = [report_id, 0x39, <value>].
    pub opcode: Vec<u8>,
    /// The Capability that must be present in `capabilities` for this command
    /// to be enabled. A command whose capability is absent is NOT callable.
    pub capability: Capability,
    /// How to turn the user's requested value into the single wire value byte.
    pub encoding: ValueEncoding,
    /// When true, append a SECOND discrete write of the documented save opcode
    /// after the command write. SAFETY: this is the one allowed extra write and
    /// it is ALWAYS exactly the descriptor's `save_command` bytes — never a burst.
    #[serde(default)]
    pub save: bool,
}

/// Value encoding for a command's single value byte. Mirrors the read `Parser`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ValueEncoding {
    /// Validate the integer against [min, max] and send it directly as one byte.
    /// Values outside the range are rejected with an error — no clamping occurs.
    IntRange { min: u8, max: u8 },
    /// Map a named choice to a fixed byte. Used for enums like ANC mode.
    Enum { entries: Vec<EnumEntry> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusSpec {
    /// Bytes appended after the report id to request a status frame.
    pub request: Vec<u8>,
    #[serde(default)]
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
    Percentage {
        min: u8,
        max: u8,
        /// When true, the scale is inverted (raw `max` → 0 %, raw `min` → 100 %).
        /// `serde` default = false keeps existing percentage fields unchanged.
        #[serde(default)]
        invert: bool,
    },
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
    fn parses_a_command_entry_with_int_range_and_save() {
        let src = r#"
            name = "T"
            vendor_id = 0x1038
            product_ids = [0x12e5]
            interface = 4
            report_id = 0x06
            report_length = 64
            capabilities = ["sidetone"]
            save_command = [0x09]

            [status]
            request = [0xb0]

            [commands.sidetone]
            opcode = [0x39]
            capability = "sidetone"
            save = true
            encoding = { type = "int_range", min = 0, max = 3 }
        "#;
        let d = parse_descriptor(src).expect("should parse");
        let c = d.commands.get("sidetone").expect("sidetone command");
        assert_eq!(c.opcode, vec![0x39]);
        assert_eq!(c.capability, Capability::Sidetone);
        assert!(c.save);
        assert_eq!(c.encoding, ValueEncoding::IntRange { min: 0, max: 3 });
        assert_eq!(d.save_command, Some(vec![0x09]));
    }

    #[test]
    fn nova_descriptor_parses_23_init_writes() {
        use crate::registry::Registry;
        use arctis_domain::DeviceId;
        let d = Registry::builtin()
            .unwrap()
            .find(DeviceId::new(0x1038, 0x12e5))
            .unwrap()
            .clone();
        assert_eq!(
            d.init_writes.len(),
            23,
            "Nova descriptor must have exactly 23 init_writes"
        );
        // The ChatMix dial-enable report is the 17th entry (0-based index 16).
        assert_eq!(
            d.init_writes[16],
            vec![0x06, 0x49, 0x01],
            "init_writes[16] must be the ChatMix dial-enable opcode [0x06,0x49,0x01]"
        );
        // First report is a wake/probe.
        assert_eq!(
            d.init_writes[0],
            vec![0x06, 0x20],
            "init_writes[0] must be the wake/probe [0x06,0x20]"
        );
    }

    #[test]
    fn nova_descriptor_has_station_volume_inverted_percentage_field() {
        use crate::registry::Registry;
        use arctis_domain::DeviceId;
        let d = Registry::builtin()
            .unwrap()
            .find(DeviceId::new(0x1038, 0x12e5))
            .unwrap()
            .clone();
        let field = d
            .status
            .fields
            .iter()
            .find(|f| f.name == "station_volume")
            .expect("station_volume field must be present");
        assert_eq!(field.match_prefix, vec![0x07, 0x25]);
        assert_eq!(field.offset, 2);
        assert_eq!(
            field.parser,
            Parser::Percentage {
                min: 0,
                max: 56,
                invert: true
            },
            "station_volume must use an inverted 0..56 percentage parser"
        );
    }

    #[test]
    fn existing_read_only_descriptor_parses_with_empty_commands() {
        // The minimal descriptor (no [commands]) must still parse; commands empty.
        let src = r#"
            name = "T"
            vendor_id = 0x1038
            product_ids = [0x12e5]
            interface = 4
            report_id = 0x06
            report_length = 64
            capabilities = []
            [status]
            request = [0xb0]
        "#;
        let d = parse_descriptor(src).expect("should parse");
        assert!(d.commands.is_empty());
        assert_eq!(d.save_command, None);
    }

    #[test]
    fn parses_a_command_entry_with_enum_encoding() {
        let src = r#"
            name = "T"
            vendor_id = 0x1038
            product_ids = [0x12e5]
            interface = 4
            report_id = 0x06
            report_length = 64
            capabilities = ["inactive_time"]
            save_command = [0x09]

            [status]
            request = [0xb0]

            [commands.inactive_time]
            opcode = [0xc1]
            capability = "inactive_time"
            save = true
            encoding = { type = "enum", entries = [
              { value = 0, label = "never" },
              { value = 1, label = "1min" },
              { value = 2, label = "5min" },
              { value = 3, label = "10min" },
              { value = 4, label = "15min" },
              { value = 5, label = "30min" },
              { value = 6, label = "60min" },
            ] }
        "#;
        let d = parse_descriptor(src).expect("should parse");
        let c = d
            .commands
            .get("inactive_time")
            .expect("inactive_time command");
        assert_eq!(c.opcode, vec![0xc1]);
        assert_eq!(c.capability, Capability::InactiveTime);
        assert!(c.save);
        match &c.encoding {
            ValueEncoding::Enum { entries } => {
                assert_eq!(entries.len(), 7);
                assert_eq!(
                    entries[0],
                    EnumEntry {
                        value: 0,
                        label: "never".to_string()
                    }
                );
                assert_eq!(
                    entries[6],
                    EnumEntry {
                        value: 6,
                        label: "60min".to_string()
                    }
                );
            }
            other => panic!("expected Enum encoding, got {other:?}"),
        }
    }

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
            Parser::Percentage {
                min: 0,
                max: 8,
                invert: false
            }
        );
    }
}
