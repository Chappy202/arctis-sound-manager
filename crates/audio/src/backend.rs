use crate::config::{render_filter_chain_conf, SinkSpec};
use crate::eq::{EqBand, EqModel};
use crate::error::AudioError;
use crate::props::{set_band_props_argv, set_node_volume_props_argv};
use crate::runner::{ChildToken, CmdOutput, CommandRunner};
use std::path::PathBuf;

/// Handle to the on-disk conf the dedicated `pipewire -c` instance reads.
/// `child` is `Some` when this call actually spawned the instance; `None` when
/// the sink was already present (idempotent create) or when no process was started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfHandle {
    pub conf_path: PathBuf,
    /// Token for the owned `pipewire -c <conf>` child, if one was just spawned.
    pub child: Option<ChildToken>,
}

pub struct AudioBackend<R: CommandRunner> {
    runner: R,
    spec: SinkSpec,
}

impl<R: CommandRunner> AudioBackend<R> {
    pub fn new(runner: R, spec: SinkSpec) -> Self {
        Self { runner, spec }
    }

    /// Expose the runner for assertions in tests.
    #[cfg(test)]
    pub fn runner(&self) -> &R {
        &self.runner
    }

    fn check(out: CmdOutput, program: &str) -> Result<CmdOutput, AudioError> {
        if out.status == 0 {
            Ok(out)
        } else {
            Err(AudioError::NonZeroExit {
                program: program.to_string(),
                status: out.status,
                stderr: out.stderr,
            })
        }
    }

