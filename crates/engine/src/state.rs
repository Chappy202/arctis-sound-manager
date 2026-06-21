use arctis_audio::StageKind;
use serde::{Deserialize, Serialize};

/// Which noise-suppression backend is in use (mirrors config's SuppressionBackend but
/// defined here to avoid a config→state or state→config dep cycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SuppressionBackend {
    #[default]
    DeepFilter,
    Rnnoise,
}

/// Tunable mic DSP parameters. Used by `Engine::mic_set_param`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MicParam {
    GainDb,
    HighpassFreq,
    VadThreshold,
    VadGraceMs,
    VadRetroGraceMs,
    GateThreshold,
    AttenuationLimitDb,
    CompThresholdDb,
    CompRatio,
    CompMakeupDb,
}

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

/// Availability of a single mic DSP stage, as detected during the last reconcile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageAvailability {
    pub stage: StageName,
    /// True when the stage's plugin/builtin is present on the system.
    pub available: bool,
    /// True when the stage is enabled in the config (i.e. was requested).
    pub requested: bool,
}

/// Serializable stage name (mirrors `StageKind` but Serialize/Deserialize).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageName {
    Gain,
    Highpass,
    Suppression,
    Compressor,
    Gate,
    MicEq,
}

impl From<StageKind> for StageName {
    fn from(k: StageKind) -> Self {
        match k {
            StageKind::Gain => StageName::Gain,
            StageKind::Highpass => StageName::Highpass,
            StageKind::Suppression => StageName::Suppression,
            StageKind::Compressor => StageName::Compressor,
            StageKind::Gate => StageName::Gate,
            StageKind::MicEq => StageName::MicEq,
        }
    }
}

/// Snapshot of one mic DSP stage: kind, enabled in config, available on system, params map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicStageSnapshot {
    pub kind: StageName,
    pub enabled: bool,
    pub available: bool,
    /// Human-readable param name → value. Populated for enabled stages; empty for disabled.
    pub params: std::collections::BTreeMap<String, f32>,
}

/// Full mic chain snapshot returned in `EngineState`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MicSnapshot {
    pub enabled: bool,
    pub stages: Vec<MicStageSnapshot>,
    pub eq_bands: Vec<EqBandSnapshot>,
    /// The active suppression backend (DeepFilter by default).
    #[serde(default)]
    pub suppression_backend: SuppressionBackend,
    /// Backends whose LADSPA .so was found by the last probe.
    #[serde(default)]
    pub available_suppression_backends: Vec<SuppressionBackend>,
}

/// Full surround snapshot returned in `EngineState`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SurroundSnapshot {
    pub enabled: bool,
    pub hrir: Option<String>,
    pub available_hrirs: Vec<String>,
    pub channels: Vec<String>,
    pub hw_sink: Option<String>,
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
    pub mic: MicSnapshot,
    #[serde(default)]
    pub surround: SurroundSnapshot,
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
    MicStageSet {
        stage: StageName,
        enabled: bool,
    },
    MicParamSet {
        param: MicParam,
        value: f32,
    },
    MicEqBandSet {
        band: usize,
    },
    MicHwMicSet {
        hw_mic: Option<String>,
    },
    MicEnabledSet {
        enabled: bool,
    },
    MicSuppressionBackendSet {
        backend: SuppressionBackend,
    },
    SurroundEnabledSet {
        enabled: bool,
    },
    SurroundHrirSet {
        hrir: Option<String>,
    },
    SurroundChannelsSet {
        channels: Vec<String>,
    },
    SurroundHwSinkSet {
        hw_sink: Option<String>,
    },
}
