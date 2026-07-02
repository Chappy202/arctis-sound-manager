use crate::backend::{parse_node_id, ConfHandle};
use crate::eq::EqModel;
use crate::error::AudioError;
use crate::runner::CommandRunner;
use std::path::Path;

// ─── SurroundSpec ─────────────────────────────────────────────────────────────

/// Identity for the virtual 7.1→binaural surround filter-chain instance.
///
/// Node names:
/// - capture (sink):  `effect_input.<node_name_base>`
/// - playback (tail): `effect_output.<node_name_base>`
#[derive(Debug, Clone, PartialEq)]
pub struct SurroundSpec {
    /// Base name, e.g. `"arctis_surround"`. Drives conf path and node names.
    pub node_name_base: String,
    /// Human-readable label for `node.description` / `media.name`.
    pub description: String,
    /// When `Some`, the playback tail is pinned to this hw sink via
    /// `target.object`. `None` means PipeWire follows the default sink.
    pub hw_sink: Option<String>,
}

impl SurroundSpec {
    /// The capture (sink) node name apps/channels route INTO. Public so the
    /// engine derives the surround route target from the spec (G4 — single
    /// source of truth, no duplicated string literals).
    pub fn capture_node_name(&self) -> String {
        format!("effect_input.{}", self.node_name_base)
    }

    pub fn playback_node_name(&self) -> String {
        format!("effect_output.{}", self.node_name_base)
    }
}

// ─── SurroundRender ───────────────────────────────────────────────────────────

/// Parameters for the extended surround conf renderer.
///
/// Pass to [`render_surround_conf_ex`] to render a filter-chain conf with
/// 5.1 (6-channel) or 7.1 (8-channel) input, and an optional per-ear
/// output EQ on the binaural tail.
pub struct SurroundRender<'a> {
    pub spec: &'a SurroundSpec,
    pub hrir_path: &'a Path,
    /// Number of input channels: `6` (5.1) or `8` (7.1).
    pub channels: u8,
    /// Optional EQ applied to the binaural (2-ch) output after convolution.
    /// When `Some`, per-ear bq nodes are inserted between the mixers and the
    /// 2-channel output.
    pub output_eq: Option<&'a EqModel>,
    /// Convolver partition size in samples. `Some(n)` emits `blocksize = n` into
    /// every convolver node's config; `None` omits it (PipeWire default).
    pub blocksize: Option<u32>,
    /// Per-convolver linear gain (`gain = g` config option, supported by the
    /// PipeWire convolver). Used to normalize each HRIR's insertion gain to a
    /// common target so switching HRIRs (or A/B-ing vs bypass) compares tone,
    /// not loudness. `None` omits it (unity).
    pub gain: Option<f32>,
    /// Convolver tail partition size in samples (`tailsize = n`). Pairs with
    /// `blocksize`: a small blocksize (low latency) with NO tailsize partitions
    /// the ENTIRE impulse response at that size — for the bundled 250 ms /
    /// 12000-sample IR that means ~188 tiny partitions and real xrun risk.
    /// `None` omits it (PipeWire default).
    pub tailsize: Option<u32>,
}

// ─── Conf renderer ────────────────────────────────────────────────────────────

/// Per-input gain baked into every binaural mixer (`mixL`/`mixR`) to prevent
/// the N-way convolver sum from clipping.
///
/// PipeWire's builtin `mixer` defaults every input to Gain=1.0.  With 8
/// convolvers feeding each ear (7.1) or 6 (5.1, but LFE still at In 8) at
/// unity, the summed binaural output routinely overshoots 0 dBFS by +6…+12 dB
/// → audible clipping.  −6 dB (×0.5) gives comfortable headroom; the level is
/// recovered at the master/hardware sink.
const SURROUND_MIX_HEADROOM_GAIN: f32 = 0.5;

/// Format a float value the way PipeWire filter-chain confs expect: always
/// emit a decimal point for whole numbers so PipeWire parses them as floats.
fn fmt_num(v: f32) -> String {
    if v.fract() == 0.0 {
        format!("{:.1}", v)
    } else {
        // Trim to a stable short form (no scientific notation for our ranges).
        format!("{v}")
    }
}

/// Build the `"Gain 1" = G  "Gain 2" = G  …  "Gain N" = G` string for a
/// PipeWire builtin `mixer` control block.
///
/// `n` is the number of consecutive input ports to attenuate (starting at 1).
/// Setting gain on an unconnected port is harmless — PipeWire ignores it.
fn mixer_gain_control(n: u8, gain: f32) -> String {
    let g = fmt_num(gain);
    (1..=n)
        .map(|i| format!("\"Gain {i}\" = {g}"))
        .collect::<Vec<_>>()
        .join("  ")
}

