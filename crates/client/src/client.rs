use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::protocol::{socket_path, Request, Response};

/// Default read/write timeout for one daemon round trip. Bounded so a wedged
/// daemon can never hang the GUI/CLI caller forever.
pub const DEFAULT_IPC_TIMEOUT: Duration = Duration::from_secs(10);
/// ChatMix validation legitimately takes ~6 s of device reads plus queueing —
/// give it more headroom than the default.
pub const CHATMIX_VALIDATE_TIMEOUT: Duration = Duration::from_secs(25);
/// Maximum accepted response-line length (bytes) — mirrors the daemon's
/// request-line cap so a broken peer cannot OOM the client.
const MAX_RESPONSE_LINE: u64 = 1 << 20; // 1 MiB

/// Pick the round-trip timeout for a request (ChatmixValidate is slow by design).
fn timeout_for(req: &Request) -> Duration {
    match req {
        Request::ChatmixValidate => CHATMIX_VALIDATE_TIMEOUT,
        _ => DEFAULT_IPC_TIMEOUT,
    }
}

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
/// Uses a per-request default timeout (see [`timeout_for`]).
pub fn send_request_to(path: &Path, req: &Request) -> Result<Response, ClientError> {
    send_request_to_with_timeout(path, req, timeout_for(req))
}

/// Same as `send_request_to`, with an explicit read/write timeout.
pub fn send_request_to_with_timeout(
    path: &Path,
    req: &Request,
    timeout: Duration,
) -> Result<Response, ClientError> {
    let stream = UnixStream::connect(path).map_err(|source| ClientError::Connect {
        path: path.display().to_string(),
        source,
    })?;
    // Bound both directions so a wedged daemon cannot hang the caller.
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let req_str = serde_json::to_string(req)?;
    let mut writer = stream.try_clone()?;
    writeln!(writer, "{req_str}")?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    // Cap the response line so a broken peer cannot grow the buffer unboundedly.
    let n = reader.by_ref().take(MAX_RESPONSE_LINE + 1).read_line(&mut line)?;
    if n as u64 > MAX_RESPONSE_LINE {
        return Err(ClientError::Daemon(format!(
            "response line too long (> {MAX_RESPONSE_LINE} bytes)"
        )));
    }
    Ok(serde_json::from_str(line.trim())?)
}
