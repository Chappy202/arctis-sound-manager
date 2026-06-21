use arctis_audio::{CommandRunner, RealRunner, StageKind};
use arctis_config::{Config, EqBandConfig};
use arctis_engine::{Engine, EngineError, MicParam, SuppressionBackend};

// Re-export protocol types and client from arctis-client so that `main.rs`
// can continue to reference `daemon::Request`, `daemon::send_request`, etc.
pub use arctis_client::{send_request, socket_path, Request, Response};

/// Map a canonical stage string (from the protocol wire or CLI) to a `StageKind`.
/// Returns `EngineError::BadRequest` for unknown strings.
fn parse_mic_stage(s: &str) -> Result<StageKind, EngineError> {
    match s {
        "gain" => Ok(StageKind::Gain),
        "highpass" => Ok(StageKind::Highpass),
        // "rnnoise" kept as backward-compat alias; canonical name is "suppression"
        "suppression" | "rnnoise" => Ok(StageKind::Suppression),
        "compressor" => Ok(StageKind::Compressor),
        "gate" => Ok(StageKind::Gate),
        "eq" => Ok(StageKind::MicEq),
        other => Err(EngineError::BadRequest(format!(
            "unknown mic stage '{other}' (use: gain|highpass|suppression|compressor|gate|eq)"
        ))),
    }
}

/// Map a canonical param string (from the protocol wire or CLI) to a `MicParam`.
/// Returns `EngineError::BadRequest` for unknown strings.
fn parse_mic_param(s: &str) -> Result<MicParam, EngineError> {
    match s {
        "gain_db" => Ok(MicParam::GainDb),
        "highpass_freq" => Ok(MicParam::HighpassFreq),
        "attenuation_limit_db" => Ok(MicParam::AttenuationLimitDb),
        "vad_threshold" => Ok(MicParam::VadThreshold),
        "vad_grace_ms" => Ok(MicParam::VadGraceMs),
        "vad_retro_grace_ms" => Ok(MicParam::VadRetroGraceMs),
        "gate_threshold" => Ok(MicParam::GateThreshold),
        "comp_threshold_db" => Ok(MicParam::CompThresholdDb),
        "comp_ratio" => Ok(MicParam::CompRatio),
        "comp_makeup_db" => Ok(MicParam::CompMakeupDb),
        other => Err(EngineError::BadRequest(format!(
            "unknown mic param '{other}' (use: gain_db|highpass_freq|attenuation_limit_db|vad_threshold|vad_grace_ms|vad_retro_grace_ms|gate_threshold|comp_threshold_db|comp_ratio|comp_makeup_db)"
        ))),
    }
}

