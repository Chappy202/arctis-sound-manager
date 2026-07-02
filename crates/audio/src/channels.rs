use crate::backend::{AudioBackend, ConfHandle};
use crate::config::SinkSpec;
use crate::eq::EqModel;
use crate::error::AudioError;
use crate::runner::ChildToken;
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
    /// Channel layout of this channel's sink. Defaults to `Stereo`; set to
    /// `Surround71` when the channel is surround-routed so a game outputs discrete
    /// 7.1 into it (which then feeds the HRIR convolver).
    pub channels: crate::config::ChainChannels,
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
            channels: crate::config::ChainChannels::Stereo,
        }
    }

    /// Map to the existing single-sink `SinkSpec` (G1 reuse).
    pub fn sink_spec(&self) -> SinkSpec {
        SinkSpec {
            node_name: self.node_name.clone(),
            description: self.description.clone(),
            channels: self.channels,
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
    ///
    /// Returns a `(ConfHandle, Option<ChildToken>)` pair per channel. The token is
    /// `Some` when a new `pipewire -c` instance was spawned for that channel;
    /// `None` when the sink was already present. The caller (engine) must call
    /// `children.track(token)` for each `Some` to ensure shutdown reaps it.
    pub fn up(
        &mut self,
        eq: &EqModel,
    ) -> Result<Vec<(ConfHandle, Option<ChildToken>)>, AudioError> {
        let mut handles = Vec::with_capacity(self.config.channels.len());
        for ch in &self.config.channels {
            let spec = ch.sink_spec();
            let mut be = AudioBackend::new(&mut self.runner, spec);
            let handle = be.create(eq)?;
            let token = handle.child.clone();
            handles.push((handle, token));
        }
        Ok(handles)
    }

    /// Change a channel's output device and ENFORCE it: update the stored
    /// definition, then rebuild that channel's sink so its playback target is
    /// actually rewired (fixes the old dead selector). `output_device = None`
    /// returns the channel to following the default sink.
    pub fn set_output(
        &mut self,
        channel_id: &str,
        output_device: Option<String>,
        eq: &EqModel,
    ) -> Result<ConfHandle, AudioError> {
        let ch = self
            .config
            .channels
            .iter_mut()
            .find(|c| c.id == channel_id)
            .ok_or_else(|| AudioError::Invalid(format!("unknown channel: {channel_id}")))?;
        ch.output_device = output_device;
        let spec = ch.sink_spec();
        let mut be = AudioBackend::new(&mut self.runner, spec);
        be.recreate(eq)
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
        // For each of 3 channels, create() runs: ls Node (absent) + spawn_owned.
        // spawn_owned goes to `spawned`, NOT `calls` — only ls checks consume queued outputs.
        let runner = MockRunner::new()
            .with_output(0, "id 1\n    node.name = \"x\"\n", "") // game ls
            .with_output(0, "id 1\n    node.name = \"x\"\n", "") // chat ls
            .with_output(0, "id 1\n    node.name = \"x\"\n", ""); // media ls
        let mut mgr = ChannelManager::new(runner, cfg());
        let pairs = mgr.up(&EqModel::default_10band()).unwrap();
        assert_eq!(pairs.len(), 3);
        // Each pair has a Some token (new spawn per channel).
        for (_, token) in &pairs {
            assert!(token.is_some(), "each channel spawn must yield a token");
        }
        // `calls` only has the 3 ls-Node existence checks.
        let calls = &mgr.runner().calls;
        assert_eq!(calls.len(), 3, "only ls-Node calls go through run");
        assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
        // `spawned` has the 3 pipewire -c invocations.
        let spawned = &mgr.runner().spawned;
        assert_eq!(spawned.len(), 3);
        assert_eq!(spawned[0][0], "pipewire");
        assert!(spawned[0][2].ends_with("Arctis_Game.conf"));
        assert!(spawned[2][2].ends_with("Arctis_Media.conf"));
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
        let pairs = mgr.up(&EqModel::default_10band()).unwrap();
        // 3 ls checks only; no spawns.
        assert_eq!(mgr.runner().calls.len(), 3);
        assert!(mgr
            .runner()
            .calls
            .iter()
            .all(|c| c == &vec!["pw-cli", "ls", "Node"]));
        // All tokens are None — sinks were already present.
        assert!(
            pairs.iter().all(|(_, t)| t.is_none()),
            "no child tokens when sinks present"
        );
        assert!(
            mgr.runner().spawned.is_empty(),
            "no spawn_owned when all present"
        );
    }

    #[test]
    fn set_output_updates_def_and_rebuilds_channel() {
        // Scrub any stale conf so the diff-before-recreate guard cannot skip
        // the scripted teardown+respawn.
        let _ = std::fs::remove_file(std::env::temp_dir().join("arctis_eq.Arctis_Media.conf"));
        // remove() path (sink present) then create() path (absent), as in recreate.
        let present_media = "id 12\n    node.name = \"Arctis_Media\"\n";
        let runner = MockRunner::new()
            .with_output(0, present_media, "") // remove: sink_exists (present)
            .with_output(0, present_media, "") // remove: find_node_id
            .with_output(0, "", "") // remove: destroy
            .with_output(0, "", "") // remove: pkill
            .with_output(0, "id 1\n    node.name = \"x\"\n", ""); // create: absent
        let mut mgr = ChannelManager::new(runner, cfg());
        mgr.set_output(
            "media",
            Some("alsa_output.speakers".into()),
            &EqModel::default_10band(),
        )
        .unwrap();
        // The stored def now carries the new device (enforced, not just stored).
        assert_eq!(
            mgr.find("media").unwrap().output_device.as_deref(),
            Some("alsa_output.speakers")
        );
        // A fresh instance was spawned with the Media conf via spawn_owned → in `spawned`.
        let spawned = &mgr.runner().spawned;
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0][0], "pipewire");
        assert!(spawned[0][2].ends_with("Arctis_Media.conf"));
    }

    #[test]
    fn set_output_unknown_channel_errors() {
        let runner = MockRunner::new();
        let mut mgr = ChannelManager::new(runner, cfg());
        let err = mgr
            .set_output("nope", None, &EqModel::default_10band())
            .unwrap_err();
        assert!(matches!(err, AudioError::Invalid(_)));
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
