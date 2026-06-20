use arctis_audio::{CommandRunner, RealRunner};
use arctis_config::{Config, EqBandConfig};
use arctis_engine::{Engine, EngineError, EngineState};

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
    fn ok_with_state(state: EngineState) -> Self {
        Self {
            ok: true,
            state: Some(state),
            error: None,
        }
    }

    fn err(msg: String) -> Self {
        Self {
            ok: false,
            state: None,
            error: Some(msg),
        }
    }
}

pub fn handle_request<R: CommandRunner>(engine: &mut Engine<R>, req: Request) -> Response {
    match req {
        Request::GetState => Response::ok_with_state(engine.state()),
        Request::SwitchProfile { name } => match engine.switch_profile(&name) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::SetEqBand {
            channel,
            band,
            kind,
            freq_hz,
            q,
            gain_db,
        } => {
            let cfg = EqBandConfig {
                kind,
                freq_hz,
                q,
                gain_db,
            };
            match engine.set_eq_band(&channel, band, cfg) {
                Ok(()) => Response::ok_with_state(engine.state()),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Request::Route {
            app_binary,
            target_sink,
        } => match engine.set_route(&app_binary, &target_sink) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::Reload => match engine.reconcile() {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::Shutdown => Response::ok_with_state(engine.state()),
    }
}

pub fn run_daemon() -> Result<(), EngineError> {
    use std::io::{BufRead, BufReader, Write};

    let path = socket_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    let listener = std::os::unix::net::UnixListener::bind(&path)
        .map_err(|e| EngineError::Ipc(e.to_string()))?;

    let cfg = arctis_config::store::load()
        .unwrap_or_else(|_| Config::default_config());
    let mut engine = Engine::new(RealRunner, cfg);
    if let Err(e) = engine.reconcile() {
        eprintln!("warning: reconcile on start failed: {e}");
    }

    let mut shutdown = false;
    for stream in listener.incoming() {
        if shutdown {
            break;
        }
        let stream = stream.map_err(|e| EngineError::Ipc(e.to_string()))?;
        let reader_stream = stream
            .try_clone()
            .map_err(|e| EngineError::Ipc(e.to_string()))?;
        let mut reader = BufReader::new(reader_stream);
        let mut writer = stream;
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader
                .read_line(&mut line)
                .map_err(|e| EngineError::Ipc(e.to_string()))?;
            if n == 0 {
                break; // EOF — client closed the connection
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let req: Request = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    let resp = Response::err(format!("parse error: {e}"));
                    let resp_str = serde_json::to_string(&resp)
                        .unwrap_or_else(|_| r#"{"ok":false,"error":"serialize error"}"#.to_string());
                    let _ = writeln!(writer, "{resp_str}");
                    continue;
                }
            };
            let is_shutdown = matches!(req, Request::Shutdown);
            let resp = handle_request(&mut engine, req);
            let resp_str = serde_json::to_string(&resp)
                .unwrap_or_else(|_| r#"{"ok":false,"error":"serialize error"}"#.to_string());
            let _ = writeln!(writer, "{resp_str}");
            if is_shutdown {
                shutdown = true;
                break;
            }
        }
    }

    let _ = std::fs::remove_file(&path);
    Ok(())
}

pub fn send_request(req: &Request) -> Result<Response, EngineError> {
    use std::io::{BufRead, BufReader, Write};

    let path = socket_path();
    let stream = std::os::unix::net::UnixStream::connect(&path)
        .map_err(|e| EngineError::Ipc(format!("connect to {}: {}", path.display(), e)))?;
    let req_str =
        serde_json::to_string(req).map_err(|e| EngineError::Ipc(e.to_string()))?;
    let mut writer = stream
        .try_clone()
        .map_err(|e| EngineError::Ipc(e.to_string()))?;
    writeln!(writer, "{req_str}").map_err(|e| EngineError::Ipc(e.to_string()))?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| EngineError::Ipc(e.to_string()))?;
    serde_json::from_str(line.trim()).map_err(|e| EngineError::Ipc(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arctis_audio::MockRunner;
    use arctis_config::{ChannelConfig, Profile};

    fn two_profile_config() -> Config {
        let channels = vec![
            ChannelConfig {
                id: "game".into(),
                node_name: "Arctis_Game".into(),
                description: "Game".into(),
                output_device: None,
                eq: vec![],
            },
            ChannelConfig {
                id: "chat".into(),
                node_name: "Arctis_Chat".into(),
                description: "Chat".into(),
                output_device: None,
                eq: vec![],
            },
            ChannelConfig {
                id: "media".into(),
                node_name: "Arctis_Media".into(),
                description: "Media".into(),
                output_device: None,
                eq: vec![],
            },
        ];
        Config {
            version: arctis_config::CURRENT_VERSION,
            active_profile: "default".into(),
            profiles: vec![
                Profile {
                    name: "default".into(),
                    channels: channels.clone(),
                    routes: vec![],
                },
                Profile {
                    name: "gaming".into(),
                    channels,
                    routes: vec![],
                },
            ],
        }
    }

    fn ls_all_present() -> String {
        [
            "id 10\n    node.name = \"Arctis_Game\"\n",
            "id 11\n    node.name = \"Arctis_Chat\"\n",
            "id 12\n    node.name = \"Arctis_Media\"\n",
        ]
        .concat()
    }

    fn queue_reconcile_present(runner: MockRunner) -> MockRunner {
        let ls = ls_all_present();
        let mut r = runner;
        // Phase 1: 3 ls (all present)
        for _ in 0..3 {
            r = r.with_output(0, &ls, "");
        }
        // Phase 2: 3 channels × (1 ls + 10 band sets)
        for _ in 0..3 {
            r = r.with_output(0, &ls, "");
            for _ in 0..10 {
                r = r.with_output(0, "", "");
            }
        }
        r
    }

    #[test]
    fn parse_get_state() {
        let req: Request = serde_json::from_str(r#"{"cmd":"get-state"}"#).unwrap();
        assert_eq!(req, Request::GetState);
    }

    #[test]
    fn parse_switch() {
        let req: Request =
            serde_json::from_str(r#"{"cmd":"switch-profile","name":"gaming"}"#).unwrap();
        assert_eq!(req, Request::SwitchProfile { name: "gaming".into() });
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
    fn handle_switch_returns_state() {
        let tmp = std::env::temp_dir()
            .join(format!("asm7_sw_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let runner = queue_reconcile_present(MockRunner::new());
        let cfg = two_profile_config();
        let mut engine = Engine::new(runner, cfg);
        let resp = handle_request(&mut engine, Request::SwitchProfile { name: "gaming".into() });
        assert!(resp.ok, "expected ok:true");
        assert!(resp.state.is_some());
        assert_eq!(resp.state.unwrap().active_profile, "gaming");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_unknown_profile_errors() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::SwitchProfile { name: "nonexistent".into() },
        );
        assert!(!resp.ok);
        assert!(resp.error.is_some());
    }
}
