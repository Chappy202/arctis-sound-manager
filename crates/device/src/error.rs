use crate::transport::TransportError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("device transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("unsupported command: {0}")]
    Unsupported(String),
    #[error("invalid value for command '{cmd}': {detail}")]
    InvalidValue { cmd: String, detail: String },
    #[error("device not connected")]
    NotConnected,
}
