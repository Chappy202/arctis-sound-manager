use crate::backend::{parse_node_id, ConfHandle};
use crate::config::{render_chain_conf, ChainSpec, FilterNode};
use crate::error::AudioError;
use crate::props::set_control_props_argv;
use crate::runner::CommandRunner;

// ─── Plugin basename constants ────────────────────────────────────────────────

/// RNNoise LADSPA plugin basename (PipeWire resolves via $LADSPA_PATH).
pub const RNNOISE_PLUGIN_BASENAME: &str = "librnnoise_ladspa";
/// RNNoise mono label.
pub const RNNOISE_LABEL_MONO: &str = "noise_suppressor_mono";

/// DeepFilterNet LADSPA plugin basename.
pub const DEEPFILTER_PLUGIN_BASENAME: &str = "libdeep_filter_ladspa";
/// DeepFilterNet mono label.
pub const DEEPFILTER_LABEL_MONO: &str = "deep_filter_mono";

/// swh sc4m compressor basename.
pub const SC4M_PLUGIN_BASENAME: &str = "sc4m_1916";
/// swh sc4m label.
pub const SC4M_LABEL: &str = "sc4m";

/// swh gate basename.
pub const GATE_PLUGIN_BASENAME: &str = "gate_1410";
/// swh gate label.
pub const GATE_LABEL: &str = "gate";

/// Hard Limiter (Marcus Andersson, LADSPA 1413) basename — the always-on mic
/// output ceiling. Chosen over swh fast_lookahead_limiter_1913: the lookahead
/// limiter is stereo-only (Input 1/2, Output 1/2) and the mic filter-chain
/// renderer wires strictly linear mono links, so pairing one mono line into
/// both inputs is not cleanly expressible; hard_limiter is genuinely mono
/// ("Input"/"Output") and transparent below its ceiling.
pub const LIMITER_PLUGIN_BASENAME: &str = "hard_limiter_1413";
/// Hard Limiter label (verified with analyseplugin).
pub const LIMITER_LABEL: &str = "hardLimiter";

// ─── LADSPA multi-distro resolver ────────────────────────────────────────────