/// Map a canonical backend string to a `SuppressionBackend`.
/// Returns `EngineError::BadRequest` for unknown strings.
fn parse_suppression_backend(s: &str) -> Result<SuppressionBackend, EngineError> {
    match s {
        "deep_filter" => Ok(SuppressionBackend::DeepFilter),
        "rnnoise" => Ok(SuppressionBackend::Rnnoise),
        other => Err(EngineError::BadRequest(format!(
            "unknown suppression backend '{other}' (use: deep_filter|rnnoise)"
        ))),
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
        Request::SetChannelOutput { channel, device } => {
            match engine.set_channel_output(&channel, device) {
                Ok(()) => Response::ok_with_state(engine.state()),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Request::ProfileNew { name } => match engine.new_profile(&name) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::DeviceSet { control, value } => match engine.device_set(&control, value) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::Reload => match engine.reconcile() {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::Shutdown => Response::ok_with_state(engine.state()),
        Request::MicStatus => Response::ok_with_state(engine.state()),
        Request::MicStage { stage, enabled } => match parse_mic_stage(&stage) {
            Ok(kind) => match engine.mic_set_stage_enabled(kind, enabled) {
                Ok(()) => Response::ok_with_state(engine.state()),
                Err(e) => Response::err(e.to_string()),
            },
            Err(e) => Response::err(e.to_string()),
        },
        Request::MicSet { param, value } => match parse_mic_param(&param) {
            Ok(p) => match engine.mic_set_param(p, value) {
                Ok(()) => Response::ok_with_state(engine.state()),
                Err(e) => Response::err(e.to_string()),
            },
            Err(e) => Response::err(e.to_string()),
        },
        Request::MicEqBand {
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
            match engine.mic_set_eq_band(band, cfg) {
                Ok(()) => Response::ok_with_state(engine.state()),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Request::MicHwMic { device } => match engine.mic_set_hw_mic(device) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::MicEnable { enabled } => match engine.mic_set_enabled(enabled) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::MicSuppressionBackend { backend } => match parse_suppression_backend(&backend) {
            Ok(b) => match engine.mic_set_suppression_backend(b) {
                Ok(()) => Response::ok_with_state(engine.state()),
                Err(e) => Response::err(e.to_string()),
            },
            Err(e) => Response::err(e.to_string()),
        },
        Request::SurroundStatus => Response::ok_with_state(engine.state()),
        Request::SurroundEnable { enabled } => match engine.surround_set_enabled(enabled) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::SurroundSetHrir { name } => match engine.surround_set_hrir(name) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::SurroundSetChannels { channels } => match engine.surround_set_channels(channels) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::SurroundSetHwSink { hw_sink } => match engine.surround_set_hw_sink(hw_sink) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::SetChannelVolume { channel, volume_db } => {
            match engine.set_channel_volume(&channel, volume_db) {
                Ok(()) => Response::ok_with_state(engine.state()),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Request::SetChannelMute { channel, muted } => {
            match engine.set_channel_mute(&channel, muted) {
                Ok(()) => Response::ok_with_state(engine.state()),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Request::ProfileRename { old, new } => match engine.rename_profile(&old, &new) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::ProfileDelete { name } => match engine.delete_profile(&name) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::ProfileExport { name } => match engine.export_profile(&name) {
            Ok(toml) => Response::ok_with_text(toml),
            Err(e) => Response::err(e.to_string()),
        },
        Request::ProfileImport { toml } => match engine.import_profile(&toml) {
            Ok(_name) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::EqPresetSave { name, channel } => match engine.save_eq_preset(&name, &channel) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::EqPresetApply { preset, channel } => {
            match engine.apply_eq_preset(&preset, &channel) {
                Ok(()) => Response::ok_with_state(engine.state()),
                Err(e) => Response::err(e.to_string()),
            }
        }
        Request::EqPresetDelete { name } => match engine.delete_eq_preset(&name) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
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

/// Core accept loop. Accepts a pre-built engine so tests can inject a
/// MockRunner-backed engine without touching real PipeWire.
///
/// On shutdown the loop breaks IMMEDIATELY after the connection that sent
/// the shutdown request (no blocking accept()). Then `engine.shutdown()` is
/// called for deterministic child teardown, and the socket file is removed.
pub fn run_daemon_with_engine<R: arctis_audio::CommandRunner>(
    engine: &mut Engine<R>,
    path: &std::path::Path,
) -> Result<(), EngineError> {
    use std::io::BufReader;

    let listener = std::os::unix::net::UnixListener::bind(path)
        .map_err(|e| EngineError::Ipc(e.to_string()))?;

    for stream in listener.incoming() {
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
        let shutdown = match serve_connection(&mut reader, &mut writer, engine) {
            Ok(true) => true,
            Ok(false) => false,
            Err(e) => {
                // Per-connection I/O error (ECONNRESET, EPIPE, …): log and continue.
                eprintln!("daemon: connection error (continuing): {e}");
                false
            }
        };
        // Break IMMEDIATELY after the connection that requested shutdown — do NOT
        // loop back to accept() which would block indefinitely.
        if shutdown {
            break;
        }
    }

    // Deterministic teardown: kill all tracked children before returning.
    // Do not rely solely on Drop timing.
    if let Err(e) = engine.shutdown() {
        eprintln!("daemon: shutdown warning: {e}");
    }
    let _ = std::fs::remove_file(path);
    Ok(())
}

/// Real device opener: discovers the Nova Pro on the hidraw interface and opens it.
struct HidOpener;

impl arctis_engine::DeviceOpener for HidOpener {
    type T = arctis_device::HidrawTransport;
    fn open(
        &self,
    ) -> Result<
        Option<(arctis_device::DeviceController<Self::T>, Vec<String>)>,
        arctis_device::DeviceError,
    > {
        let registry = arctis_device::Registry::builtin()
            .map_err(|e| arctis_device::DeviceError::Unsupported(e.to_string()))?;
        match arctis_device::discover(&registry)? {
            Some((id, iface)) => {
                let desc = registry
                    .find(id)
                    .ok_or(arctis_device::DeviceError::NotConnected)?
                    .clone();
                let transport = arctis_device::HidrawTransport::open(id, iface)?;
                // SAFETY GATE: enabled_writes starts EMPTY. OWNER-RUN tasks (Task 7)
                // add one name at a time AFTER real-HW validation. Do NOT add a name
                // here unless its OWNER-RUN gate in this plan is signed off.
                let enabled: Vec<String> = vec![/* filled by Task 7 gates */];
                let controller = arctis_device::DeviceController::new(transport, desc)
                    .with_enabled_writes(&enabled.iter().map(|s| s.as_str()).collect::<Vec<_>>());
                Ok(Some((controller, enabled)))
            }
            None => Ok(None),
        }
    }
}

pub fn run_daemon() -> Result<(), EngineError> {
    let path = socket_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    let cfg = arctis_config::store::load().unwrap_or_else(|_| Config::default_config());
    let mut engine = Engine::new(RealRunner, cfg);
    if let Err(e) = engine.reconcile() {
        eprintln!("warning: reconcile on start failed: {e}");
    }

    // Spawn the DeviceWorker read-loop on a dedicated thread.
    // Create the write-command channel so writes are serialized through the worker.
    let device_shared = engine.device_shared();
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<arctis_engine::DeviceCommand>();
    engine.set_device_tx(cmd_tx);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_worker = std::sync::Arc::clone(&stop);
    let worker_handle = std::thread::Builder::new()
        .name("device-worker".into())
        .spawn(move || {
            arctis_engine::device::run_read_loop(
                HidOpener,
                device_shared,
                None, // no event forwarding in daemon (events go through engine event_sink in future)
                std::time::Duration::from_secs(2),
                stop_worker,
                Some(cmd_rx),
            );
        })
        .map_err(|e| EngineError::Ipc(format!("failed to spawn device worker: {e}")))?;

    let result = run_daemon_with_engine(&mut engine, &path);

    // Signal the worker to stop and join it.
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Err(e) = worker_handle.join() {
        eprintln!("daemon: device worker panicked: {:?}", e);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use arctis_audio::MockRunner;
    use arctis_config::{ChannelConfig, MicChainConfig, Profile};

    /// Shared mutex to serialize all tests that mutate the process-global
    /// `ASM_CONFIG_HOME` env var. Without this, parallel test threads clobber
    /// each other's env, causing intermittent failures.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn two_profile_config() -> Config {
        let channels = vec![
            ChannelConfig {
                id: "game".into(),
                node_name: "Arctis_Game".into(),
                description: "Game".into(),
                output_device: None,
                eq: vec![],
                volume_db: 0.0,
                muted: false,
            },
            ChannelConfig {
                id: "chat".into(),
                node_name: "Arctis_Chat".into(),
                description: "Chat".into(),
                output_device: None,
                eq: vec![],
                volume_db: 0.0,
                muted: false,
            },
            ChannelConfig {
                id: "media".into(),
                node_name: "Arctis_Media".into(),
                description: "Media".into(),
                output_device: None,
                eq: vec![],
                volume_db: 0.0,
                muted: false,
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
                    mic: MicChainConfig::default(),
                    surround: arctis_config::SurroundConfig::default(),
                },
                Profile {
                    name: "gaming".into(),
                    channels,
                    routes: vec![],
                    mic: MicChainConfig::default(),
                    surround: arctis_config::SurroundConfig::default(),
                },
            ],
            eq_presets: vec![],
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
        // Phase 2b: 3 channels × (1 ls + 1 Props set for volume/mute)
        for _ in 0..3 {
            r = r.with_output(0, &ls, "");
            r = r.with_output(0, "", "");
        }
        r
    }

    #[test]
    fn handle_switch_returns_state() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
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

    // ── New verb dispatch tests (TDD) ────────────────────────────────────────

    #[test]
    fn handle_set_channel_output_updates_state() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm9_sco_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // set_channel_output calls ChannelManager::set_output:
        // queues ls for present-check, then spawn if absent.
        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, &ls, "")
            .with_output(0, &ls, "");
        let cfg = two_profile_config();
        let mut engine = Engine::new(runner, cfg);

        let resp = handle_request(
            &mut engine,
            Request::SetChannelOutput {
                channel: "game".into(),
                device: Some("alsa_output.speakers".into()),
            },
        );
        assert!(resp.ok, "expected ok:true, got: {:?}", resp.error);
        let state = resp.state.expect("state must be present");
        let game = state.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(
            game.output_device,
            Some("alsa_output.speakers".into()),
            "state must reflect new output_device"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_set_channel_output_unknown_channel_errors() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::SetChannelOutput {
                channel: "nonexistent".into(),
                device: None,
            },
        );
        assert!(!resp.ok, "unknown channel must return ok:false");
        assert!(resp.error.is_some());
    }

    #[test]
    fn handle_profile_new_creates_and_returns_state() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm9_pn_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let runner = queue_reconcile_present(MockRunner::new());
        let cfg = two_profile_config();
        let mut engine = Engine::new(runner, cfg);

        let resp = handle_request(
            &mut engine,
            Request::ProfileNew {
                name: "competitive".into(),
            },
        );
        assert!(resp.ok, "expected ok:true, got: {:?}", resp.error);
        let state = resp.state.expect("state must be present");
        assert_eq!(state.active_profile, "competitive");
        assert!(
            state.profiles.contains(&"competitive".to_string()),
            "new profile must appear in profile list"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_profile_new_duplicate_errors() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::ProfileNew {
                name: "default".into(), // already exists
            },
        );
        assert!(!resp.ok, "duplicate name must return ok:false");
        assert!(resp.error.is_some());
    }

    // ── Task 6: device-set dispatch ─────────────────────────────────────────

    #[test]
    fn handle_device_set_returns_error_when_worker_not_wired() {
        // Engine without a device worker → device_tx is None → BadRequest.
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::DeviceSet {
                control: "sidetone".into(),
                value: 2,
            },
        );
        assert!(!resp.ok, "device-set must fail when worker not wired");
        assert!(
            resp.error.is_some(),
            "error message must be present in response"
        );
        let msg = resp.error.unwrap();
        assert!(
            msg.contains("not running") || msg.contains("device worker"),
            "error must mention worker: {msg}"
        );
    }

    #[test]
    fn handle_device_set_gated_control_returns_ok_false() {
        // Wire a fake worker that always replies with the gate-refused error.
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<arctis_engine::DeviceCommand>();
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        engine.set_device_tx(cmd_tx);

        // Fake worker: drain commands, always reply gate refused.
        let worker = std::thread::spawn(move || {
            while let Ok(arctis_engine::DeviceCommand::Set { reply, .. }) = cmd_rx.recv() {
                let _ = reply.send(Err(
                    "sidetone is not enabled (no validated OWNER-RUN gate)".into()
                ));
            }
        });

        let resp = handle_request(
            &mut engine,
            Request::DeviceSet {
                control: "sidetone".into(),
                value: 2,
            },
        );
        assert!(!resp.ok, "gate-refused write must return ok:false");
        let msg = resp.error.expect("error must be present");
        assert!(
            msg.contains("not enabled") || msg.contains("OWNER-RUN"),
            "error must describe the gate: {msg}"
        );

        drop(engine);
        worker.join().expect("fake worker must not panic");
    }

    // ── Task 3: parse_suppression_backend unit tests ────────────────────────

    #[test]
    fn parse_suppression_backend_deep_filter() {
        use arctis_engine::SuppressionBackend;
        assert!(matches!(
            super::parse_suppression_backend("deep_filter"),
            Ok(SuppressionBackend::DeepFilter)
        ));
    }

    #[test]
    fn parse_suppression_backend_rnnoise() {
        use arctis_engine::SuppressionBackend;
        assert!(matches!(
            super::parse_suppression_backend("rnnoise"),
            Ok(SuppressionBackend::Rnnoise)
        ));
    }

    #[test]
    fn parse_suppression_backend_unknown_returns_bad_request() {
        let e = super::parse_suppression_backend("invalid_backend").unwrap_err();
        assert!(
            matches!(e, EngineError::BadRequest(_)),
            "unknown backend must be BadRequest"
        );
        assert!(
            e.to_string().contains("invalid_backend"),
            "error must include input"
        );
    }

    // ── Task 3: MicSuppressionBackend dispatch test ──────────────────────────

    #[test]
    fn handle_mic_suppression_backend_deep_filter_returns_ok() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_t3_dfbe_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::MicSuppressionBackend {
                backend: "deep_filter".into(),
            },
        );
        assert!(
            resp.ok,
            "MicSuppressionBackend deep_filter must return ok:true, got: {:?}",
            resp.error
        );
        assert!(resp.state.is_some(), "state must be present in response");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_mic_suppression_backend_rnnoise_returns_ok() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_t3_rnbe_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::MicSuppressionBackend {
                backend: "rnnoise".into(),
            },
        );
        assert!(
            resp.ok,
            "MicSuppressionBackend rnnoise must return ok:true, got: {:?}",
            resp.error
        );
        assert!(resp.state.is_some(), "state must be present in response");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_mic_suppression_backend_unknown_returns_error() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::MicSuppressionBackend {
                backend: "bogus_backend".into(),
            },
        );
        assert!(!resp.ok, "unknown backend must return ok:false");
        assert!(resp.error.is_some(), "error must be present");
        assert!(
            resp.error.as_deref().unwrap().contains("bogus_backend"),
            "error must include input"
        );
    }

    // ── Task 5: parse_mic_stage unit tests ──────────────────────────────────

    #[test]
    fn parse_mic_stage_all_valid_mappings() {
        use arctis_audio::StageKind;
        assert!(matches!(
            super::parse_mic_stage("gain"),
            Ok(StageKind::Gain)
        ));
        assert!(matches!(
            super::parse_mic_stage("highpass"),
            Ok(StageKind::Highpass)
        ));
        assert!(matches!(
            super::parse_mic_stage("suppression"),
            Ok(StageKind::Suppression)
        ));
        // "rnnoise" is kept as a backward-compat alias
        assert!(matches!(
            super::parse_mic_stage("rnnoise"),
            Ok(StageKind::Suppression)
        ));
        assert!(matches!(
            super::parse_mic_stage("compressor"),
            Ok(StageKind::Compressor)
        ));
        assert!(matches!(
            super::parse_mic_stage("gate"),
            Ok(StageKind::Gate)
        ));
        assert!(matches!(super::parse_mic_stage("eq"), Ok(StageKind::MicEq)));
    }

    #[test]
    fn parse_mic_stage_unknown_returns_bad_request() {
        let e = super::parse_mic_stage("invalid").unwrap_err();
        assert!(
            matches!(e, EngineError::BadRequest(_)),
            "unknown stage must be BadRequest"
        );
        assert!(
            e.to_string().contains("invalid"),
            "error must include input"
        );
    }

    // ── Task 5: parse_mic_param unit tests ──────────────────────────────────

    #[test]
    fn parse_mic_param_all_valid_mappings() {
        use arctis_engine::MicParam;
        assert!(matches!(
            super::parse_mic_param("gain_db"),
            Ok(MicParam::GainDb)
        ));
        assert!(matches!(
            super::parse_mic_param("highpass_freq"),
            Ok(MicParam::HighpassFreq)
        ));
        assert!(matches!(
            super::parse_mic_param("attenuation_limit_db"),
            Ok(MicParam::AttenuationLimitDb)
        ));
        assert!(matches!(
            super::parse_mic_param("vad_threshold"),
            Ok(MicParam::VadThreshold)
        ));
        assert!(matches!(
            super::parse_mic_param("vad_grace_ms"),
            Ok(MicParam::VadGraceMs)
        ));
        assert!(matches!(
            super::parse_mic_param("vad_retro_grace_ms"),
            Ok(MicParam::VadRetroGraceMs)
        ));
        assert!(matches!(
            super::parse_mic_param("gate_threshold"),
            Ok(MicParam::GateThreshold)
        ));
        assert!(matches!(
            super::parse_mic_param("comp_threshold_db"),
            Ok(MicParam::CompThresholdDb)
        ));
        assert!(matches!(
            super::parse_mic_param("comp_ratio"),
            Ok(MicParam::CompRatio)
        ));
        assert!(matches!(
            super::parse_mic_param("comp_makeup_db"),
            Ok(MicParam::CompMakeupDb)
        ));
    }

    #[test]
    fn parse_mic_param_unknown_returns_bad_request() {
        let e = super::parse_mic_param("bogus_param").unwrap_err();
        assert!(
            matches!(e, EngineError::BadRequest(_)),
            "unknown param must be BadRequest"
        );
        assert!(
            e.to_string().contains("bogus_param"),
            "error must include input"
        );
    }

    // ── Task 5: mic dispatch tests ───────────────────────────────────────────

    #[test]
    fn handle_mic_status_returns_ok_with_mic_snapshot() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(&mut engine, Request::MicStatus);
        assert!(resp.ok, "MicStatus must return ok:true");
        let state = resp.state.expect("state must be present");
        // mic snapshot is always present (even with default config)
        // stages vec must have entries
        assert!(
            !state.mic.stages.is_empty(),
            "mic snapshot must have stage entries"
        );
    }

    #[test]
    fn handle_mic_stage_unknown_returns_error() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::MicStage {
                stage: "nonexistent_stage".into(),
                enabled: true,
            },
        );
        assert!(!resp.ok, "unknown stage must return ok:false");
        assert!(resp.error.is_some());
    }

    // ── F1.4: surround dispatch tests ────────────────────────────────────────

    #[test]
    fn handle_surround_status_returns_ok_with_surround_snapshot() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(&mut engine, Request::SurroundStatus);
        assert!(resp.ok, "SurroundStatus must return ok:true");
        let state = resp.state.expect("state must be present");
        // surround snapshot is always present (default disabled, no HRIR)
        assert!(!state.surround.enabled, "default surround must be disabled");
    }

    #[test]
    fn handle_surround_enable_false_returns_ok() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        // disable (already disabled by default) — must return ok
        let resp = handle_request(&mut engine, Request::SurroundEnable { enabled: false });
        assert!(
            resp.ok,
            "SurroundEnable false must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.expect("state must be present");
        assert!(!state.surround.enabled);
    }

    #[test]
    fn handle_surround_set_hrir_unknown_name_returns_error() {
        // No HRIR profiles dir → requesting any specific stem returns an error.
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::SurroundSetHrir {
                name: "nonexistent-hrir".into(),
            },
        );
        // The engine returns BadRequest (no profiles dir / stem not found) → Response::err.
        assert!(
            !resp.ok,
            "SurroundSetHrir with unknown stem must return ok:false"
        );
        assert!(resp.error.is_some(), "error must be present");
    }

    #[test]
    fn handle_surround_set_channels_updates_state() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_f14_sc_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::SurroundSetChannels {
                channels: vec!["game".into()],
            },
        );
        assert!(
            resp.ok,
            "SurroundSetChannels must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.expect("state must be present");
        assert_eq!(state.surround.channels, vec!["game".to_string()]);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_surround_set_hw_sink_updates_state() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_f14_hs_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::SurroundSetHwSink {
                hw_sink: Some("alsa_output.usb-SteelSeries".into()),
            },
        );
        assert!(
            resp.ok,
            "SurroundSetHwSink must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.expect("state must be present");
        assert_eq!(
            state.surround.hw_sink,
            Some("alsa_output.usb-SteelSeries".into())
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_surround_set_hw_sink_none_clears_pin() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_f14_hsn_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(&mut engine, Request::SurroundSetHwSink { hw_sink: None });
        assert!(
            resp.ok,
            "SurroundSetHwSink None must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.expect("state must be present");
        assert_eq!(state.surround.hw_sink, None);

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_mic_set_unknown_param_returns_error() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::MicSet {
                param: "bogus_param".into(),
                value: 1.0,
            },
        );
        assert!(!resp.ok, "unknown param must return ok:false");
        assert!(resp.error.is_some());
    }

    #[test]
    fn handle_device_set_accepted_returns_ok_true_with_state() {
        // Wire a fake worker that always replies Ok(()).
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<arctis_engine::DeviceCommand>();
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        engine.set_device_tx(cmd_tx);

        let worker = std::thread::spawn(move || {
            while let Ok(arctis_engine::DeviceCommand::Set { reply, .. }) = cmd_rx.recv() {
                let _ = reply.send(Ok(()));
            }
        });

        let resp = handle_request(
            &mut engine,
            Request::DeviceSet {
                control: "sidetone".into(),
                value: 2,
            },
        );
        assert!(resp.ok, "accepted write must return ok:true");
        assert!(resp.state.is_some(), "state must be present in response");

        drop(engine);
        worker.join().expect("fake worker must not panic");
    }

    // ── F3a: profile rename/delete/export/import dispatch tests ─────────────

    #[test]
    fn handle_profile_rename_returns_state() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_f3a_ren_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::ProfileRename {
                old: "gaming".into(),
                new: "competitive".into(),
            },
        );
        assert!(
            resp.ok,
            "profile rename must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.expect("state must be present");
        assert!(
            state.profiles.contains(&"competitive".to_string()),
            "renamed profile must appear in state"
        );
        assert!(
            !state.profiles.contains(&"gaming".to_string()),
            "old name must not appear"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_profile_rename_unknown_returns_error() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::ProfileRename {
                old: "nonexistent".into(),
                new: "new_name".into(),
            },
        );
        assert!(
            !resp.ok,
            "rename of nonexistent profile must return ok:false"
        );
        assert!(resp.error.is_some());
    }

    #[test]
    fn handle_profile_delete_returns_state() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_f3a_del_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::ProfileDelete {
                name: "gaming".into(),
            },
        );
        assert!(
            resp.ok,
            "profile delete must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.expect("state must be present");
        assert!(
            !state.profiles.contains(&"gaming".to_string()),
            "deleted profile must not appear in state"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_profile_delete_active_returns_error() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::ProfileDelete {
                name: "default".into(), // active profile
            },
        );
        assert!(!resp.ok, "deleting active profile must return ok:false");
        assert!(resp.error.is_some());
    }

    #[test]
    fn handle_profile_export_returns_toml_text() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::ProfileExport {
                name: "gaming".into(),
            },
        );
        assert!(
            resp.ok,
            "profile export must return ok:true, got: {:?}",
            resp.error
        );
        assert!(
            resp.state.is_none(),
            "export response must not include state"
        );
        let text = resp.text.expect("export must return text payload");
        assert!(
            text.contains("gaming"),
            "exported TOML must mention the profile name"
        );
    }

    #[test]
    fn handle_profile_export_unknown_returns_error() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::ProfileExport {
                name: "nonexistent".into(),
            },
        );
        assert!(
            !resp.ok,
            "export of nonexistent profile must return ok:false"
        );
        assert!(resp.error.is_some());
    }

    #[test]
    fn handle_profile_import_returns_state() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_f3a_imp_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        // First export an existing profile to get valid TOML
        let export_resp = handle_request(
            &mut engine,
            Request::ProfileExport {
                name: "gaming".into(),
            },
        );
        assert!(export_resp.ok);
        let gaming_toml = export_resp.text.unwrap();

        // Modify the TOML to rename it so there's no collision
        let renamed_toml = gaming_toml.replace("gaming", "imported_profile");

        let resp = handle_request(&mut engine, Request::ProfileImport { toml: renamed_toml });
        assert!(
            resp.ok,
            "profile import must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.expect("state must be present");
        assert!(
            state
                .profiles
                .iter()
                .any(|n| n.contains("imported_profile")),
            "imported profile must appear in state: {:?}",
            state.profiles
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_profile_import_invalid_toml_returns_error() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::ProfileImport {
                toml: "this is not valid toml !!!".into(),
            },
        );
        assert!(!resp.ok, "invalid TOML import must return ok:false");
        assert!(resp.error.is_some());
    }

    // ── F3a: EQ preset dispatch tests ──────────────────────────────────────

    #[test]
    fn handle_eq_preset_save_returns_state() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_f3a_prsave_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::EqPresetSave {
                name: "my-preset".into(),
                channel: "game".into(),
            },
        );
        assert!(
            resp.ok,
            "eq preset save must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.expect("state must be present");
        assert!(
            state.eq_presets.iter().any(|p| p.name == "my-preset"),
            "saved preset must appear in state"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_eq_preset_save_unknown_channel_errors() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::EqPresetSave {
                name: "my-preset".into(),
                channel: "nonexistent".into(),
            },
        );
        assert!(!resp.ok, "unknown channel must return ok:false");
        assert!(resp.error.is_some());
    }

    #[test]
    fn handle_eq_preset_delete_returns_state() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_f3a_prdel_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        // First save a preset
        let save_resp = handle_request(
            &mut engine,
            Request::EqPresetSave {
                name: "my-preset".into(),
                channel: "game".into(),
            },
        );
        assert!(save_resp.ok);

        // Then delete it
        let resp = handle_request(
            &mut engine,
            Request::EqPresetDelete {
                name: "my-preset".into(),
            },
        );
        assert!(
            resp.ok,
            "eq preset delete must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.expect("state must be present");
        assert!(
            !state.eq_presets.iter().any(|p| p.name == "my-preset"),
            "deleted preset must not appear in state"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_eq_preset_delete_unknown_returns_error() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::EqPresetDelete {
                name: "nonexistent-preset".into(),
            },
        );
        assert!(!resp.ok, "deleting nonexistent preset must return ok:false");
        assert!(resp.error.is_some());
    }

    #[test]
    fn handle_eq_preset_apply_returns_state() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_f3a_apply_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let ls = ls_all_present();
        // apply_eq_preset calls apply_all for the channel: 1 ls (find_node_id) + 10 band sets
        let mut runner = MockRunner::new();
        // save_eq_preset: no runner calls needed
        // apply_eq_preset → apply_all → find_node_id ls + 10 band set calls
        runner = runner.with_output(0, &ls, ""); // find_node_id
        for _ in 0..10 {
            runner = runner.with_output(0, "", ""); // band set Props
        }

        let cfg = two_profile_config();
        let mut engine = Engine::new(runner, cfg);

        // First save a preset from the game channel
        let save_resp = handle_request(
            &mut engine,
            Request::EqPresetSave {
                name: "my-preset".into(),
                channel: "game".into(),
            },
        );
        assert!(save_resp.ok, "save must succeed before apply");

        // Then apply the preset to the chat channel
        let resp = handle_request(
            &mut engine,
            Request::EqPresetApply {
                preset: "my-preset".into(),
                channel: "chat".into(),
            },
        );
        assert!(
            resp.ok,
            "eq preset apply must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.expect("state must be present after apply");
        // Preset still visible in state
        assert!(
            state.eq_presets.iter().any(|p| p.name == "my-preset"),
            "applied preset must still appear in state"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_eq_preset_apply_unknown_preset_errors() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let resp = handle_request(
            &mut engine,
            Request::EqPresetApply {
                preset: "nonexistent-preset".into(),
                channel: "game".into(),
            },
        );
        assert!(!resp.ok, "applying nonexistent preset must return ok:false");
        assert!(resp.error.is_some());
    }

    #[test]
    fn handle_eq_preset_apply_unknown_channel_errors() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_f3a_applych_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        // Save a preset first
        let save_resp = handle_request(
            &mut engine,
            Request::EqPresetSave {
                name: "my-preset".into(),
                channel: "game".into(),
            },
        );
        assert!(save_resp.ok);

        // Apply to nonexistent channel
        let resp = handle_request(
            &mut engine,
            Request::EqPresetApply {
                preset: "my-preset".into(),
                channel: "nonexistent".into(),
            },
        );
        assert!(
            !resp.ok,
            "applying to nonexistent channel must return ok:false"
        );
        assert!(resp.error.is_some());

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    // ── F2.1: SetChannelVolume / SetChannelMute dispatch tests ───────────────

    #[test]
    fn handle_set_channel_volume_ok() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_vol_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let ls = ls_all_present();
        // set_channel_volume calls apply_volume_mute: 1 ls (find_node_id) + 1 Props set
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // find_node_id
            .with_output(0, "", ""); // pw-cli s ... Props ...
        let cfg = two_profile_config();
        let mut engine = Engine::new(runner, cfg);

        let resp = handle_request(
            &mut engine,
            Request::SetChannelVolume {
                channel: "game".into(),
                volume_db: -6.0,
            },
        );
        assert!(
            resp.ok,
            "set_channel_volume must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.unwrap();
        let game = state.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(game.volume_db, -6.0, "volume_db must be updated in state");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_set_channel_mute_ok() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = std::env::temp_dir().join(format!("asm_mute_{}", std::process::id()));
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let ls = ls_all_present();
        // set_channel_mute calls apply_volume_mute: 1 ls + 1 Props set
        let runner = MockRunner::new()
            .with_output(0, &ls, "")
            .with_output(0, "", "");
        let cfg = two_profile_config();
        let mut engine = Engine::new(runner, cfg);

        let resp = handle_request(
            &mut engine,
            Request::SetChannelMute {
                channel: "chat".into(),
                muted: true,
            },
        );
        assert!(
            resp.ok,
            "set_channel_mute must return ok:true, got: {:?}",
            resp.error
        );
        let state = resp.state.unwrap();
        let chat = state.channels.iter().find(|c| c.id == "chat").unwrap();
        assert!(chat.muted, "muted must be true in state");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn handle_set_channel_volume_out_of_range_errors() {
        let cfg = two_profile_config();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        let resp = handle_request(
            &mut engine,
            Request::SetChannelVolume {
                channel: "game".into(),
                volume_db: 99.0, // out of range
            },
        );
        assert!(!resp.ok, "out-of-range volume must return ok:false");
        assert!(resp.error.is_some());
    }

    // ── Integration test: shutdown breaks accept loop ─────────────────────────

    /// Fix #1 integration test: send `{"cmd":"shutdown"}` to a real Unix socket
    /// backed by a MockRunner engine. The daemon thread must exit promptly (no
    /// blocking accept()) and the socket file must be removed.
    ///
    /// This is the test that would have caught the original hang.
    #[test]
    fn shutdown_breaks_accept_loop_and_removes_socket() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;

        // Unique socket path per test run to avoid collisions.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let sock_path = std::env::temp_dir().join(format!(
            "asm_shutdown_test_{}_{}.sock",
            std::process::id(),
            nanos
        ));

        // Use a temp ASM_CONFIG_HOME so save_config doesn't touch real files.
        let tmp_cfg =
            std::env::temp_dir().join(format!("asm_shutdown_cfg_{}_{}", std::process::id(), nanos));

        // Build a MockRunner-backed engine with sinks all reported as present
        // so reconcile-on-start spawns nothing real.
        let runner = queue_reconcile_present(MockRunner::new());
        let cfg = two_profile_config();
        let mut engine = Engine::new(runner, cfg);

        // Reconcile up-front (mimics what run_daemon does before the loop).
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("ASM_CONFIG_HOME", &tmp_cfg);
        if let Err(e) = engine.reconcile() {
            eprintln!("pre-reconcile warning (test): {e}");
        }
        std::env::remove_var("ASM_CONFIG_HOME");

        let sock_path_clone = sock_path.clone();

        // Spawn daemon loop on a background thread.
        let handle =
            std::thread::spawn(move || run_daemon_with_engine(&mut engine, &sock_path_clone));

        // Wait briefly for the socket to appear (daemon thread must bind first).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !sock_path.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "daemon did not create socket within 3s"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        // Connect and send shutdown.
        let stream = UnixStream::connect(&sock_path).expect("connect to daemon socket");
        let mut writer = stream.try_clone().expect("try_clone");
        writeln!(writer, r#"{{"cmd":"shutdown"}}"#).expect("write shutdown");

        // Read the response.
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read response");
        let resp: Response = serde_json::from_str(line.trim()).expect("parse response JSON");
        assert!(resp.ok, "shutdown response must be ok:true");

        // The daemon thread must exit promptly — no blocking accept().
        let mut joined = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if handle.is_finished() {
                joined = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            joined,
            "daemon thread must exit promptly after shutdown (no blocking accept hang)"
        );
        let result = handle.join().expect("thread panicked");
        assert!(result.is_ok(), "run_daemon_with_engine must return Ok");

        // Socket file must be removed by the daemon.
        assert!(
            !sock_path.exists(),
            "socket file must be removed after shutdown"
        );

        let _ = std::fs::remove_dir_all(&tmp_cfg);
    }
}