    fn conf_path(&self) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("arctis_eq.{}.conf", self.spec.node_name));
        p
    }

    /// True if a node with our stable `node.name` is already present.
    pub fn sink_exists(&mut self) -> Result<bool, AudioError> {
        let out = self.runner.run("pw-cli", &["ls", "Node"])?;
        let out = Self::check(out, "pw-cli")?;
        Ok(out
            .stdout
            .contains(&format!("node.name = \"{}\"", self.spec.node_name)))
    }

    /// Create the sink idempotently (G3): if it already exists, reuse it.
    ///
    /// Returns a `ConfHandle` whose `child` is `Some(token)` when a new
    /// `pipewire -c` instance was spawned, or `None` when the sink was already
    /// present. The caller must track the token to ensure the process is reaped
    /// on shutdown.
    pub fn create(&mut self, eq: &EqModel) -> Result<ConfHandle, AudioError> {
        let path = self.conf_path();
        if self.sink_exists()? {
            return Ok(ConfHandle {
                conf_path: path,
                child: None,
            });
        }
        let conf = render_filter_chain_conf(&self.spec, eq)?;
        std::fs::write(&path, conf).map_err(|e| AudioError::Spawn {
            program: "write-conf".to_string(),
            source_msg: e.to_string(),
        })?;
        let path_str = path.to_string_lossy().into_owned();
        // spawn_owned: pipewire -c <conf> is a long-lived process whose pgid
        // the engine tracks so shutdown can SIGTERM it (no orphan leak).
        let token = self.runner.spawn_owned("pipewire", &["-c", &path_str])?;
        Ok(ConfHandle {
            conf_path: path,
            child: Some(token),
        })
    }

    /// Resolve the filter node id for live Props. Parses `pw-cli ls Node`.
    pub fn find_node_id(&mut self) -> Result<String, AudioError> {
        let out = self.runner.run("pw-cli", &["ls", "Node"])?;
        let out = Self::check(out, "pw-cli")?;
        parse_node_id(&out.stdout, &self.spec.node_name)
    }

    /// Apply one band live via `pw-cli s <id> Props …` (no restart — G3).
    pub fn apply_band(&mut self, band_index: usize, band: &EqBand) -> Result<(), AudioError> {
        let id = self.find_node_id()?;
        let argv = set_band_props_argv(&id, band_index, band)?;
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let out = self.runner.run("pw-cli", &args)?;
        Self::check(out, "pw-cli")?;
        Ok(())
    }

    /// Apply every band live (used by future re-apply-on-startup; here for E2E).
    pub fn apply_all(&mut self, eq: &EqModel) -> Result<(), AudioError> {
        eq.validate()?;
        let id = self.find_node_id()?;
        for (i, b) in eq.bands.iter().enumerate() {
            let argv = set_band_props_argv(&id, i, b)?;
            let args: Vec<&str> = argv.iter().map(String::as_str).collect();
            let out = self.runner.run("pw-cli", &args)?;
            Self::check(out, "pw-cli")?;
        }
        Ok(())
    }

    /// Apply volume+mute to the channel sink node live via `pw-cli s <id> Props …`.
    /// `volume_db` is in dB; converted to linear (10^(db/20)) for channelVolumes.
    /// Emits one identical channelVolume entry per sink channel (2 for stereo,
    /// 8 for a surround-routed 7.1 channel) so the gain applies to ALL channels.
    pub fn apply_volume_mute(&mut self, volume_db: f32, muted: bool) -> Result<(), AudioError> {
        let id = self.find_node_id()?;
        let linear = 10f32.powf(volume_db / 20.0);
        let vols = vec![linear; self.spec.channels.count() as usize];
        let argv = set_node_volume_props_argv(&id, &vols, muted)?;
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let out = self.runner.run("pw-cli", &args)?;
        Self::check(out, "pw-cli")?;
        Ok(())
    }

    /// Apply volume+mute using a 0–100 percent value via `pw-cli s <id> Props …`.
    /// Uses the PERCEPTUAL (cubic) scale to match wpctl/PipeWire/pavucontrol:
    /// user pct → raw linear channelVolumes = (pct/100)^3. (wpctl set-volume 0.5
    /// yields channelVolumes 0.125 = 0.5^3.) `parse_node_volume` reads the inverse
    /// (cbrt) so write/read round-trip. Channel sinks are stereo → 2 identical entries.
    pub fn apply_volume_mute_pct(
        &mut self,
        volume_pct: u8,
        muted: bool,
    ) -> Result<(), AudioError> {
        let id = self.find_node_id()?;
        let frac = (volume_pct as f32 / 100.0).clamp(0.0, 1.0);
        let linear = frac * frac * frac; // (pct/100)^3
        let vols = vec![linear; self.spec.channels.count() as usize];
        let argv = set_node_volume_props_argv(&id, &vols, muted)?;
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let out = self.runner.run("pw-cli", &args)?;
        Self::check(out, "pw-cli")?;
        Ok(())
    }

    /// Force a rebuild so a changed `SinkSpec` (e.g. a new playback target) is
    /// actually applied: tear down any existing instance, then create fresh.
    /// This is the enforcement the old per-channel output selector lacked.
    pub fn recreate(&mut self, eq: &EqModel) -> Result<ConfHandle, AudioError> {
        self.remove()?;
        self.create(eq)
    }

    /// Remove the sink idempotently: no-op if absent; else destroy the node
    /// and delete the conf. (Stopping the dedicated `pipewire -c` process is
    /// owner-managed for v1 — see Task 6; LATER: track and kill the child.)
    pub fn remove(&mut self) -> Result<(), AudioError> {
        if !self.sink_exists()? {
            let _ = std::fs::remove_file(self.conf_path());
            return Ok(());
        }
        let id = self.find_node_id()?;
        let out = self.runner.run("pw-cli", &["destroy", &id])?;
        Self::check(out, "pw-cli")?;
        // Best-effort: stop the dedicated `pipewire -c <conf>` instance.
        // pkill exits non-zero when nothing matches; ignore that — it's harmless.
        let conf_path_str = self.conf_path().to_string_lossy().into_owned();
        let _ = self.runner.run("pkill", &["-f", &conf_path_str]);
        let _ = std::fs::remove_file(self.conf_path());
        Ok(())
    }
}

