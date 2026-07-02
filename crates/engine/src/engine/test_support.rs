//! Shared fixtures and mock-output builders for the engine unit tests.
use super::*;
pub(super) use arctis_audio::MockRunner;
pub(super) use arctis_config::{ChannelConfig, MicChainConfig, Profile};

/// Global mutex to serialize tests that mutate process-wide env vars (HOME, ASM_CONFIG_HOME).
/// Tests setting those variables MUST hold this lock for their entire lifetime.
pub(super) static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ─────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────

/// Config with 3 channels (game/chat/media), no EQ overrides, no output overrides, no routes.
pub(super) fn make_config_no_eq_no_routes() -> Config {
    Config {
        version: arctis_config::CURRENT_VERSION,
        active_profile: "default".into(),
        profiles: vec![Profile {
            name: "default".into(),
            channels: vec![
                ChannelConfig {
                    id: "game".into(),
                    node_name: "Arctis_Game".into(),
                    description: "Game".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
                ChannelConfig {
                    id: "chat".into(),
                    node_name: "Arctis_Chat".into(),
                    description: "Chat".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
                ChannelConfig {
                    id: "media".into(),
                    node_name: "Arctis_Media".into(),
                    description: "Media".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
            ],
            routes: vec![],
            mic: MicChainConfig::default(),
            surround: arctis_config::SurroundConfig::default(),
            master_volume_db: 0.0,
            master_volume_pct: 100,
            master_mute: false,
            chatmix_position: 4,
            default_sink_channel: None,
        }],
        eq_presets: vec![],
        dial_controls_balance: true,
        knob_controls_master: true,
    }
}

// ─────────────────────────────────────────────
// TDD Step 5: reconcile argv-sequence tests
// ─────────────────────────────────────────────

/// Build the "ls Node" response that reports the three Arctis sinks as already present.
/// format: `id <N>\n    node.name = "<name>"\n`
pub(super) fn ls_all_present() -> String {
    [
        "id 10\n    node.name = \"Arctis_Game\"\n",
        "id 11\n    node.name = \"Arctis_Chat\"\n",
        "id 12\n    node.name = \"Arctis_Media\"\n",
        "id 13\n    node.name = \"Arctis_Aux\"\n",
    ]
    .concat()
}

/// Build the "ls Node" response where all sinks are absent (only unrelated node).
pub(super) fn ls_all_absent() -> String {
    "id 1\n    node.name = \"other_sink\"\n".to_string()
}

// ─────────────────────────────────────────────
// TDD Step 1: Task 6 — state / switch / mutation / events
// ─────────────────────────────────────────────

/// Helper: create a unique temp dir (does NOT touch HOME / XDG / real FS).
pub(super) fn unique_cfg_tmp(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    std::env::temp_dir().join(format!(
        "asm_eng6_{tag}_{pid}_{nanos}",
        pid = std::process::id()
    ))
}

/// Queue enough MockRunner outputs to survive `reconcile()` on a 4-channel
/// (game/chat/media/aux), no-EQ, no-routes config where all sinks are already
/// present AND mic disabled. Engine::new calls ensure_standard_channels() which
/// adds "aux" to any 3-channel config, making this a 4-channel reconcile.
///
/// Step 5 (mic): mic disabled → MicBackend::remove() → source_exists() → 1 ls Node
/// returning the "no mic" output (source absent, remove returns immediately).
pub(super) fn queue_reconcile_present(runner: MockRunner) -> MockRunner {
    let ls = ls_all_present();
    let ls_no_mic = ls_all_present(); // "present" for channels but no mic node
    let mut r = runner;
    // detect_headset_sink: pw-metadata 0 + pw-dump (no SteelSeries → detect returns None)
    r = r.with_output(0, "", ""); // pw-metadata 0 (empty → default_sink = None)
    r = r.with_output(0, "[]", ""); // pw-dump [] → no sinks → detect = None
    // Phase 1: 4 ls (all present, including aux seeded by Engine::new)
    for _ in 0..4 {
        r = r.with_output(0, &ls, "");
    }
    // Phase 2 + 2b interleaved: per channel (4 channels), EQ apply then volume/mute apply
    for _ in 0..4 {
        // Phase 2: EQ apply (1 ls + 10 band sets)
        r = r.with_output(0, &ls, "");
        for _ in 0..10 {
            r = r.with_output(0, "", "");
        }
        // Phase 2b: volume/mute apply (1 ls + 1 Props set)
        r = r.with_output(0, &ls, ""); // find_node_id
        r = r.with_output(0, "", ""); // Props set
    }
    // Phase 5: mic disabled → remove() → source_exists() → 1 ls (no mic node found)
    r = r.with_output(0, &ls_no_mic, "");
    // Phase 6: surround disabled → remove() → source_exists() → 1 ls (surround absent)
    r = r.with_output(0, &ls, "");
    r
}

// ─────────────────────────────────────────────
// TDD: new features — get-state full EQ, set_channel_output, new_profile
// ─────────────────────────────────────────────

/// Config with EQ bands set on the game channel.
pub(super) fn make_config_with_eq_bands() -> Config {
    Config {
        version: arctis_config::CURRENT_VERSION,
        active_profile: "default".into(),
        profiles: vec![Profile {
            name: "default".into(),
            channels: vec![
                ChannelConfig {
                    id: "game".into(),
                    node_name: "Arctis_Game".into(),
                    description: "Game".into(),
                    output_device: None,
                    eq: vec![
                        arctis_config::EqBandConfig {
                            kind: "peaking".into(),
                            freq_hz: 100.0,
                            q: 1.0,
                            gain_db: 3.0,
                        },
                        arctis_config::EqBandConfig {
                            kind: "highshelf".into(),
                            freq_hz: 8000.0,
                            q: 0.7,
                            gain_db: -2.0,
                        },
                    ],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
                ChannelConfig {
                    id: "chat".into(),
                    node_name: "Arctis_Chat".into(),
                    description: "Chat".into(),
                    output_device: Some("alsa_output.headphones".into()),
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
                ChannelConfig {
                    id: "media".into(),
                    node_name: "Arctis_Media".into(),
                    description: "Media".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
            ],
            routes: vec![],
            mic: MicChainConfig::default(),
            surround: arctis_config::SurroundConfig::default(),
            master_volume_db: 0.0,
            master_volume_pct: 100,
            master_mute: false,
            chatmix_position: 4,
            default_sink_channel: None,
        }],
        eq_presets: vec![],
        dial_controls_balance: true,
        knob_controls_master: true,
    }
}

/// Build a MicChainConfig with master switch enabled but all stages off (clean passthrough).
pub(super) fn mic_enabled_passthrough() -> arctis_config::MicChainConfig {
    arctis_config::MicChainConfig {
        enabled: true,
        hw_mic: Some("alsa_input.hw_mic".to_string()),
        ..Default::default()
    }
}

/// Build a 3-channel config with mic enabled passthrough (no stages).
pub(super) fn make_config_mic_enabled() -> Config {
    Config {
        version: arctis_config::CURRENT_VERSION,
        active_profile: "default".into(),
        profiles: vec![Profile {
            name: "default".into(),
            channels: vec![
                ChannelConfig {
                    id: "game".into(),
                    node_name: "Arctis_Game".into(),
                    description: "Game".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
                ChannelConfig {
                    id: "chat".into(),
                    node_name: "Arctis_Chat".into(),
                    description: "Chat".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
                ChannelConfig {
                    id: "media".into(),
                    node_name: "Arctis_Media".into(),
                    description: "Media".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
            ],
            routes: vec![],
            mic: mic_enabled_passthrough(),
            surround: arctis_config::SurroundConfig::default(),
            master_volume_db: 0.0,
            master_volume_pct: 100,
            master_mute: false,
            chatmix_position: 4,
            default_sink_channel: None,
        }],
        eq_presets: vec![],
        dial_controls_balance: true,
        knob_controls_master: true,
    }
}

// ─────────────────────────────────────────────
// F1.3 TDD: reconcile step6 (surround)
// ─────────────────────────────────────────────

/// Build a config with surround enabled, pointing to a real temp HRIR file.
pub(super) fn make_config_surround_enabled(hrir_stem: &str) -> Config {
    Config {
        version: arctis_config::CURRENT_VERSION,
        active_profile: "default".into(),
        profiles: vec![Profile {
            name: "default".into(),
            channels: vec![
                ChannelConfig {
                    id: "game".into(),
                    node_name: "Arctis_Game".into(),
                    description: "Game".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
                ChannelConfig {
                    id: "chat".into(),
                    node_name: "Arctis_Chat".into(),
                    description: "Chat".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
                ChannelConfig {
                    id: "media".into(),
                    node_name: "Arctis_Media".into(),
                    description: "Media".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
            ],
            routes: vec![],
            mic: MicChainConfig::default(),
            surround: arctis_config::SurroundConfig {
                enabled: true,
                hrir: Some(hrir_stem.into()),
                channels: vec!["game".into(), "media".into()],
                hw_sink: None,
                ..Default::default()
            },
            master_volume_db: 0.0,
            master_volume_pct: 100,
            master_mute: false,
            chatmix_position: 4,
            default_sink_channel: None,
        }],
        eq_presets: vec![],
        dial_controls_balance: true,
        knob_controls_master: true,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Task 3 (engine output): list_output_devices
// ─────────────────────────────────────────────────────────────────────────

pub(super) const PW_METADATA_SINK: &str = concat!(
    "update: id:0 key:'default.audio.sink' ",
    r#"value:'{"name":"alsa_output.pci-0000_00_1f.3.analog-stereo"}' type:Spa:String"#
);
