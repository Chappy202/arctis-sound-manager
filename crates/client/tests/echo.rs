use arctis_client::{send_request_to, Request};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

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
