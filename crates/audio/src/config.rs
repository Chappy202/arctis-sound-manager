use crate::eq::{EqModel, SAMPLE_RATE_HZ};
use crate::error::AudioError;

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

/// Format a value the way the conf expects: drop a trailing `.0`.
fn fmt_num(v: f32) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        // Trim to a stable short form (no scientific notation for our ranges).
        format!("{v}")
    }
}

/// Render the full `pipewire -c` conf for a filter-chain virtual EQ sink.
pub fn render_filter_chain_conf(spec: &SinkSpec, eq: &EqModel) -> Result<String, AudioError> {
    eq.validate()?;

    let rate = SAMPLE_RATE_HZ;
    let desc = &spec.description;
    let name = &spec.node_name;

    let mut nodes = String::new();
    for (i, b) in eq.bands.iter().enumerate() {
        let node_line = format!(
            "                    {{   type = builtin  name = \"{}\"  label = {}\n                        control = {{ \"Freq\" = {}  \"Q\" = {}  \"Gain\" = {} }}\n                    }}\n",
            band_node_name(i),
            b.kind.label(),
            fmt_num(b.freq_hz),
            fmt_num(b.q),
            fmt_num(b.gain_db),
        );
        nodes.push_str(&node_line);
    }

    let mut links = String::new();
    for i in 1..eq.bands.len() {
        let link_line = format!(
            "                    {{ output = \"{}:Out\"  input = \"{}:In\" }}\n",
            band_node_name(i - 1),
            band_node_name(i),
        );
        links.push_str(&link_line);
    }

    let first_in = format!("{}:In", band_node_name(0));
    let last_out = format!("{}:Out", band_node_name(eq.bands.len() - 1));

    let target_line = match &spec.playback_target {
        Some(t) => format!("                target.object = \"{t}\"\n"),
        None => String::new(),
    };

    let mut out = String::new();
    out.push_str("context.properties = {\n");
    out.push_str(&format!("    default.clock.rate = {rate}\n"));
    out.push_str(&format!("    default.clock.allowed-rates = [ {rate} ]\n"));
    out.push_str("}\n");
    out.push_str("context.modules = [\n");
    out.push_str("    {   name = libpipewire-module-filter-chain\n");
    out.push_str("        args = {\n");
    out.push_str(&format!("            node.description = \"{desc}\"\n"));
    out.push_str(&format!("            media.name       = \"{desc}\"\n"));
    out.push_str("            filter.graph = {\n");
    out.push_str("                nodes = [\n");
    out.push_str(&nodes);
    out.push_str("                ]\n");
    out.push_str("                links = [\n");
    out.push_str(&links);
    out.push_str("                ]\n");
    out.push_str(&format!("                inputs  = [ \"{first_in}\" ]\n"));
    out.push_str(&format!("                outputs = [ \"{last_out}\" ]\n"));
    out.push_str("            }\n");
    out.push_str(&format!("            audio.rate     = {rate}\n"));
    out.push_str("            audio.channels = 2\n");
    out.push_str("            audio.position = [ FL FR ]\n");
    out.push_str("            capture.props = {\n");
    out.push_str(&format!("                node.name   = \"{name}\"\n"));
    out.push_str("                media.class = Audio/Sink\n");
    out.push_str("            }\n");
    out.push_str("            playback.props = {\n");
    out.push_str(&format!(
        "                node.name    = \"{name}.output\"\n"
    ));
    out.push_str("                node.passive = true\n");
    out.push_str(&target_line);
    out.push_str("            }\n");
    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("]\n");

    Ok(out)
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
}
