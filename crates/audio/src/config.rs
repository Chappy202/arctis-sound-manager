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
    /// LADSPA plugin .so path; None for builtin.
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
}

impl ChainChannels {
    fn count(&self) -> u32 {
        match self {
            ChainChannels::Mono => 1,
            ChainChannels::Stereo => 2,
        }
    }
    fn position(&self) -> &'static str {
        match self {
            ChainChannels::Mono => "MONO",
            ChainChannels::Stereo => "FL FR",
        }
    }
}

/// Endpoint media classes + targets for a generic filter-chain instance.
#[derive(Debug, Clone, PartialEq)]
pub struct ChainSpec {
    pub node_name: String,
    pub description: String,
    pub channels: ChainChannels,
    /// capture.props media.class (e.g. "Audio/Sink" for an EQ sink,
    /// "Audio/Source" for the Clean Mic — capture side binds the hw mic).
    pub capture_media_class: String,
    /// The node.name used in capture.props. EQ sink uses the bare node_name;
    /// mic source uses "<node_name>.capture".
    pub capture_node_name: String,
    /// Optional capture target.object (the hw mic node.name for the mic source).
    pub capture_target: Option<String>,
    /// playback.props media.class (e.g. "Audio/Source" for the Clean Mic;
    /// for the EQ sink the playback tail is the hw sink output, class empty).
    pub playback_media_class: Option<String>,
    /// Optional playback target.object (the hw sink for an EQ sink).
    pub playback_target: Option<String>,
    /// Whether playback.props should carry node.passive = true (EQ sink tail).
    pub playback_passive: bool,
    /// The playback node.name; EQ sink uses "<name>.output", the mic
    /// source uses the bare "<name>" (the source IS the playback side).
    pub playback_node_name: String,
}

// ─── Identity + routing for one virtual EQ sink ─────────────────────────────

/// Identity + routing for one virtual EQ sink. `node_name` is stable so
/// create/remove are idempotent (G3).
#[derive(Debug, Clone)]
pub struct SinkSpec {
    pub node_name: String,
    pub description: String,
    /// `Some(hardware_sink_node_name)` to pin the tail; `None` follows default.
    pub playback_target: Option<String>,
}

/// Stable per-band node name. This is the addressing root the live-EQ Props
/// generator (Task 4) uses as `"<band_node_name>:Freq"` etc.
pub fn band_node_name(index: usize) -> String {
    format!("eq_band_{index}")
}

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
    let capture_name = &spec.capture_node_name;
    let capture_target_line = match &spec.capture_target {
        Some(t) => format!("                target.object = \"{t}\"\n"),
        None => String::new(),
    };

    // ── playback.props ───────────────────────────────────────────────────────
    let mut playback_inner = String::new();
    playback_inner.push_str(&format!(
        "                node.name   = \"{}\"\n",
        spec.playback_node_name
    ));
    if spec.playback_passive {
        playback_inner.push_str("                node.passive = true\n");
    }
    if let Some(ref mc) = spec.playback_media_class {
        playback_inner.push_str(&format!("                media.class = {mc}\n"));
    }
    if let Some(ref t) = spec.playback_target {
        playback_inner.push_str(&format!("                target.object = \"{t}\"\n"));
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
    out.push_str(&format!(
        "                node.name   = \"{capture_name}\"\n"
    ));
    out.push_str(&format!(
        "                media.class = {}\n",
        spec.capture_media_class
    ));
    out.push_str(&capture_target_line);
    // EQ sink: no stream.capture.sink line; mic source: added via capture_extra
    // (handled by caller by convention — the spec doesn't carry a free-form
    // extra line; the mic-source adapter adds it directly before calling us)
    // Actually: the mic source fixture has `stream.capture.sink = false` in
    // capture.props. We handle this via a dedicated field on ChainSpec — but
    // the plan's ChainSpec doesn't include it.  Looking at the fixture we need
    // it for the mic source. The plan says the mic source capture.props has
    // `stream.capture.sink = false`. We'll add it only when capture_media_class
    // is Audio/Source AND capture_target is Some (the mic source case).
    // NOTE: This is inferred from the fixture — capture.props for mic source
    // includes stream.capture.sink = false.
    if spec.capture_media_class == "Audio/Source" && spec.capture_target.is_some() {
        out.push_str("                stream.capture.sink = false\n");
    }
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

    // Build FilterNode list from EQ bands.
    let nodes: Vec<FilterNode> = eq
        .bands
        .iter()
        .enumerate()
        .map(|(i, b)| FilterNode {
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
        })
        .collect();

    let chain_spec = ChainSpec {
        node_name: spec.node_name.clone(),
        description: spec.description.clone(),
        channels: ChainChannels::Stereo,
        capture_media_class: "Audio/Sink".to_string(),
        capture_node_name: spec.node_name.clone(),
        capture_target: None,
        playback_media_class: None,
        playback_passive: true,
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
    fn renders_exact_fixture() {
        let spec = SinkSpec {
            node_name: "arctis_eq".into(),
            description: "Arctis EQ Sink".into(),
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

    // ── Mic source fixture tests ──────────────────────────────────────────────

    fn passthrough_spec() -> ChainSpec {
        ChainSpec {
            node_name: "arctis_clean_mic".into(),
            description: "Clean Mic".into(),
            channels: ChainChannels::Mono,
            capture_media_class: "Audio/Source".into(),
            capture_node_name: "arctis_clean_mic.capture".into(),
            capture_target: Some("alsa_input.hw_mic".into()),
            playback_media_class: Some("Audio/Source".into()),
            playback_passive: false,
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
                name: "mic_rnnoise".into(),
                node_type: NodeType::Ladspa,
                label: "noise_suppressor_mono".into(),
                plugin: Some("/usr/lib64/ladspa/librnnoise_ladspa.so".into()),
                port_in: "Input".into(),
                port_out: "Output".into(),
                controls: vec![
                    ("VAD Threshold (%)".to_string(), 40.0),
                    ("VAD Grace Period (ms)".to_string(), 800.0),
                    ("Retroactive VAD Grace (ms)".to_string(), 100.0),
                ],
            },
            FilterNode {
                name: "mic_gate".into(),
                node_type: NodeType::Builtin,
                label: "noisegate".into(),
                plugin: None,
                port_in: "In".into(),
                port_out: "Out".into(),
                controls: vec![
                    ("Threshold".to_string(), 0.003),
                    ("Attack".to_string(), 5.0),
                    ("Release".to_string(), 150.0),
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

    #[test]
    fn render_chain_conf_renders_ladspa_plugin_line() {
        let got = render_chain_conf(&passthrough_spec(), &full_chain_nodes()).unwrap();
        assert!(got.contains("type = ladspa"));
        assert!(got.contains("plugin = \"/usr/lib64/ladspa/librnnoise_ladspa.so\""));
    }
}
