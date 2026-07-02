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

fn default_vol_pct_100() -> u8 {
    100
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
    /// Mic output volume as 0–100 percent. 100 = unity.
    #[serde(default = "default_vol_pct_100")]
    pub volume_pct: u8,
}

/// Richer catalog entry for one available HRIR: stem, human-readable display name,
/// vendor group, and tonality string.  Surfaces in `SurroundSnapshot` so the UI
/// can render a grouped picker without a separate catalog call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HrirEntrySnapshot {
    pub stem: String,
    pub display: String,
    pub group: String,
    pub tonality: String,
}

/// Full surround snapshot returned in `EngineState`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SurroundSnapshot {
    pub enabled: bool,
    pub hrir: Option<String>,
    pub available_hrirs: Vec<String>,
    #[serde(default)]
    pub available_hrir_entries: Vec<HrirEntrySnapshot>,
    pub channels: Vec<String>,
    pub hw_sink: Option<String>,
    /// Configured surround mode as a lowercase string (e.g. `"auto"`, `"hrir71"`, `"stereo_bypass"`).
    /// Old engine versions omit this field; serde default = `""`.
    #[serde(default)]
    pub mode: String,
    /// Resolved effective mode after applying fallback logic, as a lowercase string.
    /// Old engine versions omit this field; serde default = `""`.
    #[serde(default)]
    pub effective_mode: String,
    /// Hardware-negotiated channel count (from pw-dump probe), if available.
    /// `None` = not yet probed. Old engine versions omit this field.
    #[serde(default)]
    pub negotiated_channels: Option<u8>,
    /// Whether the negotiated surround input has a rear/side channel (true 7.1/5.1)
    /// vs only stereo. `None` = no probe / no source feeding a surround channel.
    /// Old engine versions omit this field.
    #[serde(default)]
    pub negotiated_surround: Option<bool>,
    /// Pinned HRIR stem that was requested but not installed (a fallback is in use).
    /// `None` = the pinned/selected HRIR resolved normally. UI shows an import prompt when set.
    #[serde(default)]
    pub hrir_missing: Option<String>,
    /// Convolver partition size, if pinned by the profile.
    #[serde(default)]
    pub blocksize: Option<u32>,
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
    /// Master output volume as 0–100 percent. 100 = unity / 0 dB.
    #[serde(default = "default_vol_pct_100")]
    pub master_volume_pct: u8,
    /// Read-only factory EQ preset catalog. Always populated regardless of user presets.
    #[serde(default)]
    pub factory_eq_presets: Vec<EqPresetSnapshot>,
    /// Read-only factory mic preset catalog.
    #[serde(default)]
    pub mic_presets: Vec<MicPresetSnapshot>,
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
    /// When true the base-station volume KNOB mirrors its position into `master_volume_pct`
    /// (read-only hardware mirror; knob_controls_master=true in config).
    #[serde(default)]
    pub knob_controls_master: bool,
    /// True when the daemon could not read the persisted config at startup and is
    /// running on defaults (the unreadable file was moved aside). Clients should
    /// warn: the first mutation persists the defaults as the new config.
    #[serde(default)]
    pub config_degraded: bool,
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
    /// Software volume as 0–100 percent. 100 = unity / 0 dB.
    pub volume_pct: u8,
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
    MicPresetApplied {
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
        volume_pct: u8,
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
        volume_pct: u8,
    },
    MasterMuteSet {
        muted: bool,
    },
    MicVolumeSet {
        volume_pct: u8,
    },
    ChatmixSet {
        position: i64,
    },
    DefaultSinkChannelSet {
        channel: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surround_snapshot_defaults_have_no_missing_and_no_blocksize() {
        let s = SurroundSnapshot::default();
        assert_eq!(s.hrir_missing, None);
        assert_eq!(s.blocksize, None);
    }

    #[test]
    fn surround_snapshot_old_json_defaults_new_fields() {
        // JSON from an older engine omits the new fields.
        let json = r#"{"enabled":false,"hrir":null,"available_hrirs":[],"channels":[],"hw_sink":null}"#;
        let s: SurroundSnapshot = serde_json::from_str(json).expect("deserialize old snapshot");
        assert_eq!(s.hrir_missing, None);
        assert_eq!(s.blocksize, None);
    }
}
