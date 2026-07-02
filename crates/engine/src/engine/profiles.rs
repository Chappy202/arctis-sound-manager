//! Profile CRUD, switching, factory templates, import/export.
use super::*;

impl<R: CommandRunner> Engine<R> {
    /// Switch active profile in config, persist, then reconcile the graph to it.
    pub fn switch_profile(&mut self, name: &str) -> Result<(), EngineError> {
        // Validate first (no disk write on error)
        self.config.switch_profile(name)?;
        // Persist
        self.save_config()?;
        // Reconcile to the new profile
        self.reconcile()?;
        // Emit event
        self.emit(Event::ProfileSwitched {
            name: name.to_string(),
        });
        Ok(())
    }

    /// Create a new profile by cloning the currently active one under `name`,
    /// make it active, persist the config, reconcile the graph to it, and emit
    /// a `ProfileCreated` event.
    pub fn new_profile(&mut self, name: &str) -> Result<(), EngineError> {
        // new_profile_from_active validates (errors on duplicate name), clones, sets active
        self.config
            .new_profile_from_active(name)
            .map_err(EngineError::Config)?;
        // Persist
        self.save_config()?;
        // Reconcile to the new (identical) profile
        self.reconcile()?;
        // Emit event
        self.emit(Event::ProfileCreated {
            name: name.to_string(),
        });
        Ok(())
    }

    /// Create a factory profile from a named template, make it active, persist,
    /// reconcile the graph, and emit a `ProfileCreated` event.
    ///
    /// Templates come from the data-driven catalog in
    /// [`factory_profiles::factory_profiles`] and are matched case-insensitively;
    /// the matched [`factory_profiles::FactoryProfileSpec`] is layered onto a clone
    /// of the active profile (hardware settings preserved). Unknown templates
    /// return `EngineError::BadRequest`.
    pub fn create_factory_profile(&mut self, template: &str) -> Result<(), EngineError> {
        let spec = crate::factory_profiles::find_factory_profile(template)
            .ok_or_else(|| EngineError::BadRequest(format!("unknown factory profile template: {template}")))?;
        let active = self.config.active()?.clone();
        let p = crate::factory_profiles::apply_factory_spec(&active, spec)?;
        let name = p.name.clone();
        self.config.upsert_profile(p);
        self.config.active_profile = name.clone();
        self.save_config()?;
        self.reconcile()?;
        self.emit(Event::ProfileCreated { name });
        Ok(())
    }

    /// List the factory-profile catalog as serializable info for the UI.
    pub fn list_factory_profiles(&self) -> Vec<crate::factory_profiles::FactoryProfileInfo> {
        crate::factory_profiles::factory_profiles()
            .iter()
            .map(|s| crate::factory_profiles::FactoryProfileInfo {
                name: s.name.to_string(),
                hrir: s.hrir_stem.map(|h| h.to_string()),
                mode: surround_mode_str(s.mode).to_string(),
            })
            .collect()
    }

    /// Rename a profile. If it was the active profile, also updates `active_profile`.
    /// Saves config and emits `ProfileRenamed`.
    pub fn rename_profile(&mut self, old: &str, new: &str) -> Result<(), EngineError> {
        // Delegate validation to config layer
        self.config.rename_profile(old, new)?;
        // If the renamed profile was active, keep active_profile in sync
        if self.config.active_profile == old {
            self.config.active_profile = new.to_string();
        }
        self.save_config()?;
        self.emit(Event::ProfileRenamed {
            old: old.to_string(),
            new: new.to_string(),
        });
        Ok(())
    }

    /// Delete a profile. Saves config and emits `ProfileDeleted`.
    pub fn delete_profile(&mut self, name: &str) -> Result<(), EngineError> {
        self.config.delete_profile(name)?;
        self.save_config()?;
        self.emit(Event::ProfileDeleted {
            name: name.to_string(),
        });
        Ok(())
    }

    /// Export a profile by name as a TOML string. Read-only — no persist, no event.
    pub fn export_profile(&self, name: &str) -> Result<String, EngineError> {
        let profile = self
            .config
            .profile(name)
            .ok_or_else(|| EngineError::BadRequest(format!("profile not found: {name}")))?;
        toml::to_string(profile)
            .map_err(|e| EngineError::BadRequest(format!("serialize profile: {e}")))
    }

