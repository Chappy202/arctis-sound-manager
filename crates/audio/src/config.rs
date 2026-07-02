use crate::eq::{EqModel, SAMPLE_RATE_HZ};
use crate::error::AudioError;

// ─── Node IR ────────────────────────────────────────────────────────────────

/// One node in a filter-chain graph. `port_in`/`port_out` are the node's actual
/// SPA port names: builtin biquad/linear/noisegate use "In"/"Out"; LADSPA nodes
/// use the plugin's real port names (e.g. RNNoise mono = "Input"/"Output").
#[derive(Debug, Clone, PartialEq)]
pub struct FilterNode {
    pub name: String,
    /// "builtin" or "ladspa".
    pub node_type: NodeType,
    /// builtin label (e.g. "bq_highpass", "linear", "noisegate") OR ladspa label.
    pub label: String,
    /// LADSPA plugin basename (e.g. "librnnoise_ladspa"); None for builtin.
    /// PipeWire resolves the basename across $LADSPA_PATH at runtime.
    pub plugin: Option<String>,
    pub port_in: String,
    pub port_out: String,
    /// Ordered control name → value (rendered into `control = { ... }`).
    pub controls: Vec<(String, f32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    Builtin,
    Ladspa,
}

impl NodeType {
    fn as_str(&self) -> &'static str {
        match self {
            NodeType::Builtin => "builtin",
            NodeType::Ladspa => "ladspa",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainChannels {
    Mono,
    Stereo,
    /// 8-channel 7.1. Position matches the surround convolver input exactly so a
    /// surround-routed channel feeds the HRIR 1:1. This is also the Windows 7.1
    /// WAVEFORMATEXTENSIBLE order (FL FR FC LFE rear-L rear-R side-L side-R), so
    /// any game outputting 7.1 maps correctly through winepulse/Proton.
    Surround71,
    /// 6-channel 5.1, matching the 5.1 convolver input (Hrir51 mode).
    Surround51,
}

impl ChainChannels {
    pub fn count(&self) -> u32 {
        match self {
            ChainChannels::Mono => 1,
            ChainChannels::Stereo => 2,
            ChainChannels::Surround51 => 6,
            ChainChannels::Surround71 => 8,
        }
    }
    fn position(&self) -> &'static str {
        match self {
            ChainChannels::Mono => "MONO",
            ChainChannels::Stereo => "FL FR",
            ChainChannels::Surround51 => "FL FR FC LFE RL RR",
            ChainChannels::Surround71 => "FL FR FC LFE RL RR SL SR",
        }
    }
}

/// Whether a filter-chain is a virtual sink (EQ) or a virtual source (mic).
///
/// This drives the capture.props / playback.props idiom:
/// - `Sink`: capture.props gets `media.class = Audio/Sink`; playback.props gets
///   `node.passive = true` and optional `target.object` (the hw sink).
/// - `Source`: capture.props gets `node.passive = true` (NOT media.class);
///   playback.props gets `media.class = Audio/Source` (what apps select).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainKind {
    Sink,
    Source,
}

/// Endpoint media classes + targets for a generic filter-chain instance.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainSpec {
    pub node_name: String,
    pub description: String,
    pub channels: ChainChannels,
    /// Source vs sink — drives the capture.props/playback.props idiom.
    pub kind: ChainKind,
    /// The node.name used in capture.props.
    /// EQ sink: bare node_name; mic source: "<node_name>.capture".
    pub capture_node_name: String,
    /// Optional target.object in capture.props (the hw mic node.name for a source).
    pub capture_target: Option<String>,
    /// Optional target.object in playback.props (the hw sink for an EQ sink).
    pub playback_target: Option<String>,
    /// The playback node.name.
    /// EQ sink: "<name>.output"; mic source: the bare "<name>".
    pub playback_node_name: String,
}

// ─── Identity + routing for one virtual EQ sink ─────────────────────────────

/// Identity + routing for one virtual EQ sink. `node_name` is stable so
/// create/remove are idempotent (G3).
#[derive(Debug, Clone)]
pub struct SinkSpec {
    pub node_name: String,
    pub description: String,
    /// Channel layout of the sink. `Stereo` for normal channels; `Surround71`
    /// for a surround-routed channel (so a game outputs discrete 7.1 into it).
    pub channels: ChainChannels,
    /// `Some(hardware_sink_node_name)` to pin the tail; `None` follows default.
    pub playback_target: Option<String>,
}

