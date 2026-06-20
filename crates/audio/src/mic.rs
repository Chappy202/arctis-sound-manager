use crate::backend::{parse_node_id, ConfHandle};
use crate::config::{render_chain_conf, ChainSpec, FilterNode};
use crate::error::AudioError;
use crate::props::set_control_props_argv;
use crate::runner::CommandRunner;

// ─── Plugin path constants ────────────────────────────────────────────────────

/// RNNoise LADSPA plugin path on this system.
pub const RNNOISE_PLUGIN: &str = "/usr/lib64/ladspa/librnnoise_ladspa.so";
/// RNNoise mono label.
pub const RNNOISE_LABEL_MONO: &str = "noise_suppressor_mono";
/// swh sc4m compressor path.
pub const SC4M_PLUGIN: &str = "/usr/lib64/ladspa/sc4m_1916.so";
/// swh sc4m label.
pub const SC4M_LABEL: &str = "sc4m";

// ─── PluginProbe seam ─────────────────────────────────────────────────────────

/// Returns true if a LADSPA plugin .so exists and is (likely) usable.
/// Builtin nodes are always available and never need a probe check.
///
/// `Send` bound required so that `Box<dyn PluginProbe>` can be held in `Engine<R>`
/// which the daemon spawns on a thread.
pub trait PluginProbe: Send {
    fn ladspa_exists(&self, plugin_path: &str) -> bool;
}

/// Production probe: `std::path::Path::exists`.
pub struct FsPluginProbe;

impl PluginProbe for FsPluginProbe {
    fn ladspa_exists(&self, plugin_path: &str) -> bool {
        std::path::Path::new(plugin_path).exists()
    }
}

