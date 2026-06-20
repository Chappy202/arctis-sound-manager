use crate::transport::{Transport, TransportError};
use std::collections::VecDeque;

/// An in-memory transport for tests. Records writes; replays queued responses.
#[derive(Default)]
pub struct MockTransport {
    pub written: Vec<Vec<u8>>,
    responses: VecDeque<Vec<u8>>,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a frame to be returned by the next `read_report`.
    pub fn with_response(mut self, frame: Vec<u8>) -> Self {
        self.responses.push_back(frame);
        self
    }
}

impl Transport for MockTransport {
    fn write_report(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.written.push(data.to_vec());
        Ok(())
    }

    fn read_report(&mut self, buf: &mut [u8], _timeout_ms: i32) -> Result<usize, TransportError> {
        let frame = self.responses.pop_front().ok_or(TransportError::Timeout)?;
        let n = frame.len().min(buf.len());
        buf[..n].copy_from_slice(&frame[..n]);
        Ok(n)
    }
}