/// Stable per-band node name. This is the addressing root the live-EQ Props
/// generator (Task 4) uses as `"<band_node_name>:Freq"` etc.
pub fn band_node_name(index: usize) -> String {
    format!("eq_band_{index}")
}

/// Stable node name of the always-present auto-preamp `linear` node that heads
/// every channel-EQ filter-chain. Always rendered (Mult = 1.0 when no band
/// boosts) so the chain topology stays fixed and the preamp is live-updatable
/// via the existing `<node>:<control>` Props path (G3).
pub const PREAMP_NODE_NAME: &str = "eq_preamp";
/// The `linear` builtin control the preamp drives.
pub const PREAMP_CONTROL: &str = "Mult";

/// Format a value the way the conf expects: always emit a decimal point for
/// whole numbers so PipeWire parses them as floats.
fn fmt_num(v: f32) -> String {
    if v.fract() == 0.0 {
        format!("{:.1}", v)
    } else {
        // Trim to a stable short form (no scientific notation for our ranges).
        format!("{v}")
    }
}

// ─── Generic renderer ────────────────────────────────────────────────────────

/// Render the full `pipewire -c` conf for a generic filter-chain instance.
/// Works for both Audio/Sink (EQ) and Audio/Source (mic) endpoints.
///
/// Returns `AudioError::Invalid` if `nodes` is empty.
pub fn render_chain_conf(spec: &ChainSpec, nodes: &[FilterNode]) -> Result<String, AudioError> {
    if nodes.is_empty() {
        return Err(AudioError::Invalid(
            "filter-chain node list must not be empty".into(),
        ));
    }

    let rate = SAMPLE_RATE_HZ;
    let desc = &spec.description;
    let channels = spec.channels.count();
    let position = spec.channels.position();

    // ── nodes block ──────────────────────────────────────────────────────────
    let mut nodes_block = String::new();
    for node in nodes {
        // First line: type, name, label
        let mut node_line = format!(
            "                    {{   type = {}  name = \"{}\"  label = {}",
            node.node_type.as_str(),
            node.name,
            node.label,
        );
        // Optional plugin line (ladspa only)
        if let Some(ref plugin) = node.plugin {
            node_line.push('\n');
            node_line.push_str(&format!("                        plugin = \"{plugin}\""));
        }
        // Control line
        let mut ctrl = String::from("{ ");
        for (k, v) in &node.controls {
            ctrl.push_str(&format!("\"{}\" = {}  ", k, fmt_num(*v)));
        }
        // Remove trailing two spaces
        if ctrl.ends_with("  ") {
            ctrl.truncate(ctrl.len() - 2);
        }
        ctrl.push(' ');
        ctrl.push('}');
        node_line.push('\n');
        node_line.push_str(&format!("                        control = {ctrl}"));
        node_line.push('\n');
        node_line.push_str("                    }\n");
        nodes_block.push_str(&node_line);
    }

    // ── links block ──────────────────────────────────────────────────────────
    let mut links_block = String::new();
    for i in 1..nodes.len() {
        let prev = &nodes[i - 1];
        let curr = &nodes[i];
        let link_line = format!(
            "                    {{ output = \"{}:{}\"  input = \"{}:{}\" }}\n",
            prev.name, prev.port_out, curr.name, curr.port_in,
        );
        links_block.push_str(&link_line);
    }

    let first = &nodes[0];
    let last = &nodes[nodes.len() - 1];
    let first_in = format!("{}:{}", first.name, first.port_in);
    let last_out = format!("{}:{}", last.name, last.port_out);

    // ── capture.props ────────────────────────────────────────────────────────
    // Source chain: node.passive = true (NO media.class), optional target.object.
    // Sink chain:   media.class = Audio/Sink (NO node.passive).
    let capture_name = &spec.capture_node_name;
    let mut capture_inner = format!("                node.name   = \"{capture_name}\"\n");
    match spec.kind {
        ChainKind::Source => {
            capture_inner.push_str("                node.passive = true\n");
            if let Some(ref t) = spec.capture_target {
                capture_inner.push_str(&format!("                target.object = \"{t}\"\n"));
            }
        }
        ChainKind::Sink => {
            capture_inner.push_str("                media.class = Audio/Sink\n");
            // Sink chains do not pin a capture target.
        }
    }

    // ── playback.props ───────────────────────────────────────────────────────
    // Source chain: media.class = Audio/Source (what apps select). No node.passive.
    // Sink chain:   node.passive = true; optional target.object (hw sink).
    let mut playback_inner = format!(
        "                node.name   = \"{}\"\n",
        spec.playback_node_name
    );
    match spec.kind {
        ChainKind::Source => {
            playback_inner.push_str("                media.class = Audio/Source\n");
        }
        ChainKind::Sink => {
            playback_inner.push_str("                node.passive = true\n");
            if let Some(ref t) = spec.playback_target {
                playback_inner.push_str(&format!("                target.object = \"{t}\"\n"));
            }
        }
    }

    // ── assemble ─────────────────────────────────────────────────────────────
    let mut out = String::new();
    out.push_str("context.properties = {\n");
    out.push_str(&format!("    default.clock.rate = {rate}\n"));
    out.push_str(&format!("    default.clock.allowed-rates = [ {rate} ]\n"));
    out.push_str("}\n");
    out.push_str("context.spa-libs = {\n");
    out.push_str("    audio.convert.* = audioconvert/libspa-audioconvert\n");
    out.push_str("    support.*       = support/libspa-support\n");
    out.push_str("}\n");
    out.push_str("context.modules = [\n");
    out.push_str("    {   name = libpipewire-module-rt\n");
    out.push_str("        flags = [ ifexists nofail ]\n");
    out.push_str("    }\n");
    out.push_str("    {   name = libpipewire-module-protocol-native }\n");
    out.push_str("    {   name = libpipewire-module-client-node }\n");
    out.push_str("    {   name = libpipewire-module-adapter }\n");
    out.push_str("    {   name = libpipewire-module-filter-chain\n");
    out.push_str("        args = {\n");
    out.push_str(&format!("            node.description = \"{desc}\"\n"));
    out.push_str(&format!("            media.name       = \"{desc}\"\n"));
    out.push_str("            filter.graph = {\n");
    out.push_str("                nodes = [\n");
    out.push_str(&nodes_block);
    out.push_str("                ]\n");
    out.push_str("                links = [\n");
    out.push_str(&links_block);
    out.push_str("                ]\n");
    out.push_str(&format!("                inputs  = [ \"{first_in}\" ]\n"));
    out.push_str(&format!("                outputs = [ \"{last_out}\" ]\n"));
    out.push_str("            }\n");
    out.push_str(&format!("            audio.rate     = {rate}\n"));
    out.push_str(&format!("            audio.channels = {channels}\n"));
    out.push_str(&format!("            audio.position = [ {position} ]\n"));
    out.push_str("            capture.props = {\n");
    out.push_str(&capture_inner);
    out.push_str("            }\n");
    out.push_str("            playback.props = {\n");
    out.push_str(&playback_inner);
    out.push_str("            }\n");
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("]\n");

    Ok(out)
}

