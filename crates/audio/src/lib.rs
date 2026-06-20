//! Subprocess-driven PipeWire audio backend: virtual EQ sink lifecycle and
//! live parametric-EQ control. Pure generators are unit-tested with no daemon;
//! the daemon-touching path runs only under owner hardware tests (G8).
pub mod error;
pub mod runner;

pub use error::AudioError;
pub use runner::{CmdOutput, CommandRunner, MockRunner, RealRunner};
