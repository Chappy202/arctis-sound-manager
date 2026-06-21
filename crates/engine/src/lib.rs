//! `arctis-engine` — orchestrator for the SteelSeries Arctis sound manager.
//!
//! Composes `arctis-audio`, `arctis-device`, `arctis-config`, and `arctis-domain`
//! into a single engine that owns the process lifecycle of PipeWire children.
pub mod children;
pub mod convert;
pub mod device;
pub mod engine;
pub mod error;
pub mod state;

pub use children::ChildOwner;
pub use device::{DeviceCommand, DeviceOpener};
pub use engine::Engine;
pub use error::EngineError;
pub use state::{
    ChannelSnapshot, DeviceShared, EngineState, EqBandSnapshot, EqPresetSnapshot, Event, MicParam,
    MicSnapshot, MicStageSnapshot, StageAvailability, StageName, SuppressionBackend,
    SurroundSnapshot,
};
