use serde::{Deserialize, Serialize};

/// Shared device state mutated by the DeviceWorker and read by engine::state().
#[derive(Default, Clone)]
pub struct DeviceShared {
    pub present: bool,
    pub fields: std::collections::BTreeMap<String, String>,
}

/// Convert a DeviceState (BTreeMap<String, StatusValue>) into the flat
/// BTreeMap<String, String> stored in DeviceShared / EngineState::device_fields.
pub fn render_device_fields(
    state: &arctis_domain::DeviceState,
) -> std::collections::BTreeMap<String, String> {
    use arctis_domain::StatusValue;
    state
        .fields
        .iter()
        .map(|(k, v)| {
            let s = match v {
                StatusValue::Percentage(p) => format!("{p}"),
                StatusValue::Bool(b) => b.to_string(),
                StatusValue::Enum(e) => e.clone(),
                StatusValue::Int(i) => i.to_string(),
            };
            (k.clone(), s)
        })
        .collect()
}

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

/// Full snapshot of a single EQ band — carries all four parameters so the UI
/// can render the current curve without round-tripping to get-state again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqBandSnapshot {
    pub kind: String,
    pub freq_hz: f32,
    pub q: f32,
    pub gain_db: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelSnapshot {
    pub id: String,
    pub node_name: String,
    pub output_device: Option<String>,
    /// Full per-band EQ state. Empty means flat / no overrides configured.
    pub eq_bands: Vec<EqBandSnapshot>,
}

/// Events emitted on the engine's outbound stream (mpsc::Receiver<Event> for the daemon/UI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    ProfileSwitched {
        name: String,
    },
    ProfileCreated {
        name: String,
    },
    Reconciled,
    EqBandSet {
        channel_id: String,
        band: usize,
    },
    ChannelOutputSet {
        channel_id: String,
        device: Option<String>,
    },
    RouteSet {
        app_binary: String,
        target_sink: String,
    },
    DeviceState {
        fields: std::collections::BTreeMap<String, String>,
    },
}
