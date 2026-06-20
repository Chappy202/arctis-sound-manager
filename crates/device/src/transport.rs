use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("device not found: {0}")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("read timed out")]
    Timeout,
}

/// A raw HID byte transport. The report id is included as the first byte of
/// every written buffer. Reads return a single input report.
pub trait Transport {
    fn write_report(&mut self, data: &[u8]) -> Result<(), TransportError>;
    fn read_report(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, TransportError>;
}
