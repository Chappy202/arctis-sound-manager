use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A decoded status field value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
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
    fn device_state_round_trips_through_json_like_map() {
        let mut s = DeviceState::default();
        s.fields
            .insert("battery".into(), StatusValue::Percentage(75));
        assert_eq!(s.fields.get("battery"), Some(&StatusValue::Percentage(75)));
    }
}
