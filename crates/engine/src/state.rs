use serde::{Deserialize, Serialize};

/// A flat, UI-agnostic snapshot the CLI/daemon/(future UI) render.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineState {
    pub active_profile: String,
    pub profiles: Vec<String>,
    pub channels: Vec<ChannelSnapshot>,
    pub routes: Vec<(String, String)>, // (app_binary, target_sink)
    pub device_present: bool,
    pub device_fields: std::collections::BTreeMap<String, String>, // best-effort, may be empty
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelSnapshot {
    pub id: String,
    pub node_name: String,
    pub output_device: Option<String>,
    pub eq_bands: usize,
}

/// Events emitted on the engine's outbound stream (mpsc::Receiver<Event> for the daemon/UI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    ProfileSwitched {
        name: String,
    },
    Reconciled,
    EqBandSet {
        channel_id: String,
        band: usize,
    },
    RouteSet {
        app_binary: String,
        target_sink: String,
    },
    DeviceState {
        fields: std::collections::BTreeMap<String, String>,
    },
}
