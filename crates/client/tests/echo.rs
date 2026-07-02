use arctis_client::{send_request_to, send_request_to_with_timeout, Request};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

/// A daemon that accepts but never replies must NOT hang the client forever:
/// the read timeout surfaces as an Err within a bounded time.
#[test]
fn send_request_times_out_when_daemon_never_replies() {
    let dir = std::env::temp_dir().join(format!("asm_client_timeout_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("mute.sock");
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).unwrap();

    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        // Keep the connection open without replying, longer than the client timeout.
        std::thread::sleep(std::time::Duration::from_millis(800));
    });

    let start = std::time::Instant::now();
    let result = send_request_to_with_timeout(
        &sock,
        &Request::GetState,
        std::time::Duration::from_millis(200),
    );
    assert!(result.is_err(), "silent daemon must yield an error, got: {result:?}");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(3),
        "client must give up promptly (read timeout), not hang"
    );

    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn send_request_to_round_trips() {
    let dir = std::env::temp_dir().join(format!("asm_client_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("echo.sock");
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).unwrap();

    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains("get-state"));
        writeln!(writer, r#"{{"ok":true}}"#).unwrap();
    });

    let resp = send_request_to(&sock, &Request::GetState).unwrap();
    assert!(resp.ok);
    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Integration test: MicStatus request reaches the server and the response
/// carries a valid EngineState with a `mic` snapshot.
#[test]
fn mic_status_request_carries_mic_snapshot() {
    let dir = std::env::temp_dir().join(format!("asm_mic_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("mic_status.sock");
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).unwrap();

    // Minimal valid EngineState JSON with a mic snapshot (single line for the wire protocol).
    let resp_line = r#"{"ok":true,"state":{"active_profile":"default","profiles":["default"],"channels":[],"routes":[],"device_present":false,"device_fields":{},"mic":{"enabled":false,"stages":[],"eq_bands":[]}}}"#.to_string();

    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        // Verify the request wire tag is present.
        assert!(
            line.contains("mic-status"),
            "request must contain 'mic-status', got: {line}"
        );
        writeln!(writer, "{}", resp_line).unwrap();
    });

    let resp = send_request_to(&sock, &Request::MicStatus).unwrap();
    assert!(resp.ok, "MicStatus response must be ok:true");
    let state = resp
        .state
        .expect("state must be present in MicStatus response");
    // The mic field must be present and deserializable.
    assert!(
        !state.active_profile.is_empty(),
        "active_profile must be non-empty"
    );
    // mic is always present in EngineState — assert the values match the fixture
    let mic = state.mic;
    assert!(
        !mic.enabled,
        "mic.enabled must match the response fixture (false)"
    );

    server.join().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
