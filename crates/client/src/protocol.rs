use arctis_engine::EngineState;

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
    Reload,
    Shutdown,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<EngineState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok_with_state(state: EngineState) -> Self {
        Self {
            ok: true,
            state: Some(state),
            error: None,
        }
    }

    pub fn err(msg: String) -> Self {
        Self {
            ok: false,
            state: None,
            error: Some(msg),
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
}
