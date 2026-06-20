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
