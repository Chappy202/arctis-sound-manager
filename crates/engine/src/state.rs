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
    /// The pinned hardware mic capture source (if any).  `None` means auto / not pinned.
    /// Populated from `profile.mic.hw_mic`; `None` is serde-default for back-compat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hw_mic: Option<String>,
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

/// Lightweight summary of one EQ preset for the state snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqPresetSnapshot {
    pub name: String,
    pub band_count: usize,
}

/// Lightweight summary of one microphone preset for the state snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MicPresetSnapshot {
    pub name: String,
    pub description: String,
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
    #[serde(default)]
    pub eq_presets: Vec<EqPresetSnapshot>,
    /// Master output gain in dB (0.0 = unity). Populated from the active profile.
    #[serde(default)]
    pub master_volume_db: f32,
    /// Whether the master output is muted.
    #[serde(default)]
    pub master_mute: bool,
    /// ChatMix position 0..=9 (0 = full chat, 9 = full game, 4 = balanced).
    #[serde(default)]
    pub chatmix_position: i64,
    /// Channel id whose sink is the system default output, or None.
    #[serde(default)]
    pub default_sink_channel: Option<String>,
    /// When true the hardware dial owns ChatMix balance; the GUI slider is read-only.
    /// When false the GUI slider drives balance (dial_controls_balance=false in config).
    #[serde(default)]
    pub dial_controls_balance: bool,
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
    /// Software volume in dB. 0.0 = unity gain.
    pub volume_db: f32,
    /// Whether the channel is muted.
    pub muted: bool,
}

/// One real audio output device discovered via `pw-dump`, for the per-channel output selector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputDeviceSnapshot {
    pub node_name: String,
    pub description: String,
    pub is_default: bool,
}

/// One running application audio stream, resolved to a channel id, for the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppStream {
    pub id: u32,
    pub binary: String,
    pub app_name: String,
    pub pid: Option<u32>,
    pub icon_name: Option<String>,
    pub media_name: Option<String>,
    /// Resolved channel id, or None = unrouted (shown in the Master tray).
    pub current_channel: Option<String>,
    /// True when a persistent routing rule exists for this binary.
    pub routed: bool,
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
    ProfileRenamed {
        old: String,
        new: String,
    },
    ProfileDeleted {
        name: String,
    },
    ProfileImported {
        name: String,
    },
    EqPresetSaved {
        name: String,
    },
    EqPresetApplied {
        name: String,
        channel_id: String,
    },
    EqPresetDeleted {
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
    ChannelVolumeSet {
        channel_id: String,
        volume_db: f32,
    },
    ChannelMuteSet {
        channel_id: String,
        muted: bool,
    },
    RouteSet {
        app_binary: String,
        target_sink: String,
    },
    RouteCleared {
        app_binary: String,
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
    ChannelAdded {
        id: String,
    },
    ChannelRemoved {
        id: String,
    },
    MasterVolumeSet {
        volume_db: f32,
    },
    MasterMuteSet {
        muted: bool,
    },
    ChatmixSet {
        position: i64,
    },
    DefaultSinkChannelSet {
        channel: Option<String>,
    },
}