    /// Import a profile from a TOML string. Resolves name collisions by appending
    /// "-imported", then "-imported(2)", "-imported(3)", etc. until unique.
    /// Validates the config after insertion. Returns the resolved name.
    pub fn import_profile(&mut self, toml_str: &str) -> Result<String, EngineError> {
        let mut profile: arctis_config::Profile = toml::from_str(toml_str)
            .map_err(|e| EngineError::BadRequest(format!("invalid profile TOML: {e}")))?;

        // Resolve name collision
        let base_name = profile.name.clone();
        let resolved_name = if self.config.profile(&base_name).is_none() {
            base_name.clone()
        } else {
            let candidate = format!("{base_name}-imported");
            if self.config.profile(&candidate).is_none() {
                candidate
            } else {
                let mut n = 2u32;
                loop {
                    if n > 1000 {
                        return Err(EngineError::BadRequest(
                            "too many name collisions for imported profile".to_string(),
                        ));
                    }
                    let candidate = format!("{base_name}-imported({n})");
                    if self.config.profile(&candidate).is_none() {
                        break candidate;
                    }
                    n += 1;
                }
            }
        };

        profile.name = resolved_name.clone();
        self.config.upsert_profile(profile);
        // Validate the config after insertion
        self.config.validate()?;
        self.save_config()?;
        self.emit(Event::ProfileImported {
            name: resolved_name.clone(),
        });
        Ok(resolved_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;

    #[test]
    fn create_factory_profile_unknown_template_errors() {
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        let err = engine
            .create_factory_profile("not-a-game")
            .expect_err("unknown template must error");
        assert!(
            matches!(err, EngineError::BadRequest(_)),
            "unknown template must be BadRequest, got: {err:?}"
        );
    }

    #[test]
    fn switch_profile_persists_and_reconciles() {
        // Seed a 2-profile config
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles.push(Profile {
            name: "gaming".into(),
            channels: vec![
                ChannelConfig {
                    id: "game".into(),
                    node_name: "Arctis_Game".into(),
                    description: "Game".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
                ChannelConfig {
                    id: "chat".into(),
                    node_name: "Arctis_Chat".into(),
                    description: "Chat".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
                ChannelConfig {
                    id: "media".into(),
                    node_name: "Arctis_Media".into(),
                    description: "Media".into(),
                    output_device: None,
                    eq: vec![],
                    volume_db: 0.0,
                    volume_pct: 100,
                    muted: false,
                },
            ],
            routes: vec![],
            mic: MicChainConfig::default(),
            surround: arctis_config::SurroundConfig::default(),
            master_volume_db: 0.0,
            master_volume_pct: 100,
            master_mute: false,
            chatmix_position: 4,
            default_sink_channel: None,
        });

        // Use a temp ASM_CONFIG_HOME so we don't touch real config.
        // Serialize all env-var-touching tests via mutex.
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("switch");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        // Queue outputs for one reconcile pass
        let runner = queue_reconcile_present(MockRunner::new());

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        engine
            .switch_profile("gaming")
            .expect("switch_profile should succeed");

        // In-memory config updated
        assert_eq!(engine.config().active_profile, "gaming");

        // On-disk config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written on switch");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("active_profile = \"gaming\""),
            "persisted config must show gaming as active"
        );

        // MockRunner saw reconcile calls (ls Node for channels up)
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c == &vec!["pw-cli", "ls", "Node"]),
            "reconcile must issue pw-cli ls Node"
        );

        // Event received
        let event = rx.try_recv().expect("ProfileSwitched event must be sent");
        assert_eq!(
            event,
            crate::state::Event::ProfileSwitched {
                name: "gaming".to_string()
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn switch_unknown_errors_no_disk_write() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("switch_err");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        let result = engine.switch_profile("nope");
        assert!(
            matches!(result, Err(EngineError::Config(_))),
            "should error on unknown profile"
        );
        // No disk write should have happened
        assert!(
            !tmp.exists(),
            "config dir must not be created on failed switch"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn new_profile_creates_clones_active_persists_reconciles_emits_event() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("new_profile");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        // Queue outputs for one reconcile pass (new_profile calls reconcile)
        let runner = queue_reconcile_present(MockRunner::new());

        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = Engine::new(runner, cfg);
        engine.set_event_sink(tx);

        engine
            .new_profile("competitive")
            .expect("new_profile should succeed");

        // New profile created and active
        assert_eq!(engine.config().active_profile, "competitive");
        let names = engine.config().profile_names();
        assert!(
            names.contains(&"default".to_string()),
            "original profile preserved"
        );
        assert!(
            names.contains(&"competitive".to_string()),
            "new profile exists"
        );

        // Config persisted
        let saved_path = tmp.join("config.toml");
        assert!(saved_path.exists(), "config.toml must be written");
        let saved_str = std::fs::read_to_string(&saved_path).unwrap();
        assert!(
            saved_str.contains("competitive"),
            "persisted config must contain new profile name"
        );

        // Reconcile was called (pw-cli ls Node issued)
        assert!(
            engine
                .runner
                .calls
                .iter()
                .any(|c| c == &vec!["pw-cli", "ls", "Node"]),
            "reconcile must be called after new_profile"
        );

        // Event emitted
        let event = rx.try_recv().expect("ProfileCreated event must be sent");
        assert_eq!(
            event,
            crate::state::Event::ProfileCreated {
                name: "competitive".to_string()
            }
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    #[test]
    fn new_profile_duplicate_name_errors_no_disk_write() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("new_profile_dup");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        let result = engine.new_profile("default"); // "default" already exists
        assert!(result.is_err(), "duplicate profile name must error");
        assert!(!tmp.exists(), "no disk write on error");

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }

    // ─────────────────────────────────────────────
    // TDD: F3 profile management — rename active, EQ preset unit tests
    // ─────────────────────────────────────────────

    #[test]
    fn rename_active_profile_updates_active_profile_field() {
        let _env_lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = unique_cfg_tmp("rename_active");
        std::env::set_var("ASM_CONFIG_HOME", &tmp);

        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);

        // Verify initial active profile
        assert_eq!(engine.state().active_profile, "default");

        engine
            .rename_profile("default", "my-renamed")
            .expect("rename_profile should succeed");

        // active_profile in state must reflect the new name
        assert_eq!(
            engine.state().active_profile,
            "my-renamed",
            "state().active_profile must be updated after renaming the active profile"
        );
        // Old name must be gone, new name must exist
        let names = engine.config().profile_names();
        assert!(
            !names.contains(&"default".to_string()),
            "old profile name must not exist"
        );
        assert!(
            names.contains(&"my-renamed".to_string()),
            "new profile name must exist"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        std::env::remove_var("ASM_CONFIG_HOME");
    }
}
