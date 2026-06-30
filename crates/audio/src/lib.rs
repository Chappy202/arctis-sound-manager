//! Subprocess-driven PipeWire audio backend: virtual EQ sink lifecycle and
//! live parametric-EQ control. Pure generators are unit-tested with no daemon;
//! the daemon-touching path runs only under owner hardware tests (G8).
pub mod backend;
pub mod channels;
pub mod config;
pub mod eq;
pub mod error;
pub mod mic;
pub mod props;
pub mod pw_version;
pub mod routing;
pub mod runner;
pub mod sinks;
pub mod streams;
pub mod surround;

pub use backend::{AudioBackend, ConfHandle};
pub use channels::{ChannelDef, ChannelManager, ChannelSetConfig};
pub use config::{
    band_node_name, render_chain_conf, render_filter_chain_conf, ChainChannels, ChainKind,
    ChainSpec, FilterNode, NodeType, SinkSpec,
};
pub use eq::{
    BandKind, EqBand, EqModel, FREQ_MAX_HZ, FREQ_MIN_HZ, GAIN_MAX_DB, GAIN_MIN_DB, MAX_BANDS,
    Q_MAX, Q_MIN, SAMPLE_RATE_HZ,
};
pub use error::AudioError;
pub use mic::{
    resolve_ladspa, FsPluginProbe, MicBackend, MockPluginProbe, PluginProbe, StageKind,
    DEEPFILTER_LABEL_MONO, DEEPFILTER_PLUGIN_BASENAME, GATE_LABEL, GATE_PLUGIN_BASENAME,
    RNNOISE_LABEL_MONO, RNNOISE_PLUGIN_BASENAME, SC4M_LABEL, SC4M_PLUGIN_BASENAME,
};
pub use props::{
    band_props_json, control_props_json, node_volume_props_json, set_band_props_argv,
    set_control_props_argv, set_node_volume_props_argv,
};
pub use pw_version::{parse_pw_version, query_pw_version, supports_builtin_noisegate};
pub use routing::{
    clear_stream_target_argv, move_stream_argv, node_rules_fragment, parse_stream_id,
    wireplumber_fragment_path, AppMatch, RouteRule, Router,
};
pub use runner::{ChildToken, CmdOutput, CommandRunner, MockRunner, RealRunner};
pub use sinks::{
    parse_default_sink_name, parse_node_volume, parse_output_sinks, parse_stream_channels,
    OutputSink,
};
pub use streams::{
    classify_surround_input, parse_app_streams, richest_surround_input, ParsedStream, SurroundInput,
};
pub use surround::{
    render_surround_conf, render_surround_conf_ex, SurroundBackend, SurroundRender, SurroundSpec,
};
