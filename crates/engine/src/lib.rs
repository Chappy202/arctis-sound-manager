//! `arctis-engine` — orchestrator for the SteelSeries Arctis sound manager.
//!
//! Composes `arctis-audio`, `arctis-device`, `arctis-config`, and `arctis-domain`
//! into a single engine that owns the process lifecycle of PipeWire children.
pub mod children;
pub mod error;

pub use children::ChildOwner;
pub use error::EngineError;