/// Render the full standalone `pipewire -c` conf for the surround filter-chain.
///
/// Extended version: supports 5.1 (6-channel) and 7.1 (8-channel) input,
/// and an optional per-ear output EQ on the binaural tail.
///
/// Graph topology:
/// - 6 or 8 `copy` nodes (input duplicators)
/// - 12 or 16 `convolver` nodes (HeSuVi HRIR channels)
/// - `mixL` / `mixR` mixer nodes
/// - *(optional)* per-ear `eq_l_{i}` / `eq_r_{i}` bq chains after the mixers
/// - 8-ch (7.1) or 6-ch (5.1) `capture.props` (Audio/Sink)
/// - 2-ch `playback.props` (`[ FL FR ]`)
///
/// Returns `AudioError::Invalid` if:
/// - `hrir_path` is empty
/// - `channels` is not 6 or 8
/// - `output_eq` is `Some` and fails validation
pub fn render_surround_conf_ex(r: &SurroundRender<'_>) -> Result<String, AudioError> {
    let hrir_str = r.hrir_path.to_str().unwrap_or("").trim();
    if hrir_str.is_empty() {
        return Err(AudioError::Invalid("hrir_path must not be empty".into()));
    }
    if r.channels != 6 && r.channels != 8 {
        return Err(AudioError::Invalid(format!(
            "channels must be 6 or 8, got {}",
            r.channels
        )));
    }
    if let Some(eq) = r.output_eq {
        eq.validate()?;
    }

    let is_51 = r.channels == 6;
    let rate = crate::eq::SAMPLE_RATE_HZ;
    let desc = &r.spec.description;
    let capture_node = r.spec.capture_node_name();
    let playback_node = r.spec.playback_node_name();

    // ── playback.props: optional target.object ────────────────────────────────
    let target_line = match &r.spec.hw_sink {
        Some(sink) => format!("                target.object = \"{sink}\"\n"),
        None => String::new(),
    };

    let mut out = String::new();

    // ── preamble (identical shape to EQ / mic confs) ─────────────────────────
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

    // ── nodes ─────────────────────────────────────────────────────────────────
    out.push_str("                nodes = [\n");

    // Copy nodes: FL FR FC RL RR [SL SR if 7.1] LFE
    out.push_str("                    { type = builtin  label = copy  name = copyFL  }\n");
    out.push_str("                    { type = builtin  label = copy  name = copyFR  }\n");
    out.push_str("                    { type = builtin  label = copy  name = copyFC  }\n");
    out.push_str("                    { type = builtin  label = copy  name = copyRL  }\n");
    out.push_str("                    { type = builtin  label = copy  name = copyRR  }\n");
    if !is_51 {
        out.push_str("                    { type = builtin  label = copy  name = copySL  }\n");
        out.push_str("                    { type = builtin  label = copy  name = copySR  }\n");
    }
    out.push_str("                    { type = builtin  label = copy  name = copyLFE }\n");

    // Convolver nodes (HeSuVi 14-channel WAV mapping).
    // 7.1: 16 nodes including SL/SR channels; 5.1: 12 nodes (SL/SR omitted).
    let convs_51: &[(&str, u32)] = &[
        ("convFL_L", 0),
        ("convFL_R", 1),
        ("convRL_L", 4),
        ("convRL_R", 5),
        ("convFC_L", 6),
        ("convFR_R", 7),
        ("convFR_L", 8),
        ("convRR_R", 11),
        ("convRR_L", 12),
        ("convFC_R", 13),
        // LFE treated as FC (channels 6 and 13)
        ("convLFE_L", 6),
        ("convLFE_R", 13),
    ];
    let convs_71: &[(&str, u32)] = &[
        ("convFL_L", 0),
        ("convFL_R", 1),
        ("convSL_L", 2),
        ("convSL_R", 3),
        ("convRL_L", 4),
        ("convRL_R", 5),
        ("convFC_L", 6),
        ("convFR_R", 7),
        ("convFR_L", 8),
        ("convSR_R", 9),
        ("convSR_L", 10),
        ("convRR_R", 11),
        ("convRR_L", 12),
        ("convFC_R", 13),
        // LFE treated as FC (channels 6 and 13)
        ("convLFE_L", 6),
        ("convLFE_R", 13),
    ];
    let convs: &[(&str, u32)] = if is_51 { convs_51 } else { convs_71 };
    for (name, ch) in convs {
        let g = match r.gain {
            Some(g) => format!("  gain = {}", fmt_num(g)),
            None => String::new(),
        };
        let bs = match r.blocksize {
            Some(n) => format!("  blocksize = {n}"),
            None => String::new(),
        };
        let ts = match r.tailsize {
            Some(n) => format!("  tailsize = {n}"),
            None => String::new(),
        };
        out.push_str(&format!(
            "                    {{ type = builtin  label = convolver  name = {name}  config = {{ filename = \"{hrir_str}\"  channel = {ch}{g}{bs}{ts} }} }}\n"
        ));
    }

    // Mixer nodes — each input attenuated by SURROUND_MIX_HEADROOM_GAIN to prevent
    // the N-way convolver sum from clipping.
    //
    // LFE is always routed to In 8 in both 7.1 and 5.1 configurations, so we must
    // emit gains for all 8 ports.  In 5.1, ports 2 and 6 are unconnected (no
    // SL/SR); setting their gain is harmless — PipeWire ignores disconnected inputs.
    let mg = mixer_gain_control(8, SURROUND_MIX_HEADROOM_GAIN);
    out.push_str(&format!(
        "                    {{ type = builtin  label = mixer  name = mixL  control = {{ {mg} }} }}\n"
    ));
    out.push_str(&format!(
        "                    {{ type = builtin  label = mixer  name = mixR  control = {{ {mg} }} }}\n"
    ));

    // Optional output EQ nodes: per-ear bq chains (L first, then R)
    if let Some(eq) = r.output_eq {
        for (i, band) in eq.bands.iter().enumerate() {
            let freq = fmt_num(band.freq_hz);
            let q = fmt_num(band.q);
            let gain = fmt_num(band.gain_db);
            out.push_str(&format!(
                "                    {{   type = builtin  name = \"eq_l_{i}\"  label = {}\n                        control = {{ \"Freq\" = {freq}  \"Q\" = {q}  \"Gain\" = {gain} }}\n                    }}\n",
                band.kind.label()
            ));
        }
        for (i, band) in eq.bands.iter().enumerate() {
            let freq = fmt_num(band.freq_hz);
            let q = fmt_num(band.q);
            let gain = fmt_num(band.gain_db);
            out.push_str(&format!(
                "                    {{   type = builtin  name = \"eq_r_{i}\"  label = {}\n                        control = {{ \"Freq\" = {freq}  \"Q\" = {q}  \"Gain\" = {gain} }}\n                    }}\n",
                band.kind.label()
            ));
        }
    }

    out.push_str("                ]\n");

    // ── links ─────────────────────────────────────────────────────────────────
    out.push_str("                links = [\n");

    // copy → conv fan-out
    // Verbatim order from shipped conf; SL/SR links omitted for 5.1.
    out.push_str("                    { output = \"copyFL:Out\"   input = \"convFL_L:In\"  }\n");
    out.push_str("                    { output = \"copyFL:Out\"   input = \"convFL_R:In\"  }\n");
    if !is_51 {
        out.push_str(
            "                    { output = \"copySL:Out\"   input = \"convSL_L:In\"  }\n",
        );
        out.push_str(
            "                    { output = \"copySL:Out\"   input = \"convSL_R:In\"  }\n",
        );
    }
    out.push_str("                    { output = \"copyRL:Out\"   input = \"convRL_L:In\"  }\n");
    out.push_str("                    { output = \"copyRL:Out\"   input = \"convRL_R:In\"  }\n");
    out.push_str("                    { output = \"copyFC:Out\"   input = \"convFC_L:In\"  }\n");
    out.push_str("                    { output = \"copyFR:Out\"   input = \"convFR_R:In\"  }\n");
    out.push_str("                    { output = \"copyFR:Out\"   input = \"convFR_L:In\"  }\n");
    if !is_51 {
        out.push_str(
            "                    { output = \"copySR:Out\"   input = \"convSR_R:In\"  }\n",
        );
        out.push_str(
            "                    { output = \"copySR:Out\"   input = \"convSR_L:In\"  }\n",
        );
    }
    out.push_str("                    { output = \"copyRR:Out\"   input = \"convRR_R:In\"  }\n");
    out.push_str("                    { output = \"copyRR:Out\"   input = \"convRR_L:In\"  }\n");
    out.push_str("                    { output = \"copyFC:Out\"   input = \"convFC_R:In\"  }\n");
    out.push_str("                    { output = \"copyLFE:Out\"  input = \"convLFE_L:In\" }\n");
    out.push_str("                    { output = \"copyLFE:Out\"  input = \"convLFE_R:In\" }\n");

    // conv → mixer fan-in
    // Verbatim order from shipped conf (including FL→mixL In5 asymmetry).
    // SL (In 2) and SR (In 6) links omitted for 5.1.
    out.push_str("                    { output = \"convFL_L:Out\"   input = \"mixL:In 1\" }\n");
    out.push_str("                    { output = \"convFL_R:Out\"   input = \"mixR:In 1\" }\n");
    if !is_51 {
        out.push_str(
            "                    { output = \"convSL_L:Out\"   input = \"mixL:In 2\" }\n",
        );
        out.push_str(
            "                    { output = \"convSL_R:Out\"   input = \"mixR:In 2\" }\n",
        );
    }
    out.push_str("                    { output = \"convRL_L:Out\"   input = \"mixL:In 3\" }\n");
    out.push_str("                    { output = \"convRL_R:Out\"   input = \"mixR:In 3\" }\n");
    out.push_str("                    { output = \"convFC_L:Out\"   input = \"mixL:In 4\" }\n");
    out.push_str("                    { output = \"convFC_R:Out\"   input = \"mixR:In 4\" }\n");
    out.push_str("                    { output = \"convFR_R:Out\"   input = \"mixR:In 5\" }\n");
    out.push_str("                    { output = \"convFR_L:Out\"   input = \"mixL:In 5\" }\n");
    if !is_51 {
        out.push_str(
            "                    { output = \"convSR_R:Out\"   input = \"mixR:In 6\" }\n",
        );
        out.push_str(
            "                    { output = \"convSR_L:Out\"   input = \"mixL:In 6\" }\n",
        );
    }
    out.push_str("                    { output = \"convRR_R:Out\"   input = \"mixR:In 7\" }\n");
    out.push_str("                    { output = \"convRR_L:Out\"   input = \"mixL:In 7\" }\n");
    out.push_str("                    { output = \"convLFE_R:Out\"  input = \"mixR:In 8\" }\n");
    out.push_str("                    { output = \"convLFE_L:Out\"  input = \"mixL:In 8\" }\n");

    // mixer → EQ tail links (present only when output_eq is Some)
    if let Some(eq) = r.output_eq {
        let n = eq.bands.len();
        // Left ear chain
        out.push_str("                    { output = \"mixL:Out\"  input = \"eq_l_0:In\" }\n");
        for i in 1..n {
            out.push_str(&format!(
                "                    {{ output = \"eq_l_{prev}:Out\"  input = \"eq_l_{i}:In\" }}\n",
                prev = i - 1
            ));
        }
        // Right ear chain
        out.push_str("                    { output = \"mixR:Out\"  input = \"eq_r_0:In\" }\n");
        for i in 1..n {
            out.push_str(&format!(
                "                    {{ output = \"eq_r_{prev}:Out\"  input = \"eq_r_{i}:In\" }}\n",
                prev = i - 1
            ));
        }
    }

    out.push_str("                ]\n");

    // inputs / outputs
    // 7.1: verbatim order from shipped conf, including the trailing commas
    // before copySL and copySR that appear in the reference file.
    if is_51 {
        out.push_str("                inputs  = [ \"copyFL:In\" \"copyFR:In\" \"copyFC:In\" \"copyLFE:In\" \"copyRL:In\" \"copyRR:In\" ]\n");
    } else {
        out.push_str("                inputs  = [ \"copyFL:In\" \"copyFR:In\" \"copyFC:In\" \"copyLFE:In\" \"copyRL:In\" \"copyRR:In\", \"copySL:In\", \"copySR:In\" ]\n");
    }
    match r.output_eq {
        Some(eq) => {
            let last = eq.bands.len() - 1;
            out.push_str(&format!(
                "                outputs = [ \"eq_l_{last}:Out\" \"eq_r_{last}:Out\" ]\n"
            ));
        }
        None => {
            out.push_str("                outputs = [ \"mixL:Out\" \"mixR:Out\" ]\n");
        }
    }

    out.push_str("            }\n");

    // ── capture.props (multi-ch sink input) ───────────────────────────────────
    out.push_str("            capture.props = {\n");
    out.push_str(&format!(
        "                node.name   = \"{capture_node}\"\n"
    ));
    out.push_str("                media.class = Audio/Sink\n");
    if is_51 {
        out.push_str("                audio.channels = 6\n");
        out.push_str("                audio.position = [ FL FR FC LFE RL RR ]\n");
    } else {
        out.push_str("                audio.channels = 8\n");
        out.push_str("                audio.position = [ FL FR FC LFE RL RR SL SR ]\n");
    }
    out.push_str("            }\n");

    // ── playback.props (2-ch binaural tail) ───────────────────────────────────
    out.push_str("            playback.props = {\n");
    out.push_str(&format!(
        "                node.name   = \"{playback_node}\"\n"
    ));
    out.push_str("                node.passive = true\n");
    out.push_str("                audio.channels = 2\n");
    out.push_str("                audio.position = [ FL FR ]\n");
    out.push_str(&target_line);
    out.push_str("            }\n");

    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("]\n");

    Ok(out)
}

