use arctis_engine::EngineState;

/// Serialisable legacy-stack detection report (mirrors `coexist::LegacyReport`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct CoexistReport {
    pub legacy_loopbacks: Vec<String>,
    pub hrir_switch_present: bool,
    pub rpm_daemon_running: bool,
    /// True when any legacy component was detected.
    pub any_detected: bool,
}

/// One action result from a teardown run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoexistActionResult {
    pub description: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Result of a `CoexistDisable` call.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoexistDisableResult {
    pub dry_run: bool,
    pub actions_attempted: usize,
    pub successes: usize,
    pub failures: Vec<CoexistActionResult>,
    /// True when all actions succeeded (or it was a dry-run).
    pub all_ok: bool,
    /// Human note about the RPM package (owner must `sudo dnf remove` manually).
    pub owner_note: String,
}

/// Path to the Unix domain socket used for IPC.
pub fn socket_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    base.join("arctis-sound-manager.sock")
}

#[derive(Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Request {
    GetState,
    SwitchProfile {
        name: String,
    },
    SetEqBand {
        channel: String,
        band: usize,
        kind: String,
        freq_hz: f32,
        q: f32,
        gain_db: f32,
    },
    Route {
        app_binary: String,
        target_sink: String,
    },
    /// Set the output device for a single channel. `device: None` resets to default.
    SetChannelOutput {
        channel: String,
        device: Option<String>,
    },
    /// Create a new profile by cloning the currently active one.
    ProfileNew {
        name: String,
    },
    /// Set a single device hardware control by name (sidetone|mic_led|anc|inactive_time|...).
    /// Writes are gated by the enabled_writes allowlist (empty until Task 7 owner-validation).
    DeviceSet {
        control: String,
        value: i64,
    },
    Reload,
    Shutdown,
    /// Return the current mic DSP chain snapshot (EngineState.mic).
    MicStatus,
    /// Enable or disable a named mic DSP stage (gain|highpass|rnnoise|compressor|gate|eq).
    MicStage {
        stage: String,
        enabled: bool,
    },
    /// Set a named mic DSP parameter (gain_db|highpass_freq|vad_threshold|…).
    MicSet {
        param: String,
        value: f32,
    },
    /// Set one band of the mic EQ (live, no restart).
    MicEqBand {
        band: usize,
        kind: String,
        freq_hz: f32,
        q: f32,
        gain_db: f32,
    },
    /// Set (or clear) the hardware mic capture source.
    MicHwMic {
        device: Option<String>,
    },
    /// Enable or disable the whole mic chain (master switch).
    MicEnable {
        enabled: bool,
    },
    /// Select the noise-suppression backend (deep_filter|rnnoise).
    MicSuppressionBackend {
        backend: String,
    },
    /// Enable or disable virtual surround (master switch).
    SurroundEnable {
        enabled: bool,
    },
    /// Set the active HRIR profile stem (bare filename without .wav).
    SurroundSetHrir {
        name: String,
    },
    /// Set which channels are routed through surround (e.g. ["game","media"]).
    SurroundSetChannels {
        channels: Vec<String>,
    },
    /// Pin (or clear) the surround output to a specific hardware sink.
    SurroundSetHwSink {
        hw_sink: Option<String>,
    },
    /// Return the current surround snapshot (EngineState.surround).
    SurroundStatus,
    /// Set the software volume (dB) for a single channel. Range: -60..=+6.
    SetChannelVolume {
        channel: String,
        volume_db: f32,
    },
    /// Set the mute state for a single channel.
    SetChannelMute {
        channel: String,
        muted: bool,
    },
    /// Rename an existing profile.
    ProfileRename {
        old: String,
        new: String,
    },
    /// Delete a profile. The active profile and the last profile cannot be deleted.
    ProfileDelete {
        name: String,
    },
    /// Export a profile as a standalone TOML string. Returns Response with text payload.
    ProfileExport {
        name: String,
    },
    /// Import a profile from a TOML string. Resolves name collisions automatically.
    ProfileImport {
        toml: String,
    },
    /// Save the current EQ bands of a channel as a named preset.
    EqPresetSave {
        name: String,
        channel: String,
    },
    /// Apply a named EQ preset to a channel's EQ bands.
    EqPresetApply {
        preset: String,
        channel: String,
    },
    /// Delete a named EQ preset.
    EqPresetDelete {
        name: String,
    },
    /// Remove a routing rule for an app by binary name.
    /// Drops the rule from persistent config + best-effort live clear (moves the
    /// stream back to the default sink if it is currently running).
    RouteClear {
        app_binary: String,
    },
    /// Add a new channel to the active profile by id.
    /// The engine derives node_name and description from the id.
    ChannelAdd {
        id: String,
    },
    /// Remove a channel from the active profile by id.
    /// Errors if the channel does not exist or is the last remaining channel.
    ChannelRemove {
        id: String,
    },
    /// Detect the legacy arctis-sound-manager RPM stack (loopback nodes + services).
    /// Returns a CoexistReport serialised in `Response.coexist`.
    CoexistStatus,
    /// Disable the legacy arctis-sound-manager RPM stack: stop+disable user services
    /// and destroy live loopback nodes. Optionally dry-run (preview only).
    CoexistDisable {
        dry_run: bool,
    },
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<EngineState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Export payload (profile TOML string). Populated only for ProfileExport responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Coexistence status report. Populated only for CoexistStatus responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coexist_report: Option<CoexistReport>,
    /// Coexistence disable result. Populated only for CoexistDisable responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coexist_result: Option<CoexistDisableResult>,
}

