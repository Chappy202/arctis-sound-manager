use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use crate::protocol::{socket_path, Request, Response};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("connect to {path}: {source}")]
    Connect {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("daemon error: {0}")]
    Daemon(String),
}

/// One-shot blocking client: connect → write one request line → read one response line.
/// Uses the default daemon socket path from `socket_path()`.
pub fn send_request(req: &Request) -> Result<Response, ClientError> {
    send_request_to(&socket_path(), req)
}

/// Same as `send_request`, but to an explicit socket path (used by tests + src-tauri).
pub fn send_request_to(path: &Path, req: &Request) -> Result<Response, ClientError> {
    let stream = UnixStream::connect(path).map_err(|source| ClientError::Connect {
        path: path.display().to_string(),
        source,
    })?;
    let req_str = serde_json::to_string(req)?;
    let mut writer = stream.try_clone()?;
    writeln!(writer, "{req_str}")?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}
