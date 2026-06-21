//! Protocol types + Unix-socket client for the arctis daemon.
//! Shared by `asm-cli` and the Tauri backend. NO tauri/audio deps.
pub mod client;
pub mod protocol;

pub use client::{send_request, send_request_to, ClientError};
pub use protocol::{
    socket_path, CoexistActionResult, CoexistDisableResult, CoexistReport, Request, Response,
};
