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

/// Serve a single accepted connection.
///
/// Returns `Ok(true)` when the client sends the `shutdown` command,
/// `Ok(false)` on normal EOF, or `Err(_)` on an I/O error.  The
/// caller is responsible for logging `Err` and continuing to the next
/// `accept()` rather than letting the error propagate out of the daemon.
fn serve_connection<R, Re, W>(
    reader: &mut std::io::BufReader<Re>,
    writer: &mut W,
    engine: &mut Engine<R>,
) -> std::io::Result<bool>
where
    R: CommandRunner,
    Re: std::io::Read,
    W: std::io::Write,
{
    use std::io::BufRead;

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(false); // EOF — client closed connection
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
                writeln!(writer, "{resp_str}")?;
                continue;
            }
        };
        let is_shutdown = matches!(req, Request::Shutdown);
        let resp = handle_request(engine, req);
        let resp_str = serde_json::to_string(&resp)
            .unwrap_or_else(|_| r#"{"ok":false,"error":"serialize error"}"#.to_string());
        writeln!(writer, "{resp_str}")?;
        if is_shutdown {
            return Ok(true);
        }
    }
}

pub fn run_daemon() -> Result<(), EngineError> {
    use std::io::BufReader;

    let path = socket_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    let listener = std::os::unix::net::UnixListener::bind(&path)
        .map_err(|e| EngineError::Ipc(e.to_string()))?;

    let cfg = arctis_config::store::load().unwrap_or_else(|_| Config::default_config());
    let mut engine = Engine::new(RealRunner, cfg);
    if let Err(e) = engine.reconcile() {
        eprintln!("warning: reconcile on start failed: {e}");
    }

    let mut shutdown = false;
    for stream in listener.incoming() {
        if shutdown {
            break;
        }
        // Transient accept error — log and continue rather than killing the daemon.
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("daemon: accept error (continuing): {e}");
                continue;
            }
        };
        let writer_stream = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("daemon: try_clone error (continuing): {e}");
                continue;
            }
        };
        let mut reader = BufReader::new(stream);
        let mut writer = writer_stream;
        match serve_connection(&mut reader, &mut writer, &mut engine) {
            Ok(true) => {
                shutdown = true;
            }
            Ok(false) => {}
            Err(e) => {
                // Per-connection I/O error (ECONNRESET, EPIPE, …): log and continue.
                eprintln!("daemon: connection error (continuing): {e}");
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
    let req_str = serde_json::to_string(req).map_err(|e| EngineError::Ipc(e.to_string()))?;
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
    fn handle_switch_returns_state() {
        let tmp = std::env::temp_dir().join(format!("asm7_sw_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let runner = queue_reconcile_present(MockRunner::new());
        let cfg = two_profile_config();
        let mut engine = Engine::new(runner, cfg);
        let resp = handle_request(
            &mut engine,
            Request::SwitchProfile {
                name: "gaming".into(),
            },
        );
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
            Request::SwitchProfile {
                name: "nonexistent".into(),
            },
        );
        assert!(!resp.ok);
        assert!(resp.error.is_some());
    }

    // ── serve_connection unit tests (in-memory reader/writer) ────────────────

    fn make_engine() -> Engine<MockRunner> {
        Engine::new(MockRunner::new(), two_profile_config())
    }

    #[test]
    fn serve_connection_get_state_returns_ok() {
        let input = b"{\"cmd\":\"get-state\"}\n";
        let mut reader = std::io::BufReader::new(std::io::Cursor::new(input.as_ref()));
        let mut output = Vec::<u8>::new();
        let mut engine = make_engine();

        let result = serve_connection(&mut reader, &mut output, &mut engine);
        // EOF after one request → Ok(false)
        assert!(matches!(result, Ok(false)));
        let response: Response = serde_json::from_slice(output.trim_ascii()).unwrap();
        assert!(response.ok);
        assert!(response.state.is_some());
    }

    #[test]
    fn serve_connection_parse_error_returns_error_response_and_continues() {
        // Two lines: a bad JSON line followed by a valid get-state.
        let input = b"not-json\n{\"cmd\":\"get-state\"}\n";
        let mut reader = std::io::BufReader::new(std::io::Cursor::new(input.as_ref()));
        let mut output = Vec::<u8>::new();
        let mut engine = make_engine();

        let result = serve_connection(&mut reader, &mut output, &mut engine);
        assert!(matches!(result, Ok(false)));
        // Two newline-delimited JSON responses.
        let lines: Vec<&[u8]> = output
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(lines.len(), 2, "expected two response lines");
        let err_resp: Response = serde_json::from_slice(lines[0]).unwrap();
        assert!(!err_resp.ok);
        assert!(err_resp
            .error
            .as_deref()
            .unwrap_or("")
            .contains("parse error"));
        let ok_resp: Response = serde_json::from_slice(lines[1]).unwrap();
        assert!(ok_resp.ok);
    }

    #[test]
    fn serve_connection_empty_input_returns_ok_false() {
        // Simulates a client that connects and immediately closes (ECONNRESET / EOF).
        let input: &[u8] = b"";
        let mut reader = std::io::BufReader::new(std::io::Cursor::new(input));
        let mut output = Vec::<u8>::new();
        let mut engine = make_engine();

        let result = serve_connection(&mut reader, &mut output, &mut engine);
        assert!(matches!(result, Ok(false)));
        assert!(output.is_empty());
    }

    #[test]
    fn serve_connection_shutdown_returns_ok_true() {
        let input = b"{\"cmd\":\"shutdown\"}\n";
        let mut reader = std::io::BufReader::new(std::io::Cursor::new(input.as_ref()));
        let mut output = Vec::<u8>::new();
        let mut engine = make_engine();

        let result = serve_connection(&mut reader, &mut output, &mut engine);
        assert!(
            matches!(result, Ok(true)),
            "shutdown should return Ok(true)"
        );
        let resp: Response = serde_json::from_slice(output.trim_ascii()).unwrap();
        assert!(resp.ok);
    }

    #[test]
    fn serve_connection_io_error_propagates_as_err() {
        // A reader that always returns an I/O error after yielding one byte.
        struct ErrorReader;
        impl std::io::Read for ErrorReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "ECONNRESET",
                ))
            }
        }
        let mut reader = std::io::BufReader::new(ErrorReader);
        let mut output = Vec::<u8>::new();
        let mut engine = make_engine();

        let result = serve_connection(&mut reader, &mut output, &mut engine);
        assert!(
            result.is_err(),
            "I/O error must propagate out of serve_connection"
        );
    }
}