// ─── EQ sink adapter (thin wrapper over render_chain_conf) ──────────────────

/// Render the full `pipewire -c` conf for a filter-chain virtual EQ sink.
/// This is a thin adapter over `render_chain_conf`; the eq_sink_3band.conf
/// fixture must remain byte-identical (regression guard).
pub fn render_filter_chain_conf(spec: &SinkSpec, eq: &EqModel) -> Result<String, AudioError> {
    eq.validate()?;

    // Auto-preamp head node (always present so the topology never changes):
    // compensates the largest boosted band (AutoEq convention) so boosted
    // curves cannot clip at the float→S16 device boundary.
    let mut nodes: Vec<FilterNode> = vec![FilterNode {
        name: PREAMP_NODE_NAME.to_string(),
        node_type: NodeType::Builtin,
        label: "linear".to_string(),
        plugin: None,
        port_in: "In".to_string(),
        port_out: "Out".to_string(),
        controls: vec![
            (PREAMP_CONTROL.to_string(), eq.preamp_mult()),
            ("Add".to_string(), 0.0),
        ],
    }];
    // Then one biquad per EQ band.
    nodes.extend(eq.bands.iter().enumerate().map(|(i, b)| FilterNode {
        name: band_node_name(i),
        node_type: NodeType::Builtin,
        label: b.kind.label().to_string(),
        plugin: None,
        port_in: "In".to_string(),
        port_out: "Out".to_string(),
        controls: vec![
            ("Freq".to_string(), b.freq_hz),
            ("Q".to_string(), b.q),
            ("Gain".to_string(), b.gain_db),
        ],
    }));

    let chain_spec = ChainSpec {
        node_name: spec.node_name.clone(),
        description: spec.description.clone(),
        channels: spec.channels,
        kind: ChainKind::Sink,
        capture_node_name: spec.node_name.clone(),
        capture_target: None,
        playback_target: spec.playback_target.clone(),
        playback_node_name: format!("{}.output", spec.node_name),
    };

    render_chain_conf(&chain_spec, &nodes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::{BandKind, EqBand};

    fn three_band() -> EqModel {
        EqModel {
            bands: vec![
                EqBand::new(BandKind::LowShelf, 100.0, 0.7, 3.0),
                EqBand::new(BandKind::Peaking, 1000.0, 1.0, 0.0),
                EqBand::new(BandKind::HighShelf, 8000.0, 0.7, -2.0),
            ],
        }
    }

    // ── EQ sink regression ────────────────────────────────────────────────────

    #[test]
    fn render_filter_chain_conf_surround71_emits_8_channels_71_position() {
        // A surround-routed channel sink must advertise itself as 8-channel 7.1 so a
        // game outputs discrete surround into it (which then feeds the HRIR convolver).
        // The position must match the convolver input layout exactly (1:1 channel map).
        let spec = SinkSpec {
            node_name: "Arctis_Game".into(),
            description: "Arctis Game".into(),
            channels: ChainChannels::Surround71,
            playback_target: Some("effect_input.arctis_surround".into()),
        };
        let conf = render_filter_chain_conf(&spec, &three_band()).unwrap();
        assert!(
            conf.contains("audio.channels = 8"),
            "surround channel sink must be 8-channel, got:\n{conf}"
        );
        assert!(
            conf.contains("audio.position = [ FL FR FC LFE RL RR SL SR ]"),
            "surround channel sink must use the 7.1 layout matching the convolver, got:\n{conf}"
        );
    }

    #[test]
    fn renders_exact_fixture() {
        let spec = SinkSpec {
            node_name: "arctis_eq".into(),
            description: "Arctis EQ Sink".into(),
            channels: ChainChannels::Stereo,
            playback_target: Some("alsa_output.hw0".into()),
        };
        let got = render_filter_chain_conf(&spec, &three_band()).unwrap();
        let want = include_str!("../tests/fixtures/eq_sink_3band.conf");
        if got != want {
            eprintln!("=== GOT ===\n{got}\n=== WANT ===\n{want}");
        }
        assert_eq!(got, want);
    }

    #[test]
    fn omits_target_when_none() {
        let spec = SinkSpec {
            node_name: "arctis_eq".into(),
            description: "Arctis EQ Sink".into(),
            channels: ChainChannels::Stereo,
            playback_target: None,
        };
        let got = render_filter_chain_conf(&spec, &three_band()).unwrap();
        assert!(!got.contains("target.object"));
    }

    #[test]
    fn band_node_names_are_stable() {
        assert_eq!(band_node_name(0), "eq_band_0");
        assert_eq!(band_node_name(7), "eq_band_7");
    }

    /// The auto-preamp node is ALWAYS rendered — Mult 1.0 for flat/cut-only
    /// curves — so the chain topology never changes and the preamp stays
    /// live-updatable through the Props path (G3).
    #[test]
    fn preamp_node_always_present_unity_when_no_boost() {
        let spec = SinkSpec {
            node_name: "arctis_eq".into(),
            description: "Arctis EQ Sink".into(),
            channels: ChainChannels::Stereo,
            playback_target: None,
        };
        let flat = EqModel {
            bands: vec![EqBand::new(BandKind::Peaking, 1000.0, 1.0, 0.0)],
        };
        let got = render_filter_chain_conf(&spec, &flat).unwrap();
        assert!(
            got.contains("name = \"eq_preamp\"  label = linear"),
            "preamp node must be present even for a flat curve:\n{got}"
        );
        assert!(
            got.contains("control = { \"Mult\" = 1.0  \"Add\" = 0.0 }"),
            "flat curve → unity preamp:\n{got}"
        );
        assert!(
            got.contains("inputs  = [ \"eq_preamp:In\" ]"),
            "chain input must be the preamp node:\n{got}"
        );
        assert!(
            got.contains("{ output = \"eq_preamp:Out\"  input = \"eq_band_0:In\" }"),
            "preamp must link into band 0:\n{got}"
        );
    }

    // ── Mic source fixture tests ──────────────────────────────────────────────

    fn passthrough_spec() -> ChainSpec {
        ChainSpec {
            node_name: "arctis_clean_mic".into(),
            description: "Clean Mic".into(),
            channels: ChainChannels::Mono,
            kind: ChainKind::Source,
            capture_node_name: "arctis_clean_mic.capture".into(),
            capture_target: Some("alsa_input.hw_mic".into()),
            playback_target: None,
            playback_node_name: "arctis_clean_mic".into(),
        }
    }

    fn passthrough_nodes() -> Vec<FilterNode> {
        vec![FilterNode {
            name: "mic_gain".into(),
            node_type: NodeType::Builtin,
            label: "linear".into(),
            plugin: None,
            port_in: "In".into(),
            port_out: "Out".into(),
            controls: vec![("Mult".to_string(), 1.0), ("Add".to_string(), 0.0)],
        }]
    }

    #[test]
    fn render_chain_conf_passthrough_matches_fixture() {
        let got = render_chain_conf(&passthrough_spec(), &passthrough_nodes()).unwrap();
        let want = include_str!("../tests/fixtures/mic_source_passthrough.conf");
        if got != want {
            eprintln!("=== GOT ===\n{got}\n=== WANT ===\n{want}");
        }
        assert_eq!(got, want);
    }

    fn full_chain_nodes() -> Vec<FilterNode> {
        vec![
            FilterNode {
                name: "mic_gain".into(),
                node_type: NodeType::Builtin,
                label: "linear".into(),
                plugin: None,
                port_in: "In".into(),
                port_out: "Out".into(),
                controls: vec![("Mult".to_string(), 1.0), ("Add".to_string(), 0.0)],
            },
            FilterNode {
                name: "mic_highpass".into(),
                node_type: NodeType::Builtin,
                label: "bq_highpass".into(),
                plugin: None,
                port_in: "In".into(),
                port_out: "Out".into(),
                controls: vec![
                    ("Freq".to_string(), 90.0),
                    ("Q".to_string(), 0.7),
                    ("Gain".to_string(), 0.0),
                ],
            },
            FilterNode {
                name: "mic_suppression".into(),
                node_type: NodeType::Ladspa,
                label: "deep_filter_mono".into(),
                plugin: Some("libdeep_filter_ladspa".into()),
                port_in: "Audio In".into(),
                port_out: "Audio Out".into(),
                controls: vec![("Attenuation Limit (dB)".to_string(), 100.0)],
            },
            FilterNode {
                name: "mic_gate".into(),
                node_type: NodeType::Builtin,
                label: "noisegate".into(),
                plugin: None,
                port_in: "In".into(),
                port_out: "Out".into(),
                controls: vec![
                    ("Open Threshold".to_string(), 0.003),
                    ("Close Threshold".to_string(), 0.0027),
                    ("Attack (s)".to_string(), 0.005),
                    ("Hold (s)".to_string(), 0.05),
                    ("Release (s)".to_string(), 0.1),
                ],
            },
            FilterNode {
                name: "mic_eq_band_0".into(),
                node_type: NodeType::Builtin,
                label: "bq_lowshelf".into(),
                plugin: None,
                port_in: "In".into(),
                port_out: "Out".into(),
                controls: vec![
                    ("Freq".to_string(), 120.0),
                    ("Q".to_string(), 0.7),
                    ("Gain".to_string(), 3.0),
                ],
            },
        ]
    }

    #[test]
    fn render_chain_conf_full_matches_fixture() {
        let got = render_chain_conf(&passthrough_spec(), &full_chain_nodes()).unwrap();
        let want = include_str!("../tests/fixtures/mic_source_full.conf");
        if got != want {
            eprintln!("=== GOT ===\n{got}\n=== WANT ===\n{want}");
        }
        assert_eq!(got, want);
    }

    #[test]
    fn render_chain_conf_empty_nodes_errors() {
        let err = render_chain_conf(&passthrough_spec(), &[]).unwrap_err();
        assert!(matches!(err, AudioError::Invalid(_)));
    }

    /// Source chain: capture.props has node.passive = true (no media.class);
    /// playback.props has media.class = Audio/Source.
    #[test]
    fn render_chain_conf_source_has_correct_capture_playback_props() {
        let got = render_chain_conf(&passthrough_spec(), &passthrough_nodes()).unwrap();
        // capture.props: node.passive present, no media.class
        assert!(
            got.contains("node.passive = true"),
            "source capture.props must have node.passive = true"
        );
        // The media.class in capture.props must NOT be present
        let capture_section = got
            .split("capture.props")
            .nth(1)
            .and_then(|s| s.split("playback.props").next())
            .expect("capture.props section present");
        assert!(
            !capture_section.contains("media.class"),
            "source capture.props must NOT have media.class, got: {capture_section}"
        );
        // playback.props must have media.class = Audio/Source
        let playback_section = got
            .split("playback.props")
            .nth(1)
            .expect("playback.props section present");
        assert!(
            playback_section.contains("media.class = Audio/Source"),
            "source playback.props must have media.class = Audio/Source"
        );
    }

    /// Source chain uses LADSPA basename in plugin field (not absolute path).
    #[test]
    fn render_chain_conf_ladspa_emits_basename() {
        let got = render_chain_conf(&passthrough_spec(), &full_chain_nodes()).unwrap();
        assert!(got.contains("type = ladspa"));
        assert!(
            got.contains("plugin = \"libdeep_filter_ladspa\""),
            "plugin field must be basename, got: {got}"
        );
        // Must NOT contain an absolute path
        assert!(
            !got.contains("/usr/lib"),
            "plugin field must not be an absolute path"
        );
    }

    /// Source chain (unpinned hw mic) — capture.props must still have node.passive.
    #[test]
    fn render_chain_conf_source_unpinned_still_has_node_passive() {
        let spec = ChainSpec {
            node_name: "arctis_clean_mic".into(),
            description: "Clean Mic".into(),
            channels: ChainChannels::Mono,
            kind: ChainKind::Source,
            capture_node_name: "arctis_clean_mic.capture".into(),
            capture_target: None, // unpinned — no hw_mic set
            playback_target: None,
            playback_node_name: "arctis_clean_mic".into(),
        };
        let got = render_chain_conf(&spec, &passthrough_nodes()).unwrap();
        assert!(
            got.contains("node.passive = true"),
            "unpinned mic source must still have node.passive = true in capture.props"
        );
        assert!(
            !got.contains("stream.capture.sink"),
            "stream.capture.sink must not appear in any chain"
        );
    }

    /// EQ sink: capture.props has media.class = Audio/Sink (no node.passive);
    /// playback.props has node.passive = true.
    #[test]
    fn render_chain_conf_sink_has_correct_capture_playback_props() {
        use crate::eq::{BandKind, EqBand};
        let spec = SinkSpec {
            node_name: "arctis_eq".into(),
            description: "Arctis EQ Sink".into(),
            channels: ChainChannels::Stereo,
            playback_target: Some("alsa_output.hw0".into()),
        };
        let got = render_filter_chain_conf(
            &spec,
            &EqModel {
                bands: vec![EqBand::new(BandKind::Peaking, 1000.0, 1.0, 0.0)],
            },
        )
        .unwrap();
        // EQ sink capture.props: media.class = Audio/Sink, no node.passive
        let capture_section = got
            .split("capture.props")
            .nth(1)
            .and_then(|s| s.split("playback.props").next())
            .expect("capture.props section");
        assert!(
            capture_section.contains("media.class = Audio/Sink"),
            "sink capture.props must have media.class = Audio/Sink"
        );
        assert!(
            !capture_section.contains("node.passive"),
            "sink capture.props must NOT have node.passive"
        );
        // EQ sink playback.props: node.passive = true
        let playback_section = got
            .split("playback.props")
            .nth(1)
            .expect("playback.props section");
        assert!(
            playback_section.contains("node.passive = true"),
            "sink playback.props must have node.passive = true"
        );
        assert!(
            !got.contains("stream.capture.sink"),
            "EQ sink must not emit stream.capture.sink"
        );
    }
}
