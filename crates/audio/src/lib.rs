//! Subprocess-driven PipeWire audio backend: virtual EQ sink lifecycle and
//! live parametric-EQ control. Pure generators are unit-tested with no daemon;
//! the daemon-touching path runs only under owner hardware tests (G8).
pub mod backend;
pub mod config;
pub mod eq;
pub mod error;
pub mod props;
pub mod runner;

pub use backend::{AudioBackend, ConfHandle};
pub use config::{band_node_name, render_filter_chain_conf, SinkSpec};
pub use eq::{
    BandKind, EqBand, EqModel, FREQ_MAX_HZ, FREQ_MIN_HZ, GAIN_MAX_DB, GAIN_MIN_DB, MAX_BANDS,
    Q_MAX, Q_MIN, SAMPLE_RATE_HZ,
};
pub use error::AudioError;
pub use props::{band_props_json, set_band_props_argv};
pub use runner::{CmdOutput, CommandRunner, MockRunner, RealRunner};