/// Render the full standalone `pipewire -c` conf for a stereo-bypass
/// filter-chain sink.
///
/// This is the fallback used when a game outputs only stereo (no upmix through
/// the HRIR convolver). The graph passes stereo L/R through, optionally applies
/// crossfeed, and optionally applies a per-ear output EQ.
///
/// # Level matching (−6 dB pad)
///
/// The HRIR graph bakes `SURROUND_MIX_HEADROOM_GAIN` (−6 dB) into every mixer
/// input; without the same pad here, toggling HRIR ↔ bypass/crossfeed shifted
/// loudness by ~6 dB and biased every A/B comparison toward the louder path.
/// The bypass and crossfeed graphs therefore route through `mixer` nodes with
/// the SAME −6 dB per-input gain, so all three surround graphs meet at one
/// level. Surround-OFF (no filter-chain at all) remains unity — that residual
/// difference is acceptable and intentional: OFF means "untouched".
///
/// # Crossfeed
///
/// `crossfeed` is a percentage [0, 100]. When > 0, the mixers blend a fraction
/// of the opposite channel into each ear with gain
/// `cf = (crossfeed / 100) * 0.5 * SURROUND_MIX_HEADROOM_GAIN`, keeping the
/// direct:cross ratio of the pre-pad design (max crossfeed = half the direct
/// level).
///
/// The PipeWire builtin `mixer` supports per-input gain controls `"Gain 1"` …
/// `"Gain 8"` (verified: <https://docs.pipewire.org/page_module_filter_chain.html>).
///
/// # Output EQ
///
/// When `output_eq` is `Some`, per-ear `eq_l_{i}` / `eq_r_{i}` bq chains are
/// inserted between the (passthrough or crossfeed-mixed) L/R signals and the
/// 2-ch output, identical to the tail-EQ pattern in [`render_surround_conf_ex`].
///
/// Returns `AudioError::Invalid` if `output_eq` is `Some` and fails validation.
pub fn render_stereo_bypass_conf(
    spec: &SurroundSpec,
    crossfeed: u8,
    output_eq: Option<&EqModel>,
) -> Result<String, AudioError> {
    if let Some(eq) = output_eq {
        eq.validate()?;
    }

    if crossfeed > 100 {
        return Err(AudioError::Invalid(format!("crossfeed must be 0..=100, got {crossfeed}")));
    }

    let rate = crate::eq::SAMPLE_RATE_HZ;
    let desc = &spec.description;
    let capture_node = spec.capture_node_name();
    let playback_node = spec.playback_node_name();
    let has_crossfeed = crossfeed > 0;
    // −6 dB pad on the direct path (matches the HRIR graph's per-mixer-input
    // headroom gain); crossfeed keeps the same ratio relative to direct.
    let direct = SURROUND_MIX_HEADROOM_GAIN;
    let cf = (crossfeed as f32 / 100.0) * 0.5 * direct;

    let target_line = match &spec.hw_sink {
        Some(sink) => format!("                target.object = \"{sink}\"\n"),
        None => String::new(),
    };

    let mut out = String::new();

    // ── preamble (identical shape to EQ / mic / surround confs) ─────────────
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

    // ── nodes ─────────────────────────────────────────────────────────────────
    out.push_str("                nodes = [\n");

    // Stereo copy nodes (always present)
    out.push_str("                    { type = builtin  label = copy  name = copyL }\n");
    out.push_str("                    { type = builtin  label = copy  name = copyR }\n");

    // Mixer nodes (ALWAYS present): carry the −6 dB level-matching pad on the
    // direct input, plus the crossfeed blend on In 2 when requested.
    // PipeWire mixer builtin: "Gain 1".."Gain 8" set per-input gain.
    // copyL→mixL In 1 (gain 0.5), copyR→mixL In 2 (gain cf)
    // copyR→mixR In 1 (gain 0.5), copyL→mixR In 2 (gain cf)
    let direct_str = fmt_num(direct);
    let mix_ctl = if has_crossfeed {
        format!("\"Gain 1\" = {direct_str}  \"Gain 2\" = {}", fmt_num(cf))
    } else {
        format!("\"Gain 1\" = {direct_str}")
    };
    out.push_str(&format!(
        "                    {{ type = builtin  label = mixer  name = mixL  control = {{ {mix_ctl} }} }}\n"
    ));
    out.push_str(&format!(
        "                    {{ type = builtin  label = mixer  name = mixR  control = {{ {mix_ctl} }} }}\n"
    ));

    // Optional output EQ tail: per-ear bq chains (L first, then R).
    // Identical pattern to render_surround_conf_ex.
    if let Some(eq) = output_eq {
        for (i, band) in eq.bands.iter().enumerate() {
            let freq = fmt_num(band.freq_hz);
            let q = fmt_num(band.q);
            let gain = fmt_num(band.gain_db);
            out.push_str(&format!(
                "                    {{   type = builtin  name = \"eq_l_{i}\"  label = {}\n                        control = {{ \"Freq\" = {freq}  \"Q\" = {q}  \"Gain\" = {gain} }}\n                    }}\n",
                band.kind.label()
            ));
        }
        for (i, band) in eq.bands.iter().enumerate() {
            let freq = fmt_num(band.freq_hz);
            let q = fmt_num(band.q);
            let gain = fmt_num(band.gain_db);
            out.push_str(&format!(
                "                    {{   type = builtin  name = \"eq_r_{i}\"  label = {}\n                        control = {{ \"Freq\" = {freq}  \"Q\" = {q}  \"Gain\" = {gain} }}\n                    }}\n",
                band.kind.label()
            ));
        }
    }

    out.push_str("                ]\n");

    // ── links ─────────────────────────────────────────────────────────────────
    out.push_str("                links = [\n");

    // Direct routing through the padded mixers (always); cross links only
    // when crossfeed is requested.
    out.push_str("                    { output = \"copyL:Out\"  input = \"mixL:In 1\" }\n");
    out.push_str("                    { output = \"copyR:Out\"  input = \"mixR:In 1\" }\n");
    if has_crossfeed {
        out.push_str("                    { output = \"copyR:Out\"  input = \"mixL:In 2\" }\n");
        out.push_str("                    { output = \"copyL:Out\"  input = \"mixR:In 2\" }\n");
    }

    // EQ tail links: mixer → eq chain per ear
    let left_src = "mixL";
    let right_src = "mixR";

    if let Some(eq) = output_eq {
        let n = eq.bands.len();
        // Left ear chain
        out.push_str(&format!(
            "                    {{ output = \"{left_src}:Out\"  input = \"eq_l_0:In\" }}\n"
        ));
        for i in 1..n {
            out.push_str(&format!(
                "                    {{ output = \"eq_l_{prev}:Out\"  input = \"eq_l_{i}:In\" }}\n",
                prev = i - 1
            ));
        }
        // Right ear chain
        out.push_str(&format!(
            "                    {{ output = \"{right_src}:Out\"  input = \"eq_r_0:In\" }}\n"
        ));
        for i in 1..n {
            out.push_str(&format!(
                "                    {{ output = \"eq_r_{prev}:Out\"  input = \"eq_r_{i}:In\" }}\n",
                prev = i - 1
            ));
        }
    }

    out.push_str("                ]\n");

    // inputs / outputs
    out.push_str("                inputs  = [ \"copyL:In\" \"copyR:In\" ]\n");
    match output_eq {
        Some(eq) => {
            let last = eq.bands.len() - 1;
            out.push_str(&format!(
                "                outputs = [ \"eq_l_{last}:Out\" \"eq_r_{last}:Out\" ]\n"
            ));
        }
        None => {
            out.push_str("                outputs = [ \"mixL:Out\" \"mixR:Out\" ]\n");
        }
    }

    out.push_str("            }\n");

    // ── capture.props (2-ch stereo sink) ──────────────────────────────────────
    out.push_str("            capture.props = {\n");
    out.push_str(&format!(
        "                node.name   = \"{capture_node}\"\n"
    ));
    out.push_str("                media.class = Audio/Sink\n");
    out.push_str("                audio.channels = 2\n");
    out.push_str("                audio.position = [ FL FR ]\n");
    out.push_str("            }\n");

    // ── playback.props (2-ch stereo output) ───────────────────────────────────
    out.push_str("            playback.props = {\n");
    out.push_str(&format!(
        "                node.name   = \"{playback_node}\"\n"
    ));
    out.push_str("                node.passive = true\n");
    out.push_str("                audio.channels = 2\n");
    out.push_str("                audio.position = [ FL FR ]\n");
    out.push_str(&target_line);
    out.push_str("            }\n");

    out.push_str("        }\n");
    out.push_str("    }\n");
    out.push_str("]\n");

    Ok(out)
}

/// Render the full standalone `pipewire -c` conf for the 7.1→binaural HeSuVi
/// surround filter-chain.
///
/// The graph topology is reproduced verbatim from
/// `/usr/share/pipewire/filter-chain/sink-virtual-surround-7.1-hesuvi.conf`,
/// including the FL→convFL_L/convFL_R, FR→convFR_R/convFR_L asymmetry and the
/// `mixL/mixR:In 1..8` numbering. Our node names replace the shipped ones.
///
/// Unlike the EQ/mic renderers the surround graph is a DAG (fan-out / fan-in),
/// so we do NOT route through `render_chain_conf` / `FilterNode` — this
/// function owns the full template.
///
/// Prepends the same standalone preamble (`context.properties`,
/// `context.spa-libs`, `context.modules` support set) that our EQ and mic
/// confs emit, so the conf can be launched with `pipewire -c <path>`.
///
/// Returns `AudioError::Invalid` if `hrir_path` is empty.
///
/// This is a thin wrapper over [`render_surround_conf_ex`] with 8-channel
/// input and no output EQ. All existing callers remain unchanged.
pub fn render_surround_conf(spec: &SurroundSpec, hrir_path: &Path) -> Result<String, AudioError> {
    render_surround_conf_ex(&SurroundRender {
        spec,
        hrir_path,
        channels: 8,
        output_eq: None,
        blocksize: None,
        gain: None,
        tailsize: None,
    })
}

// ─── SurroundBackend ──────────────────────────────────────────────────────────

/// Backend for the virtual 7.1→binaural surround sink.
/// Mirrors `MicBackend` in structure and lifecycle (idempotent create/remove/recreate).
pub struct SurroundBackend<R: CommandRunner> {
    runner: R,
    spec: SurroundSpec,
}

