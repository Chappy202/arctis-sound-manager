use serde::{Deserialize, Serialize};

/// A discrete feature a device may support. Drives both what the engine sends
/// and what the UI renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Battery,
    Sidetone,
    Anc,
    MicVolume,
    InactiveTime,
    HardwareEq,
    EqPreset,
    ChatMix,
    WirelessMode,
    MicLed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&Capability::MicVolume).unwrap(),
            r#""mic_volume""#,
            "MicVolume must serialize to snake_case JSON string"
        );
    }

    #[test]
    fn capability_round_trips_through_json() {
        let original = Capability::MicVolume;
        let json = serde_json::to_string(&original).unwrap();
        let restored: Capability = serde_json::from_str(&json).unwrap();
        assert_eq!(
            original, restored,
            "Capability must survive a JSON round-trip"
        );
    }
}
