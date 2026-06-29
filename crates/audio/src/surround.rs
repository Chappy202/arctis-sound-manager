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
    fn capture_node_name(&self) -> String {
        format!("effect_input.{}", self.node_name_base)
    }

    fn playback_node_name(&self) -> String {
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
}

// ─── Conf renderer ────────────────────────────────────────────────────────────

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
        out.push_str(&format!(
            "                    {{ type = builtin  label = convolver  name = {name}  config = {{ filename = \"{hrir_str}\"  channel = {ch} }} }}\n"
        ));
    }

    // Mixer nodes
    out.push_str("                    { type = builtin  label = mixer  name = mixL }\n");
    out.push_str("                    { type = builtin  label = mixer  name = mixR }\n");

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

    /// 8-channel render with a 2-band EqModel must insert per-ear bq nodes
    /// linked after the mixers, with the graph output coming from the last EQ nodes.
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
        })
        .unwrap();
        assert_eq!(
            via_wrapper, via_ex,
            "render_surround_conf must be an exact alias for render_surround_conf_ex(channels=8, eq=None)"
        );
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
}