/// Test probe: only reports plugins in its allow-set as present.
/// Counts every `ladspa_exists` call so tests can assert the probe was (or was
/// not) consulted.
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

    /// Report the given paths as present.
    pub fn with<I: IntoIterator<Item = S>, S: Into<String>>(paths: I) -> Self {
        Self {
            present: paths.into_iter().map(|s| s.into()).collect(),
            call_count: Default::default(),
        }
    }

    /// Total number of times `ladspa_exists` has been called on this probe.
    pub fn calls(&self) -> usize {
        self.call_count.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl PluginProbe for MockPluginProbe {
    fn ladspa_exists(&self, plugin_path: &str) -> bool {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.present.contains(plugin_path)
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
    /// LADSPA RNNoise `noise_suppressor_mono` (requires plugin .so).
    Rnnoise,
    /// LADSPA swh sc4m compressor (requires plugin .so).
    Compressor,
    /// Builtin `noisegate` (always available).
    Gate,
    /// Builtin biquad mic-EQ bands (always available).
    MicEq,
}

impl StageKind {
    /// True when this stage is a PipeWire builtin (no external .so needed).
    pub fn is_builtin(self) -> bool {
        matches!(
            self,
            StageKind::Gain | StageKind::Highpass | StageKind::Gate | StageKind::MicEq
        )
    }

    /// Return the LADSPA plugin path for LADSPA stages; None for builtins.
    pub fn plugin_path(self) -> Option<&'static str> {
        match self {
            StageKind::Rnnoise => Some(RNNOISE_PLUGIN),
            StageKind::Compressor => Some(SC4M_PLUGIN),
            _ => None,
        }
    }
}

// ─── MicBackend ───────────────────────────────────────────────────────────────

/// Backend for the Clean Mic virtual `Audio/Source`. Mirrors `AudioBackend`'s
/// lifecycle (idempotent create/remove/recreate) but uses a `ChainSpec` +
/// `[FilterNode]` instead of `SinkSpec` + `EqModel`, so it works with the
/// generalized renderer. Plugin availability is checked via `PluginProbe`.
pub struct MicBackend<R: CommandRunner, P: PluginProbe> {
    runner: R,
    probe: P,
    spec: ChainSpec,
}

impl<R: CommandRunner, P: PluginProbe> MicBackend<R, P> {
    pub fn new(runner: R, probe: P, spec: ChainSpec) -> Self {
        Self {
            runner,
            probe,
            spec,
        }
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

    /// Report availability of the requested stages.
    ///
    /// Builtin stages are always `true`. LADSPA stages depend on `probe`.
    pub fn availability(&self, stages: &[StageKind]) -> Vec<(StageKind, bool)> {
        stages
            .iter()
            .map(|&stage| {
                let available = if stage.is_builtin() {
                    true
                } else {
                    stage
                        .plugin_path()
                        .map(|p| self.probe.ladspa_exists(p))
                        .unwrap_or(false)
                };
                (stage, available)
            })
            .collect()
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
            controls: vec![("Mult".to_string(), 1.0_f32), ("Add".to_string(), 0.0_f32)],
        }]
    }

    fn full_nodes_with_rnnoise() -> Vec<FilterNode> {
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
                name: "mic_rnnoise".into(),
                node_type: NodeType::Ladspa,
                label: RNNOISE_LABEL_MONO.into(),
                plugin: Some(RNNOISE_PLUGIN.into()),
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
        let probe = MockPluginProbe::none(); // probe never consulted for builtin-only
                                             // Keep a shared ref to count probe calls before MicBackend takes ownership.
        let probe_calls = probe.call_count.clone();
        let mut be = MicBackend::new(runner, probe, mic_spec());
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
        // Probe must never have been consulted: passthrough chain has no LADSPA nodes.
        assert_eq!(
            probe_calls.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "probe must not be consulted for a builtin-only passthrough chain"
        );
    }

    // ── Test 2: full chain with rnnoise present ──────────────────────────────

    #[test]
    fn create_full_chain_spawns_when_rnnoise_present() {
        let runner = MockRunner::new().with_output(0, LS_WITHOUT_MIC, "");
        let probe = MockPluginProbe::with([RNNOISE_PLUGIN]);
        let mut be = MicBackend::new(runner, probe, mic_spec());
        let handle = be.create(&full_nodes_with_rnnoise()).unwrap();

        let spawned = &be.runner().spawned;
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0][0], "pipewire");
        assert!(handle.child.is_some());
    }

    // ── Test 3: idempotent when source already present ───────────────────────

    #[test]
    fn create_is_idempotent_when_source_exists() {
        let runner = MockRunner::new().with_output(0, LS_WITH_MIC, "");
        let probe = MockPluginProbe::none();
        let mut be = MicBackend::new(runner, probe, mic_spec());
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
        let probe = MockPluginProbe::with([RNNOISE_PLUGIN]);
        let mut be = MicBackend::new(runner, probe, mic_spec());
        be.apply_control("mic_rnnoise", "VAD Threshold (%)", 40.0)
            .unwrap();

        let last = be.runner().last_call().unwrap();
        assert_eq!(
            last,
            &vec![
                "pw-cli".to_string(),
                "s".to_string(),
                "71".to_string(),
                "Props".to_string(),
                "{ params = [ \"mic_rnnoise:VAD Threshold (%)\" 40.0 ] }".to_string(),
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
        let probe = MockPluginProbe::none();
        let mut be = MicBackend::new(runner, probe, mic_spec());
        be.remove().unwrap();

        let calls = &be.runner().calls;
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
        assert_eq!(calls[1], vec!["pw-cli", "ls", "Node"]);
        assert_eq!(calls[2], vec!["pw-cli", "destroy", "71"]);
        assert_eq!(calls[3][0], "pkill");
        assert_eq!(calls[3][1], "-f");
        assert!(calls[3][2].ends_with("arctis_clean_mic.conf"));
    }

    // ── Test 6: availability — builtins true, ladspa per probe ──────────────

    #[test]
    fn availability_marks_builtin_true_ladspa_per_probe() {
        let probe = MockPluginProbe::none(); // no LADSPA plugins present
        let be = MicBackend::new(MockRunner::new(), probe, mic_spec());

        let stages = vec![
            StageKind::Gain,
            StageKind::Highpass,
            StageKind::Rnnoise,
            StageKind::Gate,
            StageKind::MicEq,
            StageKind::Compressor,
        ];
        let avail = be.availability(&stages);

        assert_eq!(avail.len(), 6);
        assert_eq!(avail[0], (StageKind::Gain, true)); // builtin
        assert_eq!(avail[1], (StageKind::Highpass, true)); // builtin
        assert_eq!(avail[2], (StageKind::Rnnoise, false)); // LADSPA, probe says absent
        assert_eq!(avail[3], (StageKind::Gate, true)); // builtin
        assert_eq!(avail[4], (StageKind::MicEq, true)); // builtin
        assert_eq!(avail[5], (StageKind::Compressor, false)); // LADSPA, probe says absent
    }

    #[test]
    fn availability_ladspa_true_when_probe_reports_present() {
        let probe = MockPluginProbe::with([RNNOISE_PLUGIN, SC4M_PLUGIN]);
        let be = MicBackend::new(MockRunner::new(), probe, mic_spec());

        let avail = be.availability(&[StageKind::Rnnoise, StageKind::Compressor]);
        assert_eq!(avail[0], (StageKind::Rnnoise, true));
        assert_eq!(avail[1], (StageKind::Compressor, true));
    }

    // ── Test 7: parse_node_id shared with sink backend (regression) ─────────

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