impl<R: CommandRunner> SurroundBackend<R> {
    pub fn new(runner: R, spec: SurroundSpec) -> Self {
        Self { runner, spec }
    }

    /// Expose the runner for assertions in tests.
    #[cfg(test)]
    pub fn runner(&self) -> &R {
        &self.runner
    }

    fn check(out: crate::runner::CmdOutput, program: &str) -> Result<(), AudioError> {
        if out.status == 0 {
            Ok(())
        } else {
            Err(AudioError::NonZeroExit {
                program: program.to_string(),
                status: out.status,
                stderr: out.stderr,
            })
        }
    }

    /// Path to the on-disk conf file: `/tmp/arctis_<base>.conf`.
    fn conf_path(&self) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("arctis_{}.conf", self.spec.node_name_base));
        p
    }

    /// True if the surround capture (sink) node is already present in PipeWire.
    pub fn source_exists(&mut self) -> Result<bool, AudioError> {
        let out = self.runner.run("pw-cli", &["ls", "Node"])?;
        if out.status != 0 {
            return Err(AudioError::NonZeroExit {
                program: "pw-cli".to_string(),
                status: out.status,
                stderr: out.stderr,
            });
        }
        Ok(out.stdout.contains(&format!(
            "node.name = \"{}\"",
            self.spec.capture_node_name()
        )))
    }

    /// Write `conf` to the on-disk conf file and spawn `pipewire -c <path>`.
    ///
    /// Shared write+spawn logic extracted to satisfy G1 (no duplication).
    fn spawn_conf(&mut self, conf: String) -> Result<ConfHandle, AudioError> {
        let path = self.conf_path();
        std::fs::write(&path, conf).map_err(|e| AudioError::Spawn {
            program: "write-conf".to_string(),
            source_msg: e.to_string(),
        })?;
        let path_str = path.to_string_lossy().into_owned();
        let token = self.runner.spawn_owned("pipewire", &["-c", &path_str])?;
        Ok(ConfHandle {
            conf_path: path,
            child: Some(token),
        })
    }

    /// Create the surround sink idempotently.
    ///
    /// Returns a `ConfHandle` with `child = Some(token)` when a new `pipewire -c`
    /// was spawned; `child = None` when the sink was already present.
    pub fn create(&mut self, hrir_path: &Path) -> Result<ConfHandle, AudioError> {
        let path = self.conf_path();
        if self.source_exists()? {
            return Ok(ConfHandle {
                conf_path: path,
                child: None,
            });
        }
        let conf = render_surround_conf(&self.spec, hrir_path)?;
        self.spawn_conf(conf)
    }

    /// Remove the surround sink idempotently.
    pub fn remove(&mut self) -> Result<(), AudioError> {
        if !self.source_exists()? {
            // Best-effort stale-conf cleanup; ignore errors.
            let _ = std::fs::remove_file(self.conf_path());
            return Ok(());
        }
        let id = self.find_node_id()?;
        let out = self.runner.run("pw-cli", &["destroy", &id])?;
        Self::check(out, "pw-cli")?;
        let conf_path_str = self.conf_path().to_string_lossy().into_owned();
        let _ = self.runner.run("pkill", &["-f", &conf_path_str]);
        let _ = std::fs::remove_file(self.conf_path());
        Ok(())
    }

    /// Teardown then recreate (HRIR or topology change requires a full respawn).
    pub fn recreate(&mut self, hrir_path: &Path) -> Result<ConfHandle, AudioError> {
        self.remove()?;
        self.create(hrir_path)
    }

    /// Diff-before-recreate: if `conf` is byte-identical to what's already on
    /// disk AND the node is live, skip the teardown+respawn entirely — a
    /// respawn is an audible dropout plus a fallback window where streams play
    /// unconvolved. Otherwise tear down and spawn fresh.
    fn recreate_conf(&mut self, conf: String) -> Result<ConfHandle, AudioError> {
        let path = self.conf_path();
        let unchanged = std::fs::read_to_string(&path)
            .map(|existing| existing == conf)
            .unwrap_or(false);
        if unchanged && self.source_exists()? {
            return Ok(ConfHandle {
                conf_path: path,
                child: None,
            });
        }
        self.remove()?;
        self.spawn_conf(conf)
    }

    /// Recreate with explicit channel count and optional output EQ.
    ///
    /// `channels` must be `6` (5.1) or `8` (7.1); see [`render_surround_conf_ex`].
    /// Renders first and skips the teardown+respawn when nothing changed (see
    /// [`Self::recreate_conf`]).
    #[allow(clippy::too_many_arguments)]
    pub fn recreate_ex(
        &mut self,
        hrir_path: &Path,
        channels: u8,
        output_eq: Option<&EqModel>,
        blocksize: Option<u32>,
        gain: Option<f32>,
        tailsize: Option<u32>,
    ) -> Result<ConfHandle, AudioError> {
        let conf = render_surround_conf_ex(&SurroundRender {
            spec: &self.spec,
            hrir_path,
            channels,
            output_eq,
            blocksize,
            gain,
            tailsize,
        })?;
        self.recreate_conf(conf)
    }

    /// Recreate as a stereo-bypass sink (no HRIR convolver).
    ///
    /// `crossfeed` is [0, 100]; see [`render_stereo_bypass_conf`].
    /// Renders first and skips the teardown+respawn when nothing changed.
    pub fn recreate_stereo_bypass(
        &mut self,
        crossfeed: u8,
        output_eq: Option<&EqModel>,
    ) -> Result<ConfHandle, AudioError> {
        let conf = render_stereo_bypass_conf(&self.spec, crossfeed, output_eq)?;
        self.recreate_conf(conf)
    }

    /// Resolve the filter-chain node id for the capture (sink) node.
    fn find_node_id(&mut self) -> Result<String, AudioError> {
        let out = self.runner.run("pw-cli", &["ls", "Node"])?;
        if out.status != 0 {
            return Err(AudioError::NonZeroExit {
                program: "pw-cli".to_string(),
                status: out.status,
                stderr: out.stderr,
            });
        }
        parse_node_id(&out.stdout, &self.spec.capture_node_name())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::{BandKind, EqBand, EqModel};
    use crate::runner::MockRunner;
    use std::path::PathBuf;

    fn test_spec() -> SurroundSpec {
        SurroundSpec {
            node_name_base: "arctis_surround".into(),
            description: "Arctis Surround Sink".into(),
            hw_sink: Some("hwsink".into()),
        }
    }

    // ── pw-cli ls Node fixtures ───────────────────────────────────────────────

    /// ls Node output that includes the surround capture node.
    const LS_WITH_SURROUND: &str = "\
id 40, type PipeWire:Interface:Node/3
    node.name = \"alsa_output.pci\"
id 81, type PipeWire:Interface:Node/3
    node.name = \"effect_input.arctis_surround\"
id 82, type PipeWire:Interface:Node/3
    node.name = \"effect_output.arctis_surround\"
";

    /// ls Node output that does NOT contain the surround sink.
    const LS_WITHOUT_SURROUND: &str = "\
id 40, type PipeWire:Interface:Node/3
    node.name = \"alsa_output.pci\"
";

    // ── render_surround_conf tests ────────────────────────────────────────────

    #[test]
    fn render_surround_conf_matches_fixture() {
        let spec = test_spec();
        let got = render_surround_conf(&spec, &PathBuf::from("/test/hrir.wav")).unwrap();
        let want = include_str!("../tests/fixtures/surround_7_1_hesuvi.conf");
        if got != want {
            eprintln!("=== GOT ===\n{got}\n=== WANT ===\n{want}");
            // Print first differing line for easier debugging.
            for (i, (g, w)) in got.lines().zip(want.lines()).enumerate() {
                if g != w {
                    eprintln!("  First diff at line {}: GOT={g:?} WANT={w:?}", i + 1);
                    break;
                }
            }
        }
        assert_eq!(got, want);
    }

    #[test]
    fn render_surround_conf_contains_all_16_convolver_channels() {
        let spec = test_spec();
        let got = render_surround_conf(&spec, &PathBuf::from("/test/hrir.wav")).unwrap();

        // All 16 convolver config lines must reference the test HRIR path.
        let conv_lines: Vec<&str> = got
            .lines()
            .filter(|l| l.contains("label = convolver"))
            .collect();
        assert_eq!(conv_lines.len(), 16, "must have 16 convolver nodes");

        for line in &conv_lines {
            assert!(
                line.contains("filename = \"/test/hrir.wav\""),
                "convolver line missing hrir path: {line}"
            );
        }

        // Channels 0..13 are all present (LFE uses 6 and 13 again)
        for ch in 0u32..=13 {
            assert!(
                got.contains(&format!("channel = {ch} }}")),
                "missing channel = {ch}"
            );
        }
    }

    #[test]
    fn render_surround_conf_capture_props() {
        let spec = test_spec();
        let got = render_surround_conf(&spec, &PathBuf::from("/test/hrir.wav")).unwrap();

        // capture.props: 8-ch Audio/Sink with correct position
        let cap = got
            .split("capture.props")
            .nth(1)
            .and_then(|s| s.split("playback.props").next())
            .expect("capture.props section");
        assert!(cap.contains("node.name   = \"effect_input.arctis_surround\""));
        assert!(cap.contains("media.class = Audio/Sink"));
        assert!(cap.contains("audio.channels = 8"));
        assert!(cap.contains("audio.position = [ FL FR FC LFE RL RR SL SR ]"));
        // No node.passive in capture.props for a sink
        assert!(!cap.contains("node.passive"));
    }

    #[test]
    fn render_surround_conf_playback_props() {
        let spec = test_spec();
        let got = render_surround_conf(&spec, &PathBuf::from("/test/hrir.wav")).unwrap();

        let pb = got
            .split("playback.props")
            .nth(1)
            .expect("playback.props section");
        assert!(pb.contains("node.name   = \"effect_output.arctis_surround\""));
        assert!(pb.contains("node.passive = true"));
        assert!(pb.contains("audio.channels = 2"));
        assert!(pb.contains("audio.position = [ FL FR ]"));
        assert!(pb.contains("target.object = \"hwsink\""));
    }

    #[test]
    fn render_surround_conf_no_hw_sink_omits_target() {
        let spec = SurroundSpec {
            node_name_base: "arctis_surround".into(),
            description: "Arctis Surround Sink".into(),
            hw_sink: None,
        };
        let got = render_surround_conf(&spec, &PathBuf::from("/test/hrir.wav")).unwrap();
        assert!(!got.contains("target.object"));
    }

    #[test]
    fn render_surround_conf_rejects_empty_hrir_path() {
        let spec = test_spec();
        let err = render_surround_conf(&spec, Path::new("")).unwrap_err();
        assert!(
            matches!(err, AudioError::Invalid(_)),
            "expected Invalid, got {err:?}"
        );
    }

    // ── render_surround_conf_ex: new parametric renderer tests ────────────────

    #[test]
    fn render_surround_conf_ex_rejects_invalid_channels() {
        let spec = test_spec();
        let err = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &PathBuf::from("/test/hrir.wav"),
            channels: 7,
            output_eq: None,
            blocksize: None,
            gain: None,
            tailsize: None,
        })
        .unwrap_err();
        assert!(
            matches!(err, AudioError::Invalid(_)),
            "expected Invalid for channels=7, got {err:?}"
        );
    }

    #[test]
    fn render_surround_conf_ex_rejects_invalid_eq() {
        let spec = test_spec();
        // EqModel with no bands fails validation.
        let eq = EqModel { bands: vec![] };
        let err = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &PathBuf::from("/test/hrir.wav"),
            channels: 8,
            output_eq: Some(&eq),
            blocksize: None,
            gain: None,
            tailsize: None,
        })
        .unwrap_err();
        assert!(
            matches!(err, AudioError::Invalid(_)),
            "expected Invalid for empty eq, got {err:?}"
        );
    }

    /// 6-channel render must emit 6-ch capture props and omit all SL/SR nodes.
    #[test]
    fn render_5_1_emits_6_channel_capture_and_no_side_nodes() {
        let spec = test_spec();
        let got = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &PathBuf::from("/test/hrir.wav"),
            channels: 6,
            output_eq: None,
            blocksize: None,
            gain: None,
            tailsize: None,
        })
        .unwrap();

        // capture.props must be 6-ch with 5.1 position
        let cap = got
            .split("capture.props")
            .nth(1)
            .and_then(|s| s.split("playback.props").next())
            .expect("capture.props section");
        assert!(
            cap.contains("audio.channels = 6"),
            "5.1 must have 6 channels in capture.props"
        );
        assert!(
            cap.contains("audio.position = [ FL FR FC LFE RL RR ]"),
            "5.1 must use 5.1 position"
        );
        assert!(
            !cap.contains("SL"),
            "5.1 capture.props must not reference SL"
        );
        assert!(
            !cap.contains("SR"),
            "5.1 capture.props must not reference SR"
        );

        // No SL/SR copy or convolver nodes
        assert!(
            !got.contains("copySL"),
            "5.1 must not emit copySL node"
        );
        assert!(
            !got.contains("copySR"),
            "5.1 must not emit copySR node"
        );
        assert!(
            !got.contains("convSL"),
            "5.1 must not emit any convSL node"
        );
        assert!(
            !got.contains("convSR"),
            "5.1 must not emit any convSR node"
        );

        // Must still have 12 convolver nodes (not 16)
        let conv_count = got.lines().filter(|l| l.contains("label = convolver")).count();
        assert_eq!(conv_count, 12, "5.1 must have 12 convolver nodes, got {conv_count}");

        // Outputs still come from mixers (no EQ tail)
        assert!(
            got.contains("outputs = [ \"mixL:Out\" \"mixR:Out\" ]"),
            "5.1 no-EQ outputs must be from mixers"
        );
    }

    #[test]
    fn render_with_blocksize_emits_blocksize_on_every_convolver() {
        let spec = test_spec();
        let got = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &PathBuf::from("/test/hrir.wav"),
            channels: 8,
            output_eq: None,
            blocksize: Some(128),
            gain: None,
            tailsize: None,
        })
        .unwrap();
        let conv_lines: Vec<&str> = got.lines().filter(|l| l.contains("label = convolver")).collect();
        assert_eq!(conv_lines.len(), 16, "7.1 has 16 convolvers");
        for line in &conv_lines {
            assert!(line.contains("blocksize = 128"), "convolver line missing blocksize: {line}");
        }
    }

    #[test]
    fn render_with_gain_emits_gain_on_every_convolver() {
        // HRIR insertion-gain normalization: `gain = g` on every convolver so
        // every HRIR meets the same target level (locks the rendered value).
        let spec = test_spec();
        let got = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &PathBuf::from("/test/hrir.wav"),
            channels: 8,
            output_eq: None,
            blocksize: None,
            gain: Some(1.25),
            tailsize: None,
        })
        .unwrap();
        let conv_lines: Vec<&str> = got.lines().filter(|l| l.contains("label = convolver")).collect();
        assert_eq!(conv_lines.len(), 16);
        for line in &conv_lines {
            assert!(line.contains("gain = 1.25"), "convolver line missing gain: {line}");
        }
    }

    #[test]
    fn render_without_gain_omits_gain_key() {
        let spec = test_spec();
        let got = render_surround_conf(&test_spec(), &PathBuf::from("/test/hrir.wav")).unwrap();
        assert!(!got.contains("gain ="), "None must not emit a gain key");
        let _ = spec;
    }

    #[test]
    fn render_without_blocksize_omits_blocksize() {
        let spec = test_spec();
        let got = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &PathBuf::from("/test/hrir.wav"),
            channels: 8,
            output_eq: None,
            blocksize: None,
            gain: None,
            tailsize: None,
        })
        .unwrap();
        assert!(!got.contains("blocksize"), "None must not emit a blocksize key");
    }

    /// 8-channel render with a 2-band EqModel must insert per-ear bq nodes
    /// linked after the mixers, with the graph output coming from the last EQ nodes.
    /// STRENGTHENED: asserts the actual control value formatting, not just node presence.
    #[test]
    fn render_with_output_eq_inserts_per_ear_bq_nodes_after_mixers() {
        let spec = test_spec();
        let eq = EqModel {
            bands: vec![
                EqBand::new(BandKind::Peaking, 200.0, 1.0, 3.0),
                EqBand::new(BandKind::HighShelf, 8000.0, 0.7, -2.0),
            ],
        };
        let got = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &PathBuf::from("/test/hrir.wav"),
            channels: 8,
            output_eq: Some(&eq),
            blocksize: None,
            gain: None,
            tailsize: None,
        })
        .unwrap();

        // Both band types must appear
        assert!(got.contains("bq_peaking"), "must contain bq_peaking node");
        assert!(got.contains("bq_highshelf"), "must contain bq_highshelf node");

        // Per-ear node names must be present
        assert!(got.contains("\"eq_l_0\""), "must have eq_l_0 node");
        assert!(got.contains("\"eq_l_1\""), "must have eq_l_1 node");
        assert!(got.contains("\"eq_r_0\""), "must have eq_r_0 node");
        assert!(got.contains("\"eq_r_1\""), "must have eq_r_1 node");

        // STRENGTHENED: Assert actual control value formatting for peaking band (eq_l_0).
        // fmt_num formats: whole numbers with .1 (200.0), decimals as-is (1.0, 3.0).
        assert!(
            got.contains("\"Freq\" = 200.0  \"Q\" = 1.0  \"Gain\" = 3.0"),
            "eq_l_0 must have exact peaking band controls: Freq=200.0, Q=1.0, Gain=3.0"
        );
        assert!(
            got.contains("\"Freq\" = 8000.0  \"Q\" = 0.7  \"Gain\" = -2.0"),
            "eq_l_1 must have exact highshelf band controls: Freq=8000.0, Q=0.7, Gain=-2.0"
        );

        // Mixer → EQ links must be present
        assert!(
            got.contains("output = \"mixL:Out\"  input = \"eq_l_0:In\""),
            "mixL must link to eq_l_0"
        );
        assert!(
            got.contains("output = \"mixR:Out\"  input = \"eq_r_0:In\""),
            "mixR must link to eq_r_0"
        );

        // Intra-EQ chain links
        assert!(
            got.contains("output = \"eq_l_0:Out\"  input = \"eq_l_1:In\""),
            "eq_l_0 must link to eq_l_1"
        );
        assert!(
            got.contains("output = \"eq_r_0:Out\"  input = \"eq_r_1:In\""),
            "eq_r_0 must link to eq_r_1"
        );

        // Outputs must come from the last EQ nodes, not directly from mixers
        assert!(
            got.contains("outputs = [ \"eq_l_1:Out\" \"eq_r_1:Out\" ]"),
            "outputs must reference last EQ nodes"
        );
        assert!(
            !got.contains("outputs = [ \"mixL:Out\" \"mixR:Out\" ]"),
            "outputs must NOT come directly from mixers when EQ is present"
        );

        // Still 7.1: 8-ch capture, 16 convolvers
        assert!(got.contains("audio.channels = 8"));
        let conv_count = got.lines().filter(|l| l.contains("label = convolver")).count();
        assert_eq!(conv_count, 16, "7.1 with EQ must still have 16 convolver nodes");
    }

    /// 5.1 (6-channel) with output EQ must combine both features:
    /// 6-ch capture.props AND per-ear EQ nodes after mixers with EQ outputs.
    #[test]
    fn render_5_1_with_output_eq_combines_both() {
        let spec = test_spec();
        let eq = EqModel {
            bands: vec![
                EqBand::new(BandKind::Peaking, 1000.0, 1.5, 2.0),
                EqBand::new(BandKind::LowShelf, 100.0, 0.9, -1.5),
            ],
        };
        let got = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &PathBuf::from("/test/hrir.wav"),
            channels: 6,
            output_eq: Some(&eq),
            blocksize: None,
            gain: None,
            tailsize: None,
        })
        .unwrap();

        // 5.1: capture must be 6-ch
        let cap = got
            .split("capture.props")
            .nth(1)
            .and_then(|s| s.split("playback.props").next())
            .expect("capture.props section");
        assert!(
            cap.contains("audio.channels = 6"),
            "5.1 with EQ must emit 6-ch capture"
        );
        assert!(
            cap.contains("audio.position = [ FL FR FC LFE RL RR ]"),
            "5.1 with EQ must use 5.1 position"
        );

        // No SL/SR nodes in 5.1
        assert!(
            !got.contains("copySL"),
            "5.1 with EQ must not emit copySL"
        );
        assert!(
            !got.contains("convSL"),
            "5.1 with EQ must not emit convSL"
        );

        // EQ nodes present: 2-band chain per ear
        assert!(got.contains("\"eq_l_0\""), "must have eq_l_0");
        assert!(got.contains("\"eq_l_1\""), "must have eq_l_1");
        assert!(got.contains("\"eq_r_0\""), "must have eq_r_0");
        assert!(got.contains("\"eq_r_1\""), "must have eq_r_1");

        // Mixer → EQ_0 links
        assert!(
            got.contains("output = \"mixL:Out\"  input = \"eq_l_0:In\""),
            "mixL must link to eq_l_0"
        );
        assert!(
            got.contains("output = \"mixR:Out\"  input = \"eq_r_0:In\""),
            "mixR must link to eq_r_0"
        );

        // Outputs reference last EQ nodes
        assert!(
            got.contains("outputs = [ \"eq_l_1:Out\" \"eq_r_1:Out\" ]"),
            "5.1 with EQ outputs must reference eq_l_1/eq_r_1"
        );

        // Only 12 convolvers for 5.1
        let conv_count = got.lines().filter(|l| l.contains("label = convolver")).count();
        assert_eq!(conv_count, 12, "5.1 with EQ must have 12 convolver nodes");
    }

    /// mixL and mixR must carry a control block with SURROUND_MIX_HEADROOM_GAIN (0.5)
    /// on every input port, for both 7.1 and 5.1 configurations.
    ///
    /// Root cause of clipping (now fixed): PipeWire builtin mixer defaults every
    /// input to Gain=1.0.  With 8 convolvers feeding each ear's mixer in 7.1 (or
    /// 6 in 5.1, still with LFE at In 8), the summed output routinely overshoots
    /// 0 dBFS by +6…+12 dB.  Fix: bake -6 dB (gain=0.5) into each mixer input.
    ///
    /// For 5.1, convolvers connect at ports 1,3,4,5,7,8 (LFE always at In 8).
    /// We emit gains for all 8 ports; the gain on unconnected ports 2 and 6
    /// is applied to disconnected inputs and is therefore harmless.
    #[test]
    fn surround_mixer_has_headroom_gains() {
        let spec = test_spec();
        let path = PathBuf::from("/test/hrir.wav");

        // Build the expected gains string: "Gain 1" = 0.5 … "Gain 8" = 0.5
        let gain_str = (1u8..=8)
            .map(|i| format!("\"Gain {i}\" = 0.5"))
            .collect::<Vec<_>>()
            .join("  ");

        // --- 7.1 (8 channels) ---
        let got_71 = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &path,
            channels: 8,
            output_eq: None,
            blocksize: None,
            gain: None,
            tailsize: None,
        })
        .unwrap();

        assert!(
            got_71.contains(&format!("name = mixL  control = {{ {gain_str} }}")),
            "7.1 mixL must carry 8-entry headroom gains; got:\n{got_71}"
        );
        assert!(
            got_71.contains(&format!("name = mixR  control = {{ {gain_str} }}")),
            "7.1 mixR must carry 8-entry headroom gains"
        );

        // --- 5.1 (6 channels) ---
        // 5.1 connects to ports 1,3,4,5,7,8 (max port = 8 via LFE).
        // We still emit 8 gain entries; unconnected ports 2 and 6 are harmless.
        let got_51 = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &path,
            channels: 6,
            output_eq: None,
            blocksize: None,
            gain: None,
            tailsize: None,
        })
        .unwrap();

        assert!(
            got_51.contains(&format!("name = mixL  control = {{ {gain_str} }}")),
            "5.1 mixL must carry 8-entry headroom gains (LFE at In 8)"
        );
        assert!(
            got_51.contains(&format!("name = mixR  control = {{ {gain_str} }}")),
            "5.1 mixR must carry 8-entry headroom gains (LFE at In 8)"
        );
    }

    /// Render with a 1-band EqModel must create a degenerate chain (no internal links).
    /// outputs must reference eq_l_0:Out / eq_r_0:Out (no eq_l_1, etc.).
    #[test]
    fn render_with_single_band_eq_degenerate_chain() {
        let spec = test_spec();
        let eq = EqModel {
            bands: vec![EqBand::new(BandKind::Peaking, 500.0, 1.0, 0.0)],
        };
        let got = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &PathBuf::from("/test/hrir.wav"),
            channels: 8,
            output_eq: Some(&eq),
            blocksize: None,
            gain: None,
            tailsize: None,
        })
        .unwrap();

        // Only eq_l_0 and eq_r_0 nodes exist (no eq_l_1, eq_r_1, etc.)
        assert!(got.contains("\"eq_l_0\""), "must have eq_l_0 node");
        assert!(got.contains("\"eq_r_0\""), "must have eq_r_0 node");
        assert!(
            !got.contains("\"eq_l_1\""),
            "degenerate 1-band chain must not have eq_l_1"
        );
        assert!(
            !got.contains("\"eq_r_1\""),
            "degenerate 1-band chain must not have eq_r_1"
        );

        // Mixer directly links to the single EQ node
        assert!(
            got.contains("output = \"mixL:Out\"  input = \"eq_l_0:In\""),
            "mixL must link to eq_l_0 in degenerate chain"
        );
        assert!(
            got.contains("output = \"mixR:Out\"  input = \"eq_r_0:In\""),
            "mixR must link to eq_r_0 in degenerate chain"
        );

        // No intra-EQ links (no eq_l_0 → eq_l_1)
        assert!(
            !got.contains("output = \"eq_l_0:Out\"  input = \"eq_l_1:In\""),
            "degenerate chain must not have internal EQ links"
        );

        // Outputs reference the sole EQ nodes
        assert!(
            got.contains("outputs = [ \"eq_l_0:Out\" \"eq_r_0:Out\" ]"),
            "degenerate 1-band outputs must reference eq_l_0/eq_r_0"
        );

        // Control values present and correct (0.0 dB formatted as "0.0")
        assert!(
            got.contains("\"Freq\" = 500.0  \"Q\" = 1.0  \"Gain\" = 0.0"),
            "single band must have exact control values: Freq=500.0, Q=1.0, Gain=0.0"
        );

        // Still 7.1: 8-ch capture, 16 convolvers
        assert!(got.contains("audio.channels = 8"));
        let conv_count = got.lines().filter(|l| l.contains("label = convolver")).count();
        assert_eq!(conv_count, 16, "7.1 with 1-band EQ must still have 16 convolver nodes");
    }

    /// render_surround_conf must produce identical output to render_surround_conf_ex
    /// with channels=8 and output_eq=None (back-compat wrapper guarantee).
    #[test]
    fn render_surround_conf_wrapper_unchanged_vs_ex() {
        let spec = test_spec();
        let path = PathBuf::from("/test/hrir.wav");
        let via_wrapper = render_surround_conf(&spec, &path).unwrap();
        let via_ex = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &path,
            channels: 8,
            output_eq: None,
            blocksize: None,
            gain: None,
            tailsize: None,
        })
        .unwrap();
        assert_eq!(
            via_wrapper, via_ex,
            "render_surround_conf must be an exact alias for render_surround_conf_ex(channels=8, eq=None)"
        );
    }

    // ── render_stereo_bypass_conf tests ──────────────────────────────────────

    /// Passthrough (crossfeed=0, no EQ): 2-channel FL/FR sink, no convolver nodes.
    #[test]
    fn stereo_bypass_passthrough_no_eq_is_2ch_no_convolver() {
        let spec = test_spec();
        let got = render_stereo_bypass_conf(&spec, 0, None).unwrap();

        // No convolver nodes anywhere
        assert!(
            !got.contains("convolver"),
            "passthrough must not have any convolver nodes"
        );

        // capture.props must be a 2-ch stereo Audio/Sink
        let cap = got
            .split("capture.props")
            .nth(1)
            .and_then(|s| s.split("playback.props").next())
            .expect("capture.props section");
        assert!(
            cap.contains("media.class = Audio/Sink"),
            "must be Audio/Sink"
        );
        assert!(
            cap.contains("audio.channels = 2"),
            "must have 2 channels"
        );
        assert!(
            cap.contains("audio.position = [ FL FR ]"),
            "must have stereo position"
        );

        // playback.props must also be 2-ch
        let pb = got
            .split("playback.props")
            .nth(1)
            .expect("playback.props section");
        assert!(pb.contains("audio.channels = 2"), "playback must be 2-ch");
        assert!(pb.contains("audio.position = [ FL FR ]"), "playback must be FL FR");
        assert!(pb.contains("target.object = \"hwsink\""), "target must be set");

        // Passthrough now routes through the level-matching mixers: −6 dB pad
        // (SURROUND_MIX_HEADROOM_GAIN) matches the HRIR graph so mode switches
        // compare tone, not loudness.
        assert!(
            got.contains("name = mixL  control = { \"Gain 1\" = 0.5 }"),
            "bypass mixL must carry the −6 dB pad, got:\n{got}"
        );
        assert!(
            got.contains("name = mixR  control = { \"Gain 1\" = 0.5 }"),
            "bypass mixR must carry the −6 dB pad"
        );
        assert!(
            got.contains("outputs = [ \"mixL:Out\" \"mixR:Out\" ]"),
            "passthrough outputs must come from the padded mixers"
        );
        assert!(
            got.contains("output = \"copyL:Out\"  input = \"mixL:In 1\""),
            "copyL must feed mixL In 1"
        );
    }

    /// crossfeed=0 with a 1-band EQ: eq_l_0/eq_r_0 nodes present, outputs from them.
    #[test]
    fn stereo_bypass_with_eq_applies_tail() {
        let spec = test_spec();
        let eq = EqModel {
            bands: vec![EqBand::new(BandKind::Peaking, 500.0, 1.0, 0.0)],
        };
        let got = render_stereo_bypass_conf(&spec, 0, Some(&eq)).unwrap();

        // No convolver nodes
        assert!(!got.contains("convolver"), "must not have convolver nodes");

        // EQ nodes are present
        assert!(got.contains("\"eq_l_0\""), "must have eq_l_0 node");
        assert!(got.contains("\"eq_r_0\""), "must have eq_r_0 node");
        assert!(
            !got.contains("\"eq_l_1\""),
            "single-band chain must not have eq_l_1"
        );
        assert!(
            !got.contains("\"eq_r_1\""),
            "single-band chain must not have eq_r_1"
        );

        // The padded mixers link into the EQ tail (level-matching pad always
        // in the path).
        assert!(
            got.contains("output = \"mixL:Out\"  input = \"eq_l_0:In\""),
            "mixL must link to eq_l_0"
        );
        assert!(
            got.contains("output = \"mixR:Out\"  input = \"eq_r_0:In\""),
            "mixR must link to eq_r_0"
        );
        assert!(
            got.contains("\"Gain 1\" = 0.5"),
            "−6 dB pad must be present with EQ too"
        );

        // Outputs come from the EQ nodes, not the mixers
        assert!(
            got.contains("outputs = [ \"eq_l_0:Out\" \"eq_r_0:Out\" ]"),
            "outputs must reference eq_l_0/eq_r_0"
        );
        assert!(
            !got.contains("outputs = [ \"mixL:Out\" \"mixR:Out\" ]"),
            "mixer outputs must not appear when EQ is present"
        );
    }

    /// crossfeed=50 → direct 0.5 (−6 dB pad), cross = (50/100)·0.5·0.5 = 0.125:
    /// same direct:cross ratio as before the pad, one level with the HRIR graph.
    #[test]
    fn stereo_bypass_crossfeed_blends_opposite_channel() {
        let spec = test_spec();
        let got = render_stereo_bypass_conf(&spec, 50, None).unwrap();

        // No convolver nodes
        assert!(!got.contains("convolver"), "must not have convolver nodes");

        // Mixer nodes are present
        assert!(got.contains("name = mixL"), "must have mixL node");
        assert!(got.contains("name = mixR"), "must have mixR node");

        // Per-input gain controls: Gain 1 = 0.5 (direct, −6 dB pad),
        // Gain 2 = 0.125 (cross, same ratio to direct as before the pad)
        assert!(
            got.contains("\"Gain 1\" = 0.5  \"Gain 2\" = 0.125"),
            "mixer must have Gain 1=0.5 and Gain 2=0.125 for crossfeed=50, got:\n{got}"
        );

        // Direct links: each channel's own copy to its mixer's In 1
        assert!(
            got.contains("output = \"copyL:Out\"  input = \"mixL:In 1\""),
            "copyL must feed mixL In 1 (direct)"
        );
        assert!(
            got.contains("output = \"copyR:Out\"  input = \"mixR:In 1\""),
            "copyR must feed mixR In 1 (direct)"
        );

        // Cross links: opposite channel to In 2
        assert!(
            got.contains("output = \"copyR:Out\"  input = \"mixL:In 2\""),
            "copyR must feed mixL In 2 (crossfeed)"
        );
        assert!(
            got.contains("output = \"copyL:Out\"  input = \"mixR:In 2\""),
            "copyL must feed mixR In 2 (crossfeed)"
        );

        // Outputs come from mixers (no EQ)
        assert!(
            got.contains("outputs = [ \"mixL:Out\" \"mixR:Out\" ]"),
            "crossfeed outputs must be from mixers"
        );
    }

    /// crossfeed=50 with a 1-band EQ: mixers are present AND EQ chain composes.
    /// Verifies that mixL/mixR link INTO the EQ tail (not copyL/copyR).
    #[test]
    fn stereo_bypass_crossfeed_with_eq_composes() {
        let spec = test_spec();
        let eq = EqModel {
            bands: vec![EqBand::new(BandKind::Peaking, 1000.0, 1.0, 2.0)],
        };
        let got = render_stereo_bypass_conf(&spec, 50, Some(&eq)).unwrap();

        // No convolver nodes
        assert!(!got.contains("convolver"), "must not have convolver nodes");

        // Mixer nodes are present (for crossfeed)
        assert!(got.contains("name = mixL"), "must have mixL node for crossfeed");
        assert!(got.contains("name = mixR"), "must have mixR node for crossfeed");

        // EQ nodes are present
        assert!(got.contains("\"eq_l_0\""), "must have eq_l_0 node");
        assert!(got.contains("\"eq_r_0\""), "must have eq_r_0 node");

        // Mixer gains set for crossfeed=50 with the −6 dB pad
        assert!(
            got.contains("\"Gain 1\" = 0.5  \"Gain 2\" = 0.125"),
            "mixer must have correct padded crossfeed gains"
        );

        // CRITICAL: mixL and mixR link to eq_l_0 and eq_r_0 (NOT copyL/copyR)
        assert!(
            got.contains("output = \"mixL:Out\"  input = \"eq_l_0:In\""),
            "mixL must link to eq_l_0 when both crossfeed and EQ are present"
        );
        assert!(
            got.contains("output = \"mixR:Out\"  input = \"eq_r_0:In\""),
            "mixR must link to eq_r_0 when both crossfeed and EQ are present"
        );

        // Ensure copyL/copyR do NOT link to EQ (they go to mixer instead)
        assert!(
            !got.contains("output = \"copyL:Out\"  input = \"eq_l_0:In\""),
            "copyL must NOT link to eq_l_0; it goes to mixL first"
        );
        assert!(
            !got.contains("output = \"copyR:Out\"  input = \"eq_r_0:In\""),
            "copyR must NOT link to eq_r_0; it goes to mixR first"
        );

        // Outputs come from EQ, not directly from mixer
        assert!(
            got.contains("outputs = [ \"eq_l_0:Out\" \"eq_r_0:Out\" ]"),
            "outputs must reference eq_l_0/eq_r_0 when EQ is present"
        );
    }

    /// crossfeed > 100 must return an error (out of valid range 0..=100).
    #[test]
    fn stereo_bypass_crossfeed_over_100_errors() {
        let spec = test_spec();
        let err = render_stereo_bypass_conf(&spec, 200, None).unwrap_err();
        assert!(
            matches!(err, AudioError::Invalid(_)),
            "crossfeed=200 must return Invalid, got {err:?}"
        );
        // Verify the error message mentions the constraint
        let msg = format!("{err:?}");
        assert!(msg.contains("0..=100"), "error must mention valid range 0..=100");
        assert!(msg.contains("200"), "error must mention the invalid value 200");
    }

    // ── SurroundBackend tests (MockRunner) ────────────────────────────────────

    #[test]
    fn create_when_absent_spawns_pipewire() {
        let runner = MockRunner::new().with_output(0, LS_WITHOUT_SURROUND, "");
        let mut be = SurroundBackend::new(runner, test_spec());
        let handle = be.create(&PathBuf::from("/test/hrir.wav")).unwrap();

        let spawned = &be.runner().spawned;
        assert_eq!(spawned.len(), 1, "one spawn_owned call");
        assert_eq!(spawned[0][0], "pipewire");
        assert_eq!(spawned[0][1], "-c");
        assert!(
            spawned[0][2].ends_with("arctis_surround.conf"),
            "conf path ends with arctis_surround.conf, got: {}",
            spawned[0][2]
        );
        assert!(handle.child.is_some(), "child token must be present");
    }

    #[test]
    fn create_is_idempotent_when_source_exists() {
        let runner = MockRunner::new().with_output(0, LS_WITH_SURROUND, "");
        let mut be = SurroundBackend::new(runner, test_spec());
        let handle = be.create(&PathBuf::from("/test/hrir.wav")).unwrap();

        // Only the ls Node existence check ran; no spawn.
        let calls = &be.runner().calls;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
        assert!(
            be.runner().spawned.is_empty(),
            "no spawn when source present"
        );
        assert!(handle.child.is_none(), "no child token when source present");
    }

    #[test]
    fn remove_when_present_destroys_exact_node_id() {
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_SURROUND, "") // source_exists
            .with_output(0, LS_WITH_SURROUND, "") // find_node_id
            .with_output(0, "", "") // pw-cli destroy
            .with_output(0, "", ""); // pkill -f <conf>
        let mut be = SurroundBackend::new(runner, test_spec());
        be.remove().unwrap();

        let calls = &be.runner().calls;
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
        assert_eq!(calls[1], vec!["pw-cli", "ls", "Node"]);
        assert_eq!(calls[2], vec!["pw-cli", "destroy", "81"]);
        assert_eq!(calls[3][0], "pkill");
        assert_eq!(calls[3][1], "-f");
        assert!(
            calls[3][2].ends_with("arctis_surround.conf"),
            "pkill target ends with arctis_surround.conf, got: {}",
            calls[3][2]
        );
    }

    #[test]
    fn remove_is_noop_when_absent() {
        let runner = MockRunner::new().with_output(0, LS_WITHOUT_SURROUND, "");
        let mut be = SurroundBackend::new(runner, test_spec());
        be.remove().unwrap();
        // Only the existence check ran; no destroy or pkill.
        assert_eq!(be.runner().calls.len(), 1);
    }

    #[test]
    fn recreate_removes_then_creates() {
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_SURROUND, "") // remove: source_exists
            .with_output(0, LS_WITH_SURROUND, "") // remove: find_node_id
            .with_output(0, "", "") // remove: destroy
            .with_output(0, "", "") // remove: pkill
            .with_output(0, LS_WITHOUT_SURROUND, ""); // create: source_exists (absent)
        let mut be = SurroundBackend::new(runner, test_spec());
        let handle = be.recreate(&PathBuf::from("/test/hrir.wav")).unwrap();

        let calls = &be.runner().calls;
        assert_eq!(calls[2], vec!["pw-cli", "destroy", "81"]);
        assert_eq!(calls.len(), 5, "5 run calls: ls ls destroy pkill ls-absent");
        let spawned = &be.runner().spawned;
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0][0], "pipewire");
        assert!(
            handle.child.is_some(),
            "recreate must surface new child token"
        );
    }

    // ── recreate_ex / recreate_stereo_bypass tests ───────────────────────────
    //
    // Each test uses a unique node_name_base to avoid file-system races when
    // tests run in parallel (all would otherwise share the same conf path).

    #[test]
    fn recreate_ex_8ch_with_eq_writes_conf_with_eq_and_spawns() {
        let spec = SurroundSpec {
            node_name_base: "test_surr_8ch_eq".into(),
            description: "Test 8ch EQ".into(),
            hw_sink: None,
        };
        // Scrub any stale conf so the diff-guard cannot skip the scripted spawn.
        let _ = std::fs::remove_file(std::env::temp_dir().join("arctis_test_surr_8ch_eq.conf"));
        // remove: source_exists → absent (early return, no destroy)
        let runner = MockRunner::new().with_output(0, LS_WITHOUT_SURROUND, "");
        let mut be = SurroundBackend::new(runner, spec);
        let hrir = PathBuf::from("/test/hrir.wav");
        let eq = EqModel {
            bands: vec![EqBand::new(BandKind::Peaking, 1000.0, 1.0, 3.0)],
        };
        let handle = be.recreate_ex(&hrir, 8, Some(&eq), None, None, None).unwrap();

        // (a) one spawn_owned("pipewire", ["-c", <conf_path>]) recorded
        let spawned = &be.runner().spawned;
        assert_eq!(spawned.len(), 1, "exactly one spawn_owned call");
        assert_eq!(spawned[0][0], "pipewire");
        assert_eq!(spawned[0][1], "-c");
        assert!(
            spawned[0][2].ends_with("test_surr_8ch_eq.conf"),
            "conf path ends with test_surr_8ch_eq.conf, got: {}",
            spawned[0][2]
        );
        assert!(handle.child.is_some(), "child token must be present");

        // (b) conf file contains EQ nodes and 8-channel capture
        let conf = std::fs::read_to_string(&spawned[0][2])
            .expect("conf file must exist at spawned path");
        assert!(
            conf.contains("\"eq_l_0\""),
            "conf must contain eq_l_0 node (EQ tail present)"
        );
        assert!(
            conf.contains("\"eq_r_0\""),
            "conf must contain eq_r_0 node (EQ tail present)"
        );
        assert!(
            conf.contains("audio.channels = 8"),
            "conf must have 8-channel capture props"
        );
    }

    #[test]
    fn recreate_ex_6ch_writes_5_1_conf() {
        let spec = SurroundSpec {
            node_name_base: "test_surr_6ch".into(),
            description: "Test 6ch".into(),
            hw_sink: None,
        };
        let _ = std::fs::remove_file(std::env::temp_dir().join("arctis_test_surr_6ch.conf"));
        // remove: source_exists → absent
        let runner = MockRunner::new().with_output(0, LS_WITHOUT_SURROUND, "");
        let mut be = SurroundBackend::new(runner, spec);
        let hrir = PathBuf::from("/test/hrir.wav");
        let handle = be.recreate_ex(&hrir, 6, None, None, None, None).unwrap();

        let spawned = &be.runner().spawned;
        assert_eq!(spawned.len(), 1, "exactly one spawn_owned call");
        assert!(handle.child.is_some(), "child token must be present");

        let conf = std::fs::read_to_string(&spawned[0][2])
            .expect("conf file must exist at spawned path");
        assert!(
            conf.contains("audio.channels = 6"),
            "5.1 conf must have 6-channel capture props"
        );
    }

    #[test]
    fn recreate_stereo_bypass_writes_2ch_no_convolver() {
        let spec = SurroundSpec {
            node_name_base: "test_surr_bypass".into(),
            description: "Test bypass".into(),
            hw_sink: None,
        };
        let _ = std::fs::remove_file(std::env::temp_dir().join("arctis_test_surr_bypass.conf"));
        // remove: source_exists → absent
        let runner = MockRunner::new().with_output(0, LS_WITHOUT_SURROUND, "");
        let mut be = SurroundBackend::new(runner, spec);
        let handle = be.recreate_stereo_bypass(0, None).unwrap();

        let spawned = &be.runner().spawned;
        assert_eq!(spawned.len(), 1, "exactly one spawn_owned call");
        assert!(handle.child.is_some(), "child token must be present");

        let conf = std::fs::read_to_string(&spawned[0][2])
            .expect("conf file must exist at spawned path");
        assert!(
            conf.contains("audio.channels = 2"),
            "stereo bypass must have 2-channel capture props"
        );
        assert!(
            !conf.contains("convolver"),
            "stereo bypass must not contain any convolver node"
        );
    }

    /// Diff-before-recreate: identical on-disk conf + live node → NO teardown,
    /// no respawn (an unnecessary respawn is an audible dropout).
    #[test]
    fn recreate_ex_skips_respawn_when_conf_identical_and_node_live() {
        let spec = SurroundSpec {
            node_name_base: "test_surr_guard".into(),
            description: "Guard".into(),
            hw_sink: None,
        };
        let hrir = PathBuf::from("/test/hrir.wav");
        // Pre-write the EXACT conf recreate_ex would render.
        let conf = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &hrir,
            channels: 8,
            output_eq: None,
            blocksize: Some(128),
            gain: None,
            tailsize: Some(1024),
        })
        .unwrap();
        let path = std::env::temp_dir().join("arctis_test_surr_guard.conf");
        std::fs::write(&path, conf).unwrap();

        let ls = "id 81, type PipeWire:Interface:Node/3\n    node.name = \"effect_input.test_surr_guard\"\n";
        let runner = MockRunner::new().with_output(0, ls, ""); // guard: source_exists (present)
        let mut be = SurroundBackend::new(runner, spec);
        let handle = be
            .recreate_ex(&hrir, 8, None, Some(128), None, Some(1024))
            .unwrap();

        assert!(handle.child.is_none(), "no respawn when nothing changed");
        assert_eq!(be.runner().calls.len(), 1, "only the existence check runs");
        assert!(be.runner().spawned.is_empty(), "no pipewire spawn");
        let _ = std::fs::remove_file(&path);
    }

    /// Diff-before-recreate: a CHANGED conf must tear down + respawn as before.
    #[test]
    fn recreate_ex_respawns_when_conf_changed() {
        let spec = SurroundSpec {
            node_name_base: "test_surr_guard2".into(),
            description: "Guard2".into(),
            hw_sink: None,
        };
        let path = std::env::temp_dir().join("arctis_test_surr_guard2.conf");
        std::fs::write(&path, "stale contents").unwrap();

        // remove: source_exists → absent (early return) → spawn
        let runner = MockRunner::new().with_output(0, LS_WITHOUT_SURROUND, "");
        let mut be = SurroundBackend::new(runner, spec);
        let handle = be
            .recreate_ex(&PathBuf::from("/test/hrir.wav"), 8, None, None, None, None)
            .unwrap();
        assert!(handle.child.is_some(), "changed conf must respawn");
        assert_eq!(be.runner().spawned.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    /// Tailsize is emitted on every convolver when set.
    #[test]
    fn render_with_tailsize_emits_tailsize_on_every_convolver() {
        let spec = test_spec();
        let got = render_surround_conf_ex(&SurroundRender {
            spec: &spec,
            hrir_path: &PathBuf::from("/test/hrir.wav"),
            channels: 8,
            output_eq: None,
            blocksize: Some(64),
            gain: None,
            tailsize: Some(4096),
        })
        .unwrap();
        let conv_lines: Vec<&str> = got.lines().filter(|l| l.contains("label = convolver")).collect();
        assert_eq!(conv_lines.len(), 16);
        for line in &conv_lines {
            assert!(
                line.contains("blocksize = 64  tailsize = 4096"),
                "convolver line missing blocksize/tailsize pair: {line}"
            );
        }
    }
}