impl Response {
    pub fn ok_with_state(state: EngineState) -> Self {
        Self {
            ok: true,
            state: Some(state),
            error: None,
            text: None,
            coexist_report: None,
            coexist_result: None,
        }
    }

    pub fn ok_with_text(text: String) -> Self {
        Self {
            ok: true,
            state: None,
            error: None,
            text: Some(text),
            coexist_report: None,
            coexist_result: None,
        }
    }

    pub fn err(msg: String) -> Self {
        Self {
            ok: false,
            state: None,
            error: Some(msg),
            text: None,
            coexist_report: None,
            coexist_result: None,
        }
    }

    pub fn ok_with_coexist_report(report: CoexistReport) -> Self {
        Self {
            ok: true,
            state: None,
            error: None,
            text: None,
            coexist_report: Some(report),
            coexist_result: None,
        }
    }

    pub fn ok_with_coexist_result(result: CoexistDisableResult) -> Self {
        Self {
            ok: true,
            state: None,
            error: None,
            text: None,
            coexist_report: None,
            coexist_result: Some(result),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_get_state() {
        let req: Request = serde_json::from_str(r#"{"cmd":"get-state"}"#).unwrap();
        assert_eq!(req, Request::GetState);
    }

    #[test]
    fn parse_switch() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"switch-profile","name":"gaming"}"#).unwrap();
        assert_eq!(
            req,
            Request::SwitchProfile {
                name: "gaming".into()
            }
        );
    }

    #[test]
    fn parse_set_eq_band() {
        let req: Request = serde_json::from_str(
            r#"{"cmd":"set-eq-band","channel":"game","band":2,"kind":"peaking","freq_hz":1000.0,"q":1.0,"gain_db":-3.0}"#,
        )
        .unwrap();
        assert_eq!(
            req,
            Request::SetEqBand {
                channel: "game".into(),
                band: 2,
                kind: "peaking".into(),
                freq_hz: 1000.0,
                q: 1.0,
                gain_db: -3.0,
            }
        );
    }

    #[test]
    fn parse_route() {
        let req: Request = serde_json::from_str(
            r#"{"cmd":"route","app_binary":"firefox","target_sink":"Arctis_Media"}"#,
        )
        .unwrap();
        assert_eq!(
            req,
            Request::Route {
                app_binary: "firefox".into(),
                target_sink: "Arctis_Media".into(),
            }
        );
    }

    #[test]
    fn parse_shutdown() {
        let req: Request = serde_json::from_str(r#"{"cmd":"shutdown"}"#).unwrap();
        assert_eq!(req, Request::Shutdown);
    }

    #[test]
    fn response_ok_with_state_round_trips() {
        // Verify a minimal ok:true / ok:false response serializes and deserializes.
        let resp = Response {
            ok: true,
            state: None,
            error: None,
            text: None,
            coexist_report: None,
            coexist_result: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        assert!(back.ok);
        assert!(back.state.is_none());
        assert!(back.error.is_none());
    }

    #[test]
    fn response_err_round_trips() {
        let resp = Response::err("something went wrong".into());
        let json = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        assert!(!back.ok);
        assert_eq!(back.error.as_deref(), Some("something went wrong"));
    }

    #[test]
    fn request_get_state_round_trips() {
        let req = Request::GetState;
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_switch_profile_round_trips() {
        let req = Request::SwitchProfile {
            name: "gaming".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_set_eq_band_round_trips() {
        let req = Request::SetEqBand {
            channel: "game".into(),
            band: 3,
            kind: "peaking".into(),
            freq_hz: 1000.0,
            q: 1.0,
            gain_db: -3.0,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_route_round_trips() {
        let req = Request::Route {
            app_binary: "firefox".into(),
            target_sink: "Arctis_Media".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_reload_round_trips() {
        let req = Request::Reload;
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_shutdown_round_trips() {
        let req = Request::Shutdown;
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    // ── New verb round-trip tests (TDD: these must fail until verbs are added) ──

    #[test]
    fn parse_set_channel_output_with_device() {
        let req: Request = serde_json::from_str(
            r#"{"cmd":"set-channel-output","channel":"game","device":"alsa_output.speakers"}"#,
        )
        .unwrap();
        assert_eq!(
            req,
            Request::SetChannelOutput {
                channel: "game".into(),
                device: Some("alsa_output.speakers".into()),
            }
        );
    }

    #[test]
    fn parse_set_channel_output_no_device() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"set-channel-output","channel":"chat","device":null}"#)
                .unwrap();
        assert_eq!(
            req,
            Request::SetChannelOutput {
                channel: "chat".into(),
                device: None,
            }
        );
    }

    #[test]
    fn request_set_channel_output_with_device_round_trips() {
        let req = Request::SetChannelOutput {
            channel: "game".into(),
            device: Some("alsa_output.speakers".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("set-channel-output"),
            "cmd tag must be kebab-case"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_set_channel_output_none_device_round_trips() {
        let req = Request::SetChannelOutput {
            channel: "media".into(),
            device: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn parse_profile_new() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"profile-new","name":"competitive"}"#).unwrap();
        assert_eq!(
            req,
            Request::ProfileNew {
                name: "competitive".into()
            }
        );
    }

    #[test]
    fn request_profile_new_round_trips() {
        let req = Request::ProfileNew {
            name: "competitive".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("profile-new"), "cmd tag must be kebab-case");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    // ── Task 6: DeviceSet verb ───────────────────────────────────────────────

    #[test]
    fn parse_device_set() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"device-set","control":"sidetone","value":2}"#).unwrap();
        assert_eq!(
            req,
            Request::DeviceSet {
                control: "sidetone".into(),
                value: 2,
            }
        );
    }

    #[test]
    fn request_device_set_round_trips() {
        let req = Request::DeviceSet {
            control: "sidetone".into(),
            value: 2,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("device-set"), "cmd tag must be kebab-case");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_device_set_negative_value_round_trips() {
        // Verify i64 sign is preserved (some controls may use signed values in future).
        let req = Request::DeviceSet {
            control: "mic_volume".into(),
            value: -1,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    // ── Task 5: mic verb wire-tag parse tests ────────────────────────────────

    #[test]
    fn parse_mic_status_wire_tag() {
        let req: Request = serde_json::from_str(r#"{"cmd":"mic-status"}"#).unwrap();
        assert_eq!(req, Request::MicStatus);
    }

    #[test]
    fn parse_mic_stage_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"mic-stage","stage":"rnnoise","enabled":true}"#)
                .unwrap();
        assert_eq!(
            req,
            Request::MicStage {
                stage: "rnnoise".into(),
                enabled: true,
            }
        );
    }

    #[test]
    fn parse_mic_set_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"mic-set","param":"vad_threshold","value":40.0}"#)
                .unwrap();
        assert_eq!(
            req,
            Request::MicSet {
                param: "vad_threshold".into(),
                value: 40.0,
            }
        );
    }

    #[test]
    fn parse_mic_eq_band_wire_tag() {
        let req: Request = serde_json::from_str(
            r#"{"cmd":"mic-eq-band","band":2,"kind":"peaking","freq_hz":1000.0,"q":1.0,"gain_db":-3.0}"#,
        )
        .unwrap();
        assert_eq!(
            req,
            Request::MicEqBand {
                band: 2,
                kind: "peaking".into(),
                freq_hz: 1000.0,
                q: 1.0,
                gain_db: -3.0,
            }
        );
    }

    #[test]
    fn parse_mic_hw_mic_with_device_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"mic-hw-mic","device":"alsa_input.usb-SteelSeries"}"#)
                .unwrap();
        assert_eq!(
            req,
            Request::MicHwMic {
                device: Some("alsa_input.usb-SteelSeries".into()),
            }
        );
    }

    #[test]
    fn parse_mic_hw_mic_none_wire_tag() {
        let req: Request = serde_json::from_str(r#"{"cmd":"mic-hw-mic","device":null}"#).unwrap();
        assert_eq!(req, Request::MicHwMic { device: None });
    }

    // ── Task 5: mic verb round-trip tests ────────────────────────────────────

    #[test]
    fn request_mic_status_round_trips() {
        let req = Request::MicStatus;
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("mic-status"), "cmd tag must be mic-status");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_mic_stage_round_trips() {
        let req = Request::MicStage {
            stage: "gain".into(),
            enabled: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("mic-stage"), "cmd tag must be mic-stage");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_mic_set_round_trips() {
        let req = Request::MicSet {
            param: "vad_threshold".into(),
            value: 40.0,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("mic-set"), "cmd tag must be mic-set");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_mic_eq_band_round_trips() {
        let req = Request::MicEqBand {
            band: 3,
            kind: "peaking".into(),
            freq_hz: 1000.0,
            q: 1.0,
            gain_db: -3.0,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("mic-eq-band"), "cmd tag must be mic-eq-band");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_mic_hw_mic_some_round_trips() {
        let req = Request::MicHwMic {
            device: Some("alsa_input.usb-SteelSeries".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("mic-hw-mic"), "cmd tag must be mic-hw-mic");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_mic_hw_mic_none_round_trips() {
        let req = Request::MicHwMic { device: None };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("mic-hw-mic"), "cmd tag must be mic-hw-mic");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    // ── Task 3: MicSuppressionBackend wire-tag + round-trip tests ────────────

    #[test]
    fn parse_mic_suppression_backend_deep_filter_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"mic-suppression-backend","backend":"deep_filter"}"#)
                .unwrap();
        assert_eq!(
            req,
            Request::MicSuppressionBackend {
                backend: "deep_filter".into(),
            }
        );
    }

    #[test]
    fn parse_mic_suppression_backend_rnnoise_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"mic-suppression-backend","backend":"rnnoise"}"#)
                .unwrap();
        assert_eq!(
            req,
            Request::MicSuppressionBackend {
                backend: "rnnoise".into(),
            }
        );
    }

    #[test]
    fn request_mic_suppression_backend_deep_filter_round_trips() {
        let req = Request::MicSuppressionBackend {
            backend: "deep_filter".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("mic-suppression-backend"),
            "cmd tag must be mic-suppression-backend, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_mic_suppression_backend_rnnoise_round_trips() {
        let req = Request::MicSuppressionBackend {
            backend: "rnnoise".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("mic-suppression-backend"),
            "cmd tag must be mic-suppression-backend, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    // ── F1.4: surround verb wire-tag parse tests ─────────────────────────────

    #[test]
    fn parse_surround_enable_true_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"surround-enable","enabled":true}"#).unwrap();
        assert_eq!(req, Request::SurroundEnable { enabled: true });
    }

    #[test]
    fn parse_surround_enable_false_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"surround-enable","enabled":false}"#).unwrap();
        assert_eq!(req, Request::SurroundEnable { enabled: false });
    }

    #[test]
    fn parse_surround_set_hrir_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"surround-set-hrir","name":"02-dh-dolby-headphone"}"#)
                .unwrap();
        assert_eq!(
            req,
            Request::SurroundSetHrir {
                name: "02-dh-dolby-headphone".into()
            }
        );
    }

    #[test]
    fn parse_surround_set_channels_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"surround-set-channels","channels":["game","media"]}"#)
                .unwrap();
        assert_eq!(
            req,
            Request::SurroundSetChannels {
                channels: vec!["game".into(), "media".into()]
            }
        );
    }

    #[test]
    fn parse_surround_set_hw_sink_some_wire_tag() {
        let req: Request = serde_json::from_str(
            r#"{"cmd":"surround-set-hw-sink","hw_sink":"alsa_output.usb-SteelSeries"}"#,
        )
        .unwrap();
        assert_eq!(
            req,
            Request::SurroundSetHwSink {
                hw_sink: Some("alsa_output.usb-SteelSeries".into())
            }
        );
    }

    #[test]
    fn parse_surround_set_hw_sink_none_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"surround-set-hw-sink","hw_sink":null}"#).unwrap();
        assert_eq!(req, Request::SurroundSetHwSink { hw_sink: None });
    }

    #[test]
    fn parse_surround_status_wire_tag() {
        let req: Request = serde_json::from_str(r#"{"cmd":"surround-status"}"#).unwrap();
        assert_eq!(req, Request::SurroundStatus);
    }

    // ── F1.4: surround verb round-trip tests ─────────────────────────────────

    #[test]
    fn request_surround_enable_true_round_trips() {
        let req = Request::SurroundEnable { enabled: true };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("surround-enable"),
            "cmd tag must be surround-enable, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_surround_enable_false_round_trips() {
        let req = Request::SurroundEnable { enabled: false };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("surround-enable"),
            "cmd tag must be surround-enable, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_surround_set_hrir_round_trips() {
        let req = Request::SurroundSetHrir {
            name: "02-dh-dolby-headphone".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("surround-set-hrir"),
            "cmd tag must be surround-set-hrir, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_surround_set_channels_round_trips() {
        let req = Request::SurroundSetChannels {
            channels: vec!["game".into(), "media".into()],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("surround-set-channels"),
            "cmd tag must be surround-set-channels, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_surround_set_hw_sink_some_round_trips() {
        let req = Request::SurroundSetHwSink {
            hw_sink: Some("alsa_output.usb-SteelSeries".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("surround-set-hw-sink"),
            "cmd tag must be surround-set-hw-sink, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_surround_set_hw_sink_none_round_trips() {
        let req = Request::SurroundSetHwSink { hw_sink: None };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("surround-set-hw-sink"),
            "cmd tag must be surround-set-hw-sink, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_surround_status_round_trips() {
        let req = Request::SurroundStatus;
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("surround-status"),
            "cmd tag must be surround-status, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    // ── Task 5b: MicEnable wire-tag parse test ───────────────────────────────

    #[test]
    fn parse_mic_enable_wire_tag() {
        let req: Request = serde_json::from_str(r#"{"cmd":"mic-enable","enabled":true}"#).unwrap();
        assert_eq!(req, Request::MicEnable { enabled: true });
    }

    #[test]
    fn parse_mic_enable_false_wire_tag() {
        let req: Request = serde_json::from_str(r#"{"cmd":"mic-enable","enabled":false}"#).unwrap();
        assert_eq!(req, Request::MicEnable { enabled: false });
    }

    #[test]
    fn request_mic_enable_true_round_trips() {
        let req = Request::MicEnable { enabled: true };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("mic-enable"), "cmd tag must be mic-enable");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_mic_enable_false_round_trips() {
        let req = Request::MicEnable { enabled: false };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("mic-enable"), "cmd tag must be mic-enable");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    // ── F3a: new verb wire-tag parse tests ──────────────────────────────────

    #[test]
    fn parse_profile_rename_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"profile-rename","old":"default","new":"gaming"}"#)
                .unwrap();
        assert_eq!(
            req,
            Request::ProfileRename {
                old: "default".into(),
                new: "gaming".into()
            }
        );
    }

    #[test]
    fn parse_profile_delete_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"profile-delete","name":"gaming"}"#).unwrap();
        assert_eq!(
            req,
            Request::ProfileDelete {
                name: "gaming".into()
            }
        );
    }

    #[test]
    fn parse_profile_export_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"profile-export","name":"gaming"}"#).unwrap();
        assert_eq!(
            req,
            Request::ProfileExport {
                name: "gaming".into()
            }
        );
    }

    #[test]
    fn parse_profile_import_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"profile-import","toml":"name = \"test\""}"#).unwrap();
        assert_eq!(
            req,
            Request::ProfileImport {
                toml: "name = \"test\"".into()
            }
        );
    }

    #[test]
    fn parse_eq_preset_save_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"eq-preset-save","name":"my-preset","channel":"game"}"#)
                .unwrap();
        assert_eq!(
            req,
            Request::EqPresetSave {
                name: "my-preset".into(),
                channel: "game".into()
            }
        );
    }

    #[test]
    fn parse_eq_preset_apply_wire_tag() {
        let req: Request = serde_json::from_str(
            r#"{"cmd":"eq-preset-apply","preset":"my-preset","channel":"game"}"#,
        )
        .unwrap();
        assert_eq!(
            req,
            Request::EqPresetApply {
                preset: "my-preset".into(),
                channel: "game".into()
            }
        );
    }

    #[test]
    fn parse_eq_preset_delete_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"eq-preset-delete","name":"my-preset"}"#).unwrap();
        assert_eq!(
            req,
            Request::EqPresetDelete {
                name: "my-preset".into()
            }
        );
    }

    // ── F3a: new verb round-trip tests ────────────────────────────────────────

    #[test]
    fn request_profile_rename_round_trips() {
        let req = Request::ProfileRename {
            old: "default".into(),
            new: "gaming".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("profile-rename"),
            "cmd tag must be profile-rename, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_profile_delete_round_trips() {
        let req = Request::ProfileDelete {
            name: "gaming".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("profile-delete"),
            "cmd tag must be profile-delete, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_profile_export_round_trips() {
        let req = Request::ProfileExport {
            name: "gaming".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("profile-export"),
            "cmd tag must be profile-export, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_profile_import_round_trips() {
        let req = Request::ProfileImport {
            toml: "name = \"test\"".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("profile-import"),
            "cmd tag must be profile-import, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_eq_preset_save_round_trips() {
        let req = Request::EqPresetSave {
            name: "gaming-boost".into(),
            channel: "game".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("eq-preset-save"),
            "cmd tag must be eq-preset-save, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_eq_preset_apply_round_trips() {
        let req = Request::EqPresetApply {
            preset: "gaming-boost".into(),
            channel: "game".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("eq-preset-apply"),
            "cmd tag must be eq-preset-apply, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_eq_preset_delete_round_trips() {
        let req = Request::EqPresetDelete {
            name: "gaming-boost".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("eq-preset-delete"),
            "cmd tag must be eq-preset-delete, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn response_ok_with_text_round_trips() {
        let resp = Response::ok_with_text("profile TOML here".into());
        let json = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        assert!(back.ok);
        assert_eq!(back.text.as_deref(), Some("profile TOML here"));
        assert!(back.state.is_none());
        assert!(back.error.is_none());
    }

    #[test]
    fn response_ok_with_text_has_no_state_field() {
        let resp = Response::ok_with_text("exported".into());
        let json = serde_json::to_string(&resp).unwrap();
        // `state` should not appear (skip_serializing_if = None)
        assert!(!json.contains("\"state\""), "state must be absent: {json}");
    }

    // ── F5a: RouteClear wire-tag + round-trip tests ──────────────────────────

    #[test]
    fn parse_route_clear_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"route-clear","app_binary":"firefox"}"#).unwrap();
        assert_eq!(
            req,
            Request::RouteClear {
                app_binary: "firefox".into()
            }
        );
    }

    #[test]
    fn request_route_clear_round_trips() {
        let req = Request::RouteClear {
            app_binary: "firefox".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("route-clear"),
            "cmd tag must be route-clear, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    // ── F4: ChannelAdd / ChannelRemove wire-tag parse tests ──────────────────

    #[test]
    fn parse_channel_add_wire_tag() {
        let req: Request = serde_json::from_str(r#"{"cmd":"channel-add","id":"aux"}"#).unwrap();
        assert_eq!(req, Request::ChannelAdd { id: "aux".into() });
    }

    #[test]
    fn parse_channel_remove_wire_tag() {
        let req: Request = serde_json::from_str(r#"{"cmd":"channel-remove","id":"aux"}"#).unwrap();
        assert_eq!(req, Request::ChannelRemove { id: "aux".into() });
    }

    // ── R2: CoexistStatus + CoexistDisable wire-tag + round-trip tests ──────────

    #[test]
    fn parse_coexist_status_wire_tag() {
        let req: Request = serde_json::from_str(r#"{"cmd":"coexist-status"}"#).unwrap();
        assert_eq!(req, Request::CoexistStatus);
    }

    #[test]
    fn parse_coexist_disable_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"coexist-disable","dry_run":true}"#).unwrap();
        assert_eq!(req, Request::CoexistDisable { dry_run: true });
    }

    #[test]
    fn request_coexist_status_round_trips() {
        let req = Request::CoexistStatus;
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("coexist-status"),
            "cmd tag must be coexist-status, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_coexist_disable_round_trips() {
        let req = Request::CoexistDisable { dry_run: false };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("coexist-disable"),
            "cmd tag must be coexist-disable, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_coexist_disable_dry_run_true_round_trips() {
        let req = Request::CoexistDisable { dry_run: true };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_channel_add_round_trips() {
        let req = Request::ChannelAdd { id: "aux".into() };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("channel-add"),
            "cmd tag must be channel-add, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_channel_remove_round_trips() {
        let req = Request::ChannelRemove { id: "aux".into() };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("channel-remove"),
            "cmd tag must be channel-remove, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    // ── F2.1: SetChannelVolume / SetChannelMute wire-tag + round-trip tests ──

    #[test]
    fn parse_set_channel_volume_wire_tag() {
        let req: Request = serde_json::from_str(
            r#"{"cmd":"set-channel-volume","channel":"game","volume_db":-6.0}"#,
        )
        .unwrap();
        assert_eq!(
            req,
            Request::SetChannelVolume {
                channel: "game".into(),
                volume_db: -6.0,
            }
        );
    }

    #[test]
    fn parse_set_channel_mute_wire_tag() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"set-channel-mute","channel":"chat","muted":true}"#)
                .unwrap();
        assert_eq!(
            req,
            Request::SetChannelMute {
                channel: "chat".into(),
                muted: true,
            }
        );
    }

    #[test]
    fn request_set_channel_volume_round_trips() {
        let req = Request::SetChannelVolume {
            channel: "game".into(),
            volume_db: -6.0,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("set-channel-volume"),
            "cmd tag must be set-channel-volume, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    #[test]
    fn request_set_channel_mute_round_trips() {
        let req = Request::SetChannelMute {
            channel: "chat".into(),
            muted: false,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.contains("set-channel-mute"),
            "cmd tag must be set-channel-mute, got: {json}"
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }
}