/// Search the standard LADSPA plugin directories for `<basename>.so` and return
/// the first existing absolute path.
///
/// Search order:
/// 1. Each colon-separated directory in `$LADSPA_PATH` (env var).
/// 2. `/usr/lib64/ladspa` (Fedora/Nobara/RHEL).
/// 3. `/usr/lib/ladspa` (Debian/Ubuntu/Arch).
/// 4. `/usr/lib/x86_64-linux-gnu/ladspa` (Debian multiarch).
///
/// Returns `None` if no match is found or on any I/O error.
pub fn resolve_ladspa(basename: &str) -> Option<std::path::PathBuf> {
    let filename = format!("{basename}.so");

    // Collect search dirs: $LADSPA_PATH first, then system defaults.
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(ladspa_path) = std::env::var("LADSPA_PATH") {
        for dir in ladspa_path.split(':') {
            if !dir.is_empty() {
                dirs.push(std::path::PathBuf::from(dir));
            }
        }
    }
    dirs.push(std::path::PathBuf::from("/usr/lib64/ladspa"));
    dirs.push(std::path::PathBuf::from("/usr/lib/ladspa"));
    dirs.push(std::path::PathBuf::from("/usr/lib/x86_64-linux-gnu/ladspa"));

    for dir in dirs {
        let candidate = dir.join(&filename);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

// ─── PluginProbe seam ─────────────────────────────────────────────────────────

/// Returns true if a LADSPA plugin basename is available (the .so can be found
/// by the resolver). Builtin nodes are always available and never need a probe check.
///
/// `Send` bound required so that `Box<dyn PluginProbe>` can be held in `Engine<R>`
/// which the daemon spawns on a thread.
pub trait PluginProbe: Send {
    /// Return true if `<basename>.so` is present and (likely) loadable.
    fn ladspa_available(&self, basename: &str) -> bool;
}

/// Production probe: uses `resolve_ladspa` to locate the .so.
pub struct FsPluginProbe;

impl PluginProbe for FsPluginProbe {
    fn ladspa_available(&self, basename: &str) -> bool {
        resolve_ladspa(basename).is_some()
    }
}

/// Test probe: only reports basenames in its allow-set as present.
/// Counts every `ladspa_available` call so tests can assert the probe was (or
/// was not) consulted.
#[cfg_attr(not(test), allow(dead_code))]
pub struct MockPluginProbe {
    pub present: std::collections::HashSet<String>,
    call_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl MockPluginProbe {
    /// All probes return false (nothing present).
    pub fn none() -> Self {
        Self {
            present: Default::default(),
            call_count: Default::default(),
        }
    }

    /// Report the given basenames as present.
    pub fn with<I: IntoIterator<Item = S>, S: Into<String>>(basenames: I) -> Self {
        Self {
            present: basenames.into_iter().map(|s| s.into()).collect(),
            call_count: Default::default(),
        }
    }

    /// Total number of times `ladspa_available` has been called on this probe.
    pub fn calls(&self) -> usize {
        self.call_count.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl PluginProbe for MockPluginProbe {
    fn ladspa_available(&self, basename: &str) -> bool {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.present.contains(basename)
    }
}

// ─── StageKind ────────────────────────────────────────────────────────────────

/// Identifies one DSP stage in the mic chain. Used for availability reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StageKind {
    /// Builtin `linear` gain node (always available).
    Gain,
    /// Builtin `bq_highpass` node (always available).
    Highpass,
    /// Noise suppression stage — DeepFilterNet or RNNoise (requires plugin .so).
    Suppression,
    /// LADSPA swh sc4m compressor (requires plugin .so).
    Compressor,
    /// Gate stage — builtin noisegate (≥1.6) or LADSPA gate_1410 fallback.
    /// Availability computed in `convert::mic_chain_nodes` based on PW version.
    Gate,
    /// Builtin biquad mic-EQ bands (always available).
    MicEq,
}

impl StageKind {}

// ─── MicBackend ───────────────────────────────────────────────────────────────

/// Backend for the Clean Mic virtual `Audio/Source`. Mirrors `AudioBackend`'s
/// lifecycle (idempotent create/remove/recreate) but uses a `ChainSpec` +
/// `[FilterNode]` instead of `SinkSpec` + `EqModel`, so it works with the
/// generalized renderer. Stage availability is decided by `convert::mic_chain_nodes`,
/// not by this struct.
pub struct MicBackend<R: CommandRunner> {
    runner: R,
    spec: ChainSpec,
}

impl<R: CommandRunner> MicBackend<R> {
    pub fn new(runner: R, spec: ChainSpec) -> Self {
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

    fn conf_path(&self) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("arctis_{}.conf", self.spec.node_name));
        p
    }

    /// True if the Clean Mic source node is already present.
    pub fn source_exists(&mut self) -> Result<bool, AudioError> {
        let out = self.runner.run("pw-cli", &["ls", "Node"])?;
        if out.status != 0 {
            return Err(AudioError::NonZeroExit {
                program: "pw-cli".to_string(),
                status: out.status,
                stderr: out.stderr,
            });
        }
        Ok(out
            .stdout
            .contains(&format!("node.name = \"{}\"", self.spec.playback_node_name)))
    }

    /// Create the Clean Mic source idempotently.
    ///
    /// Returns a `ConfHandle` whose `child` is `Some(token)` when a new
    /// `pipewire -c` was spawned; `None` when the source was already present.
    pub fn create(&mut self, nodes: &[FilterNode]) -> Result<ConfHandle, AudioError> {
        let path = self.conf_path();
        if self.source_exists()? {
            return Ok(ConfHandle {
                conf_path: path,
                child: None,
            });
        }
        let conf = render_chain_conf(&self.spec, nodes)?;
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

    /// Remove the Clean Mic source idempotently.
    pub fn remove(&mut self) -> Result<(), AudioError> {
        if !self.source_exists()? {
            // Best-effort stale-conf cleanup (mirrors AudioBackend::remove); ignore errors.
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

    /// Teardown then recreate (topology changes require a full respawn).
    pub fn recreate(&mut self, nodes: &[FilterNode]) -> Result<ConfHandle, AudioError> {
        self.remove()?;
        self.create(nodes)
    }

    /// Resolve the filter-chain node id for live Props updates.
    pub fn find_node_id(&mut self) -> Result<String, AudioError> {
        let out = self.runner.run("pw-cli", &["ls", "Node"])?;
        if out.status != 0 {
            return Err(AudioError::NonZeroExit {
                program: "pw-cli".to_string(),
                status: out.status,
                stderr: out.stderr,
            });
        }
        parse_node_id(&out.stdout, &self.spec.playback_node_name)
    }

    /// Apply one control live via `pw-cli s <id> Props …` (no restart).
    pub fn apply_control(
        &mut self,
        node_name: &str,
        control: &str,
        value: f32,
    ) -> Result<(), AudioError> {
        let id = self.find_node_id()?;
        let argv = set_control_props_argv(&id, node_name, control, value)?;
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let out = self.runner.run("pw-cli", &args)?;
        Self::check(out, "pw-cli")?;
        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ChainChannels, NodeType};

    /// Replica of the LS_WITH_SINK fixture from backend.rs tests — used to
    /// verify that the shared `parse_node_id` function still works (regression).
    const LS_WITH_SINK_REPLICA: &str = "\
id 40, type PipeWire:Interface:Node/3\n    node.name = \"alsa_output.pci\"\nid 57, type PipeWire:Interface:Node/3\n    node.name = \"arctis_eq\"\nid 58, type PipeWire:Interface:Node/3\n    node.name = \"arctis_eq.output\"\n";
    use crate::runner::MockRunner;

    /// pw-cli ls Node output that includes the Clean Mic source node.
    const LS_WITH_MIC: &str = "\
id 40, type PipeWire:Interface:Node/3
    node.name = \"alsa_output.pci\"
id 71, type PipeWire:Interface:Node/3
    node.name = \"arctis_clean_mic\"
id 72, type PipeWire:Interface:Node/3
    node.name = \"arctis_clean_mic.capture\"
";

    /// ls Node output that does NOT contain the mic source.
    const LS_WITHOUT_MIC: &str = "\
id 40, type PipeWire:Interface:Node/3
    node.name = \"alsa_output.pci\"
";

    fn mic_spec() -> ChainSpec {
        ChainSpec {
            node_name: "arctis_clean_mic".into(),
            description: "Clean Mic".into(),
            channels: ChainChannels::Mono,
            kind: crate::config::ChainKind::Source,
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
            controls: vec![("Mult".to_string(), 1.0_f32), ("Add".to_string(), 0.0_f32)],
        }]
    }

    fn full_nodes_with_suppression() -> Vec<FilterNode> {
        vec![
            FilterNode {
                name: "mic_gain".into(),
                node_type: NodeType::Builtin,
                label: "linear".into(),
                plugin: None,
                port_in: "In".into(),
                port_out: "Out".into(),
                controls: vec![("Mult".to_string(), 1.0_f32), ("Add".to_string(), 0.0_f32)],
            },
            FilterNode {
                name: "mic_suppression".into(),
                node_type: NodeType::Ladspa,
                label: RNNOISE_LABEL_MONO.into(),
                plugin: Some(RNNOISE_PLUGIN_BASENAME.into()),
                port_in: "Input".into(),
                port_out: "Output".into(),
                controls: vec![
                    ("VAD Threshold (%)".to_string(), 40.0_f32),
                    ("VAD Grace Period (ms)".to_string(), 800.0_f32),
                    ("Retroactive VAD Grace (ms)".to_string(), 100.0_f32),
                ],
            },
        ]
    }

    // ── Test 1: passthrough spawns pipewire -c, no LADSPA needed ────────────

    #[test]
    fn create_passthrough_spawns_with_no_ladspa() {
        // source_exists: absent
        let runner = MockRunner::new().with_output(0, LS_WITHOUT_MIC, "");
        let mut be = MicBackend::new(runner, mic_spec());
        let handle = be.create(&passthrough_nodes()).unwrap();

        // spawn_owned was called with "pipewire -c <conf>"
        let spawned = &be.runner().spawned;
        assert_eq!(spawned.len(), 1, "one spawn_owned call");
        assert_eq!(spawned[0][0], "pipewire");
        assert_eq!(spawned[0][1], "-c");
        assert!(
            spawned[0][2].ends_with("arctis_clean_mic.conf"),
            "conf path ends with arctis_clean_mic.conf, got: {}",
            spawned[0][2]
        );
        // Handle carries the child token.
        assert!(handle.child.is_some());
    }

    // ── Test 2: full chain with rnnoise present ──────────────────────────────

    #[test]
    fn create_full_chain_spawns_when_rnnoise_present() {
        let runner = MockRunner::new().with_output(0, LS_WITHOUT_MIC, "");
        let mut be = MicBackend::new(runner, mic_spec());
        let handle = be.create(&full_nodes_with_suppression()).unwrap();

        let spawned = &be.runner().spawned;
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0][0], "pipewire");
        assert!(handle.child.is_some());
    }

    // ── Test 3: idempotent when source already present ───────────────────────

    #[test]
    fn create_is_idempotent_when_source_exists() {
        let runner = MockRunner::new().with_output(0, LS_WITH_MIC, "");
        let mut be = MicBackend::new(runner, mic_spec());
        let handle = be.create(&passthrough_nodes()).unwrap();

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

    // ── Test 4: apply_control emits exact argv ───────────────────────────────

    #[test]
    fn apply_control_emits_exact_props_argv() {
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_MIC, "") // find_node_id
            .with_output(0, "", ""); // the set
        let mut be = MicBackend::new(runner, mic_spec());
        be.apply_control("mic_suppression", "VAD Threshold (%)", 40.0)
            .unwrap();

        let last = be.runner().last_call().unwrap();
        assert_eq!(
            last,
            &vec![
                "pw-cli".to_string(),
                "s".to_string(),
                "71".to_string(),
                "Props".to_string(),
                "{ params = [ \"mic_suppression:VAD Threshold (%)\" 40.0 ] }".to_string(),
            ]
        );
    }

    // ── Test 5: remove tears down exact node id ──────────────────────────────

    #[test]
    fn remove_when_present_destroys_exact_node_id() {
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_MIC, "") // source_exists
            .with_output(0, LS_WITH_MIC, "") // find_node_id
            .with_output(0, "", "") // pw-cli destroy
            .with_output(0, "", ""); // pkill -f <conf> (best-effort)
        let mut be = MicBackend::new(runner, mic_spec());
        be.remove().unwrap();

        let calls = &be.runner().calls;
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
        assert_eq!(calls[1], vec!["pw-cli", "ls", "Node"]);
        assert_eq!(calls[2], vec!["pw-cli", "destroy", "71"]);
        assert_eq!(calls[3][0], "pkill");
        assert_eq!(calls[3][1], "-f");
        assert!(calls[3][2].ends_with("arctis_clean_mic.conf"));
    }

    // ── Test 6: parse_node_id shared with sink backend (regression) ─────────

    #[test]
    fn parse_node_id_shared_with_sink_backend() {
        // Verify that the shared parse_node_id function (factored to pub(crate)
        // from backend.rs) still correctly parses the sink backend's fixture.
        assert_eq!(
            parse_node_id(LS_WITH_SINK_REPLICA, "arctis_eq").unwrap(),
            "57"
        );
    }
}