/// Parse the numeric id of the node whose block declares `node.name = "<name>"`
/// in `pw-cli ls Node` output. Shared with `MicBackend`.
pub(crate) fn parse_node_id(stdout: &str, node_name: &str) -> Result<String, AudioError> {
    let needle = format!("node.name = \"{node_name}\"");
    let mut current_id: Option<String> = None;
    for line in stdout.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("id ") {
            // line form: `id 57, type PipeWire:Interface:Node/3`
            let id = rest
                .split([',', ' '])
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !id.is_empty() {
                current_id = Some(id);
            }
        }
        if trimmed.contains(&needle) {
            if let Some(id) = current_id.clone() {
                return Ok(id);
            }
        }
    }
    Err(AudioError::Parse {
        what: "node id".to_string(),
        detail: format!("no node with node.name=\"{node_name}\""),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::{BandKind, EqBand, EqModel};
    use crate::runner::MockRunner;

    fn spec() -> SinkSpec {
        SinkSpec {
            node_name: "arctis_eq".into(),
            description: "Arctis EQ Sink".into(),
            channels: crate::config::ChainChannels::Stereo,
            playback_target: None,
        }
    }

    const LS_WITH_SINK: &str = "\
id 40, type PipeWire:Interface:Node/3
    node.name = \"alsa_output.pci\"
id 57, type PipeWire:Interface:Node/3
    node.name = \"arctis_eq\"
id 58, type PipeWire:Interface:Node/3
    node.name = \"arctis_eq.output\"
";

    #[test]
    fn parses_node_id_for_stable_name() {
        assert_eq!(parse_node_id(LS_WITH_SINK, "arctis_eq").unwrap(), "57");
    }

    #[test]
    fn parse_errors_when_absent() {
        assert!(parse_node_id(LS_WITH_SINK, "nope").is_err());
    }

    #[test]
    fn create_is_idempotent_when_sink_exists() {
        let runner = MockRunner::new().with_output(0, LS_WITH_SINK, "");
        let mut be = AudioBackend::new(runner, spec());
        let handle = be.create(&EqModel::default_10band()).unwrap();
        // Only the `ls Node` existence check ran; no `pipewire -c` spawn.
        let calls = &be.runner().calls;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
        // No child spawned when sink already exists.
        assert!(handle.child.is_none(), "no child when sink already present");
        assert!(be.runner().spawned.is_empty(), "no spawn_owned calls");
    }

    #[test]
    fn create_spawns_dedicated_instance_when_absent() {
        // pipewire -c now goes through spawn_owned (recorded in `spawned`, not `calls`).
        let runner = MockRunner::new().with_output(
            0,
            "id 1, type PipeWire:Interface:Node/3\n    node.name = \"x\"\n",
            "",
        );
        let mut be = AudioBackend::new(runner, spec());
        let handle = be.create(&EqModel::default_10band()).unwrap();
        // Only the ls-Node existence check went through `run`.
        let calls = &be.runner().calls;
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
        assert_eq!(calls.len(), 1, "only the ls check hits `run`");
        // The pipewire spawn is in `spawned`.
        let spawned = &be.runner().spawned;
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0][0], "pipewire");
        assert_eq!(spawned[0][1], "-c");
        assert!(spawned[0][2].ends_with("arctis_eq.conf"));
        // The handle carries the child token.
        assert!(handle.child.is_some(), "handle must carry the child token");
    }

    #[test]
    fn apply_band_emits_exact_pw_cli_props_argv() {
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_SINK, "") // find_node_id
            .with_output(0, "", ""); // the set
        let mut be = AudioBackend::new(runner, spec());
        let band = EqBand::new(BandKind::Peaking, 1200.0, 1.0, -4.5);
        be.apply_band(3, &band).unwrap();
        let last = be.runner().last_call().unwrap();
        assert_eq!(
            last,
            &vec![
                "pw-cli".to_string(),
                "s".to_string(),
                "57".to_string(),
                "Props".to_string(),
                "{ params = [ \"eq_band_3:Freq\" 1200.0 \"eq_band_3:Q\" 1.0 \"eq_band_3:Gain\" -4.5 ] }".to_string(),
            ]
        );
    }

    #[test]
    fn nonzero_exit_is_typed_error() {
        let runner = MockRunner::new().with_output(1, "", "denied");
        let mut be = AudioBackend::new(runner, spec());
        let err = be.sink_exists().unwrap_err();
        assert!(matches!(err, AudioError::NonZeroExit { status: 1, .. }));
    }

    #[test]
    fn remove_is_noop_when_absent() {
        let runner = MockRunner::new().with_output(0, "id 1\n    node.name = \"other\"\n", "");
        let mut be = AudioBackend::new(runner, spec());
        be.remove().unwrap();
        assert_eq!(be.runner().calls.len(), 1); // only the existence check
    }

    #[test]
    fn remove_when_present_destroys_exact_node_id() {
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_SINK, "") // sink_exists (ls Node)
            .with_output(0, LS_WITH_SINK, "") // find_node_id (ls Node)
            .with_output(0, "", "") // pw-cli destroy
            .with_output(0, "", ""); // pkill -f <conf> (best-effort)
        let mut be = AudioBackend::new(runner, spec());
        be.remove().unwrap();
        let calls = &be.runner().calls;
        // First two calls: existence check + id lookup (both ls Node)
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
        assert_eq!(calls[1], vec!["pw-cli", "ls", "Node"]);
        // Third call: exact destroy argv with the id from the fixture (57)
        assert_eq!(calls[2], vec!["pw-cli", "destroy", "57"]);
        // Fourth call: best-effort pkill to stop the spawned pipewire instance
        assert_eq!(calls[3][0], "pkill");
        assert_eq!(calls[3][1], "-f");
        assert!(calls[3][2].ends_with("arctis_eq.conf"));
    }

    #[test]
    fn recreate_tears_down_then_creates_with_new_target() {
        // remove(): sink_exists ls (present) → find_node_id ls → destroy → pkill
        // create(): sink_exists ls (now absent) → spawn_owned pipewire -c
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_SINK, "") // remove: sink_exists
            .with_output(0, LS_WITH_SINK, "") // remove: find_node_id
            .with_output(0, "", "") // remove: destroy
            .with_output(0, "", "") // remove: pkill
            .with_output(0, "id 1\n    node.name = \"x\"\n", ""); // create: sink_exists (absent)
        let spec = SinkSpec {
            node_name: "arctis_eq".into(),
            description: "Arctis EQ Sink".into(),
            channels: crate::config::ChainChannels::Stereo,
            playback_target: Some("alsa_output.speakers".into()),
        };
        let mut be = AudioBackend::new(runner, spec);
        let handle = be.recreate(&EqModel::default_10band()).unwrap();
        let calls = &be.runner().calls;
        assert_eq!(calls[2], vec!["pw-cli", "destroy", "57"]);
        // calls: ls, ls, destroy, pkill, ls-absent = 5 entries
        assert_eq!(calls.len(), 5, "5 run calls: ls ls destroy pkill ls-absent");
        // The fresh pipewire instance is in spawned (not calls).
        let spawned = &be.runner().spawned;
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0][0], "pipewire");
        assert!(spawned[0][2].ends_with("arctis_eq.conf"));
        // Handle carries the new child token.
        assert!(
            handle.child.is_some(),
            "recreate must surface the new child token"
        );
    }

    #[test]
    fn apply_volume_mute_emits_exact_pw_cli_props_argv() {
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_SINK, "") // find_node_id
            .with_output(0, "", ""); // the set
        let mut be = AudioBackend::new(runner, spec());
        be.apply_volume_mute(-6.0, false).unwrap();
        let last = be.runner().last_call().unwrap();
        let linear = 10f32.powf(-6.0_f32 / 20.0);
        let expected_vol = if linear.fract() == 0.0 {
            format!("{linear:.1}")
        } else {
            format!("{linear}")
        };
        let expected_json =
            format!("{{ channelVolumes = [ {expected_vol} {expected_vol} ] mute = false }}");
        assert_eq!(
            last,
            &vec![
                "pw-cli".to_string(),
                "s".to_string(),
                "57".to_string(),
                "Props".to_string(),
                expected_json,
            ]
        );
    }

    #[test]
    fn apply_volume_mute_muted_sends_mute_true() {
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_SINK, "")
            .with_output(0, "", "");
        let mut be = AudioBackend::new(runner, spec());
        be.apply_volume_mute(0.0, true).unwrap();
        let last = be.runner().last_call().unwrap();
        assert!(last.last().unwrap().contains("mute = true"));
    }

    #[test]
    fn apply_volume_mute_pct_sizes_channelvolumes_to_sink_channel_count() {
        // A surround-routed channel sink is 8-channel; the volume must be applied to all
        // 8 channelVolumes entries. A hardcoded 2-entry array would leave 6 channels at
        // unity, so the volume slider would only attenuate the front pair (bug H1).
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_SINK, "") // find_node_id
            .with_output(0, "", ""); // the set
        let spec = SinkSpec {
            node_name: "arctis_eq".into(),
            description: "Arctis EQ Sink".into(),
            channels: crate::config::ChainChannels::Surround71,
            playback_target: None,
        };
        let mut be = AudioBackend::new(runner, spec);
        be.apply_volume_mute_pct(50, false).unwrap();
        let props = be.runner().last_call().unwrap().last().unwrap().clone();
        // pct=50 perceptual → 0.5^3 = 0.125 per channel; expect 8 entries.
        assert_eq!(
            props.matches("0.125").count(),
            8,
            "8-channel sink must emit 8 channelVolumes entries, got: {props}"
        );
    }

    #[test]
    fn apply_volume_mute_pct_uses_cubic_perceptual_scale() {
        // pct=50 (perceptual) → raw linear channelVolumes 0.125 (=0.5^3), matching wpctl.
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_SINK, "") // find_node_id
            .with_output(0, "", ""); // the set
        let mut be = AudioBackend::new(runner, spec());
        be.apply_volume_mute_pct(50, false).unwrap();
        let last = be.runner().last_call().unwrap();
        assert_eq!(
            last.last().unwrap(),
            "{ channelVolumes = [ 0.125 0.125 ] mute = false }",
            "pct=50 must apply cubic linear 0.125, not raw-linear 0.5"
        );
    }

    #[test]
    fn apply_volume_mute_pct_read_round_trips_perceptual() {
        // Write→read inverse: apply_volume_mute_pct(P) emits channelVolumes = (P/100)^3;
        // parse_node_volume reads cbrt(channelVolumes)*100 back to P (±1 rounding).
        use crate::sinks::parse_node_volume;
        for p in [0u8, 8, 25, 50, 100] {
            let runner = MockRunner::new()
                .with_output(0, LS_WITH_SINK, "") // find_node_id
                .with_output(0, "", ""); // the set
            let mut be = AudioBackend::new(runner, spec());
            be.apply_volume_mute_pct(p, false).unwrap();
            // Extract the raw linear value the backend emitted.
            let json = be.runner().last_call().unwrap().last().unwrap().clone();
            let inner = json
                .split("[ ")
                .nth(1)
                .and_then(|s| s.split(' ').next())
                .unwrap();
            let linear: f64 = inner.parse().unwrap();
            // Feed it back through the live-read parser via a synthetic pw-dump.
            let dump = format!(
                "[{{\"type\":\"PipeWire:Interface:Node\",\"id\":1,\"info\":{{\"props\":{{\"node.name\":\"n\"}},\"params\":{{\"Props\":[{{\"channelVolumes\":[{linear},{linear}],\"mute\":false}}]}}}}}}]"
            );
            let read = parse_node_volume(&dump, "n").unwrap();
            let diff = (read as i16 - p as i16).abs();
            assert!(diff <= 1, "round-trip pct {p} → linear {linear} → read {read} (diff {diff})");
        }
    }

    #[test]
    fn apply_all_issues_props_set_per_band() {
        let band0 = EqBand::new(BandKind::Peaking, 100.0, 1.0, 2.0);
        let band1 = EqBand::new(BandKind::Peaking, 1000.0, 1.0, -3.0);
        let band2 = EqBand::new(BandKind::Peaking, 10000.0, 1.0, 0.0);
        let model = EqModel {
            bands: vec![band0, band1, band2],
        };
        let runner = MockRunner::new()
            .with_output(0, LS_WITH_SINK, "") // find_node_id
            .with_output(0, "", "") // band 0 set
            .with_output(0, "", "") // band 1 set
            .with_output(0, "", ""); // band 2 set
        let mut be = AudioBackend::new(runner, spec());
        be.apply_all(&model).unwrap();
        let calls = &be.runner().calls;
        // First call: find_node_id ls Node
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
        // Exactly 3 band set calls follow (one per band)
        assert_eq!(calls.len(), 4);
        // First band argv: ["pw-cli", "s", "57", "Props", "<payload>"]
        let expected_band0_payload =
            "{ params = [ \"eq_band_0:Freq\" 100.0 \"eq_band_0:Q\" 1.0 \"eq_band_0:Gain\" 2.0 ] }";
        assert_eq!(
            calls[1],
            vec!["pw-cli", "s", "57", "Props", expected_band0_payload,]
        );
        // Last band argv
        let expected_band2_payload =
            "{ params = [ \"eq_band_2:Freq\" 10000.0 \"eq_band_2:Q\" 1.0 \"eq_band_2:Gain\" 0.0 ] }";
        assert_eq!(
            calls[3],
            vec!["pw-cli", "s", "57", "Props", expected_band2_payload,]
        );
    }
}
