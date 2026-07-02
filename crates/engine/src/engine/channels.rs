//! Channel lifecycle: add/remove channels and per-channel output devices.
use super::*;

impl<R: CommandRunner> Engine<R> {
    /// Set (or clear) the output device for a single channel in the active profile.
    ///
    /// Updates the in-memory config, persists it atomically, rebuilds that
    /// channel live via `ChannelManager::set_output`, tracks any new child token,
    /// and emits a `ChannelOutputSet` event.
    pub fn set_channel_output(
        &mut self,
        channel_id: &str,
        device: Option<String>,
    ) -> Result<(), EngineError> {
        // Validate channel exists before touching disk
        {
            let active_name = self.config.active_profile.clone();
            let profile = self.config.profile_mut(&active_name).ok_or_else(|| {
                EngineError::Config(arctis_config::ConfigError::ProfileNotFound(
                    active_name.clone(),
                ))
            })?;
            let channel = profile
                .channels
                .iter_mut()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found: {channel_id}"))
                })?;
            channel.output_device = device.clone();
        }
        // Persist
        self.save_config()?;
        // Apply live: rebuild that channel with the new output device
        {
            let profile = self.config.active()?.clone();
            let channel = profile
                .channels
                .iter()
                .find(|ch| ch.id == channel_id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found: {channel_id}"))
                })?;
            let eq_model = convert::eq_model_for(channel)?;
            let channel_set = convert::channel_set_from_profile(&profile);
            let mut mgr = ChannelManager::new(&mut self.runner, channel_set);
            let handle = mgr.set_output(channel_id, device.clone(), &eq_model)?;
            if let Some(t) = handle.child {
                self.children.track(t);
            }
        }
        // Emit event
        self.emit(Event::ChannelOutputSet {
            channel_id: channel_id.to_string(),
            device,
        });
        Ok(())
    }

    /// Add a new channel to the active profile with sane defaults.
    ///
    /// `id` must be non-empty, contain no whitespace or path separators, and not
    /// already exist in the active profile. `node_name` is derived as `"Arctis_<Title>"`
    /// and `description` as `"<id> audio channel"`. After adding to config and persisting,
    /// the new channel sink is brought up (reusing AudioBackend::create). Emits `ChannelAdded`.
    pub fn add_channel(&mut self, id: &str) -> Result<(), EngineError> {
        // Derive node_name and description from id
        let title = {
            let mut c = id.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        };
        let node_name = format!("Arctis_{title}");
        let description = format!("{id} audio channel");

        // Mutate config (validates id constraints)
        self.config
            .add_channel(id, &node_name, &description)
            .map_err(EngineError::Config)?;

        // Persist
        self.save_config()?;

        // Detect the headset hardware sink BEFORE the immutable borrow of
        // self.config.active() below so the borrow checker is satisfied.
        // Mirrors reconcile()'s overlay_default_output pattern: the stored
        // config keeps output_device=None so future reconcile passes can
        // re-detect the headset automatically; the overlay is applied only to
        // the local clone used to build the PipeWire filter-chain conf.
        let headset = self.detect_headset_sink();

        // Bring the new channel sink up (reuse channel-up path)
        {
            let profile = self.config.active()?.clone();
            let mut channel = profile
                .channels
                .iter()
                .find(|ch| ch.id == id)
                .ok_or_else(|| {
                    EngineError::BadRequest(format!("channel not found after add: {id}"))
                })?
                .clone();
            // Pin to the hardware sink so the new channel's output does not
            // follow the PipeWire default sink (which can be another virtual
            // channel, causing chain-through chaining).  output_device stays
            // None in the stored config so that apply_surround can override
            // the target when surround-routing is later enabled on this channel.
            if channel.output_device.is_none() {
                channel.output_device = headset;
            }
            let eq_model = convert::eq_model_for(&channel)?;
            let def = convert::channel_def_from_cfg(&channel);
            let spec = def.sink_spec();
            let mut be = arctis_audio::AudioBackend::new(&mut self.runner, spec);
            match be.create(&eq_model) {
                Ok(handle) => {
                    if let Some(token) = handle.child {
                        self.children.track(token);
                    }
                }
                Err(e) => {
                    eprintln!(
                        "warning: add_channel create sink for '{id}' failed (post-spawn race?): {e}"
                    );
                }
            }
        }

        self.emit(Event::ChannelAdded { id: id.to_string() });
        Ok(())
    }

    /// Remove a channel from the active profile.
    ///
    /// Errors if the channel does not exist or if it is the last remaining channel.
    /// Any channel may be removed, including game/chat/media.
    /// Routes referencing the removed channel become inert (no automatic cleanup).
    ///
    /// Tears down the channel's PipeWire sink (reusing AudioBackend::remove).
    /// Emits `ChannelRemoved`.
    pub fn remove_channel(&mut self, id: &str) -> Result<(), EngineError> {
        // Snapshot the channel def before removal (needed for teardown)
        let channel_def = {
            let profile = self.config.active()?;
            let channel = profile
                .channels
                .iter()
                .find(|ch| ch.id == id)
                .ok_or_else(|| EngineError::BadRequest(format!("channel not found: {id}")))?;
            convert::channel_def_from_cfg(channel)
        };

        // Mutate config (validates last-channel guard)
        self.config
            .remove_channel(id)
            .map_err(EngineError::Config)?;

        // Prune the removed channel from surround.channels so config doesn't reference a
        // deleted channel. Do this after the last-channel guard passes (above).
        {
            let active_name = self.config.active_profile.clone();
            if let Some(profile) = self.config.profile_mut(&active_name) {
                profile.surround.channels.retain(|ch| ch != id);
            }
        }

        // Persist
        self.save_config()?;

        // Tear down the channel's PipeWire sink (reuse channel-down path)
        {
            let spec = channel_def.sink_spec();
            let mut be = arctis_audio::AudioBackend::new(&mut self.runner, spec);
            if let Err(e) = be.remove() {
                eprintln!(
                    "warning: remove_channel sink teardown for '{id}' failed (ignoring): {e}"
                );
            }
        }

        self.emit(Event::ChannelRemoved { id: id.to_string() });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;

    #[test]
    fn set_channel_output_updates_config_persists_and_emits_event() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("set_ch_out");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();

        // set_channel_output calls ChannelManager::set_output which does:
        //   1. ls Node (find existing handle) + possibly spawn
        // Queue a present ls so set_output succeeds without spawn.
        let ls = ls_all_present();
        // set_output: ls Node to find channel + attempt to set output device
        // ChannelManager::set_output: ls to find node_id, then up + maybe spawn
        // When sinks are present, set_output does: ls (find) → present → no new spawn
        // But it re-creates the channel with new output, which means: ls (exists?) + spawn_owned
        // For simplicity, queue enough outputs so the operation can complete
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // ls for set_output
            .with_output(0, &ls, ""); // extra ls if needed

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        engine
            .set_channel_output("game", Some("alsa_output.speakers".to_string()))
            .expect("set_channel_output should succeed");

        // In-memory config updated
        let active = engine.config().active().unwrap();
        let game_ch = active.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(
            game_ch.output_device,
            Some("alsa_output.speakers".to_string()),
            "in-memory output_device must be updated"
        );

        // Config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("alsa_output.speakers"),
            "persisted config must contain the new output device"
        );

        // Event emitted
        let event = rx.try_recv().expect("ChannelOutputSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::ChannelOutputSet {
                channel_id: "game".to_string(),
                device: Some("alsa_output.speakers".to_string()),
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_channel_output_none_clears_device() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("set_ch_out_none");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Start with a channel that HAS an output device
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].channels[0].output_device = Some("alsa_output.old".into());

        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, &ls, "")
            .with_output(0, &ls, "");

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        engine
            .set_channel_output("game", None)
            .expect("set_channel_output(None) should succeed");

        let active = engine.config().active().unwrap();
        let game_ch = active.channels.iter().find(|c| c.id == "game").unwrap();
        assert_eq!(game_ch.output_device, None, "output_device must be cleared");

        let event = rx.try_recv().expect("ChannelOutputSet event must be sent");
        assert_eq!(
            event,
            crate::state::Event::ChannelOutputSet {
                channel_id: "game".to_string(),
                device: None,
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn set_channel_output_unknown_channel_errors() {
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let result = engine.set_channel_output("nonexistent", Some("some_device".into()));
        assert!(result.is_err(), "unknown channel_id must return an error");
    }

    // ─────────────────────────────────────────────
    // Minor fix tests: surround tracker + prune
    // ─────────────────────────────────────────────

    /// Fix: remove_channel prunes surround.channels and surround_routed stays consistent.
    ///
    /// Setup: config has channels = ["game", "chat", "media"], surround.channels = ["game", "media"].
    /// surround_routed = {"game", "media"} (simulating prior enable).
    ///
    /// After remove_channel("media"):
    ///   - surround.channels must not contain "media"
    ///   - surround_routed tracker is unaffected by remove_channel itself (reconcile handles it),
    ///     but a subsequent apply_surround driven from the new config must not try to route "media"
    ///     (it no longer exists in channels) and must restore it from surround_routed → tracker
    ///     drains "media" correctly.
    #[test]
    fn remove_channel_prunes_surround_channels_and_tracker_stays_consistent() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("rm_ch_surr_prune");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Config: surround enabled, surround.channels = ["game", "media"],
        // actual channels = ["game", "chat", "media"].
        let cfg = make_config_surround_enabled("unused-hrir"); // channels: game, chat, media; surround.channels: game, media

        // remove_channel("media") → AudioBackend::remove:
        //   sink_exists → 1 ls (present), find_node_id → 1 ls, destroy, pkill
        let ls = ls_all_present();
        let runner = MockRunner::new()
            .with_output(0, &ls, "") // sink_exists
            .with_output(0, &ls, "") // find_node_id
            .with_output(0, "", "") // pw-cli destroy
            .with_output(1, "", ""); // pkill (exit 1 — ignored)

        let mut engine = Engine::new(runner, cfg);
        // Simulate prior surround enable: both channels tracked.
        engine.surround_routed.insert("game".into());
        engine.surround_routed.insert("media".into());

        engine
            .remove_channel("media")
            .expect("remove_channel must succeed");

        // surround.channels must no longer reference the deleted channel.
        let surr_channels = &engine.config.active().unwrap().surround.channels;
        assert!(
            !surr_channels.contains(&"media".to_string()),
            "surround.channels must not reference deleted channel 'media': {surr_channels:?}"
        );
        assert!(
            surr_channels.contains(&"game".to_string()),
            "surround.channels must still contain 'game': {surr_channels:?}"
        );

        // The actual channels list must also not contain "media".
        let profile = engine.config.active().unwrap();
        assert!(
            !profile.channels.iter().any(|c| c.id == "media"),
            "channels list must not contain removed channel"
        );

        // surround_routed tracker: remove_channel doesn't touch it (reconcile/apply_surround
        // will clean it up on next pass). That is the correct contract — verify it's still
        // populated so a subsequent apply_surround can restore cleanly.
        assert!(
            engine.surround_routed.contains("media"),
            "surround_routed retains 'media' until next apply_surround pass"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Bug fix: add_channel must pin new channel output to headset hardware sink
    // ─────────────────────────────────────────────────────────────────────────

    /// Regression test: a channel created via `add_channel` must have its
    /// PipeWire filter-chain conf emitted with `target.object` pointing at the
    /// detected headset hardware sink, NOT left empty (which caused the new
    /// channel to follow the PipeWire default sink and chain through whatever
    /// virtual channel happened to be default at the time).
    ///
    /// MockRunner call sequence after the fix:
    ///   [0] pw-metadata 0  — detect_headset_sink → list_output_devices default
    ///   [1] pw-dump        — detect_headset_sink → list_output_devices sinks
    ///   [2] pw-cli ls Node — AudioBackend::create → sink_exists (empty → absent)
    ///   spawn: pipewire -c — spawn_owned (never queued; always succeeds)
    #[test]
    fn add_channel_pins_output_to_headset_sink() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("add_channel_pin");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let dump = include_str!("../../../audio/tests/fixtures/pw_dump_sinks.json");
        // Three queued run() outputs; spawn_owned is not from the queue.
        let runner = arctis_audio::MockRunner::new()
            .with_output(0, PW_METADATA_SINK, "") // [0] pw-metadata 0
            .with_output(0, dump, "")             // [1] pw-dump (SteelSeries present)
            .with_output(0, "", "");              // [2] pw-cli ls Node (empty → absent → spawn)

        let mut engine = Engine::new(runner, cfg);
        engine.add_channel("music").expect("add_channel must succeed");

        // The filter-chain conf written to /tmp must pin target.object to the
        // headset hardware sink node name found in the fixture.
        let conf_path = std::env::temp_dir().join("arctis_eq.Arctis_Music.conf");
        let conf = std::fs::read_to_string(&conf_path)
            .expect("filter-chain conf must be written by add_channel");
        assert!(
            conf.contains(
                "target.object = \"alsa_output.usb-SteelSeries_Arctis_Nova_Pro_Wireless-00.analog-stereo\""
            ),
            "new channel conf must pin to headset hardware sink, got:\n{conf}"
        );

        // Stored config must still have output_device=None so future reconcile
        // passes can re-detect (matching reconcile's overlay_default_output contract).
        let active = engine.config().active().expect("active profile");
        let stored_ch = active
            .channels
            .iter()
            .find(|c| c.id == "music")
            .expect("music channel must be in stored config");
        assert!(
            stored_ch.output_device.is_none(),
            "stored output_device must remain None (overlay is in-memory only), got: {:?}",
            stored_ch.output_device
        );

        let _ = std::fs::remove_file(&conf_path);
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }
}
