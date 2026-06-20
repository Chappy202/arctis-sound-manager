use crate::backend::{AudioBackend, ConfHandle};
use crate::config::SinkSpec;
use crate::eq::EqModel;
use crate::error::AudioError;
use crate::runner::CommandRunner;

/// One submix channel: a stable logical id, its PipeWire sink node.name,
/// a human description, and an optional pinned output device (hardware sink
/// node.name). `output_device = None` follows the default sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelDef {
    pub id: String,
    pub node_name: String,
    pub description: String,
    pub output_device: Option<String>,
}

impl ChannelDef {
    pub fn new(
        id: &str,
        node_name: &str,
        description: &str,
        output_device: Option<String>,
    ) -> Self {
        Self {
            id: id.to_string(),
            node_name: node_name.to_string(),
            description: description.to_string(),
            output_device,
        }
    }

    /// Map to the existing single-sink `SinkSpec` (G1 reuse).
    pub fn sink_spec(&self) -> SinkSpec {
        SinkSpec {
            node_name: self.node_name.clone(),
            description: self.description.clone(),
            playback_target: self.output_device.clone(),
        }
    }
}

/// The full set of channels managed together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelSetConfig {
    pub channels: Vec<ChannelDef>,
}

impl ChannelSetConfig {
    /// Sonar-mirroring default: Game / Chat / Media, each pinned to
    /// `hardware_sink` if given (else following the default sink).
    pub fn default_sonar(hardware_sink: Option<&str>) -> Self {
        let hw = hardware_sink.map(String::from);
        Self {
            channels: vec![
                ChannelDef::new("game", "Arctis_Game", "Arctis Game", hw.clone()),
                ChannelDef::new("chat", "Arctis_Chat", "Arctis Chat", hw.clone()),
                ChannelDef::new("media", "Arctis_Media", "Arctis Media", hw),
            ],
        }
    }

    pub fn find(&self, id: &str) -> Option<&ChannelDef> {
        self.channels.iter().find(|c| c.id == id)
    }
}

/// Manages the lifecycle of every channel sink by driving the existing
/// single-sink `AudioBackend` once per channel (G1 — no duplicated logic).
pub struct ChannelManager<R: CommandRunner> {
    runner: R,
    config: ChannelSetConfig,
}

impl<R: CommandRunner> ChannelManager<R> {
    pub fn new(runner: R, config: ChannelSetConfig) -> Self {
        Self { runner, config }
    }

    pub fn config(&self) -> &ChannelSetConfig {
        &self.config
    }

    #[cfg(test)]
    pub fn runner(&self) -> &R {
        &self.runner
    }

    pub fn find(&self, id: &str) -> Option<&ChannelDef> {
        self.config.find(id)
    }

    /// Create every channel sink idempotently. Reuses `AudioBackend::create`.
    pub fn up(&mut self, eq: &EqModel) -> Result<Vec<ConfHandle>, AudioError> {
        let mut handles = Vec::with_capacity(self.config.channels.len());
        for ch in &self.config.channels {
            let spec = ch.sink_spec();
            let mut be = AudioBackend::new(&mut self.runner, spec);
            handles.push(be.create(eq)?);
        }
        Ok(handles)
    }

    /// Remove every channel sink idempotently. Reuses `AudioBackend::remove`.
    pub fn down(&mut self) -> Result<(), AudioError> {
        for ch in &self.config.channels {
            let spec = ch.sink_spec();
            let mut be = AudioBackend::new(&mut self.runner, spec);
            be.remove()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::MockRunner;

    fn cfg() -> ChannelSetConfig {
        ChannelSetConfig::default_sonar(Some("alsa_output.arctis"))
    }

    #[test]
    fn default_sonar_has_three_named_channels() {
        let c = ChannelSetConfig::default_sonar(None);
        let names: Vec<&str> = c.channels.iter().map(|c| c.node_name.as_str()).collect();
        assert_eq!(names, vec!["Arctis_Game", "Arctis_Chat", "Arctis_Media"]);
        assert!(c.channels.iter().all(|c| c.output_device.is_none()));
    }

    #[test]
    fn sink_spec_maps_output_device_to_playback_target() {
        let ch = ChannelDef::new("media", "Arctis_Media", "Arctis Media", Some("spk".into()));
        let s = ch.sink_spec();
        assert_eq!(s.node_name, "Arctis_Media");
        assert_eq!(s.playback_target.as_deref(), Some("spk"));
    }

    #[test]
    fn up_creates_every_channel_when_absent() {
        // For each of 3 channels, create() runs: ls Node (absent) + spawn.
        let runner = MockRunner::new()
            .with_output(0, "id 1\n    node.name = \"x\"\n", "") // game ls
            .with_output(0, "", "") // game spawn
            .with_output(0, "id 1\n    node.name = \"x\"\n", "") // chat ls
            .with_output(0, "", "") // chat spawn
            .with_output(0, "id 1\n    node.name = \"x\"\n", "") // media ls
            .with_output(0, "", ""); // media spawn
        let mut mgr = ChannelManager::new(runner, cfg());
        let handles = mgr.up(&EqModel::default_10band()).unwrap();
        assert_eq!(handles.len(), 3);
        let calls = &mgr.runner().calls;
        // 3 channels × (1 ls + 1 spawn) = 6 calls.
        assert_eq!(calls.len(), 6);
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
        assert_eq!(calls[1][0], "pipewire");
        assert!(calls[1][2].ends_with("Arctis_Game.conf"));
        assert!(calls[5][2].ends_with("Arctis_Media.conf"));
    }

    #[test]
    fn up_is_idempotent_when_all_present() {
        // Each create() sees its sink already present → only the ls check runs.
        let present = "\
id 10\n    node.name = \"Arctis_Game\"\n\
id 11\n    node.name = \"Arctis_Chat\"\n\
id 12\n    node.name = \"Arctis_Media\"\n";
        let runner = MockRunner::new()
            .with_output(0, present, "")
            .with_output(0, present, "")
            .with_output(0, present, "");
        let mut mgr = ChannelManager::new(runner, cfg());
        mgr.up(&EqModel::default_10band()).unwrap();
        // 3 ls checks only; no spawns.
        assert_eq!(mgr.runner().calls.len(), 3);
        assert!(mgr
            .runner()
            .calls
            .iter()
            .all(|c| c == &vec!["pw-cli", "ls", "Node"]));
    }

    #[test]
    fn down_removes_every_channel_noop_when_absent() {
        // Each remove() sees its sink absent → only the existence check runs.
        let runner = MockRunner::new()
            .with_output(0, "id 1\n    node.name = \"other\"\n", "")
            .with_output(0, "id 1\n    node.name = \"other\"\n", "")
            .with_output(0, "id 1\n    node.name = \"other\"\n", "");
        let mut mgr = ChannelManager::new(runner, cfg());
        mgr.down().unwrap();
        assert_eq!(mgr.runner().calls.len(), 3);
    }
}
