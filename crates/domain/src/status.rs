use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A decoded status field value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum StatusValue {
    Percentage(u8),
    Bool(bool),
    Enum(String),
    Int(i64),
}

/// A snapshot of decoded device state, keyed by field name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceState {
    pub fields: BTreeMap<String, StatusValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_state_inserts_and_reads_fields() {
        let mut s = DeviceState::default();
        s.fields
            .insert("battery".into(), StatusValue::Percentage(75));
        assert_eq!(s.fields.get("battery"), Some(&StatusValue::Percentage(75)));
    }

    #[test]
    fn status_value_percentage_round_trips_through_json() {
        let original = StatusValue::Percentage(50);
        let json = serde_json::to_string(&original).unwrap();
        let restored: StatusValue = serde_json::from_str(&json).unwrap();
        assert_eq!(
            original, restored,
            "Percentage(50) must survive a JSON round-trip"
        );
    }

    #[test]
    fn status_value_percentage_json_contains_kind_tag_snake_case() {
        let json = serde_json::to_string(&StatusValue::Percentage(50)).unwrap();
        assert!(
            json.contains(r#""kind":"percentage""#),
            "serialized Percentage must contain the snake_case kind tag; got: {json}"
        );
    }
}
