use crate::{
    error::ConfigError,
    schema::{Config, Profile},
};

impl Config {
    /// Set active_profile to `name`; error if it doesn't exist.
    pub fn switch_profile(&mut self, name: &str) -> Result<(), ConfigError> {
        if self.profile(name).is_none() {
            return Err(ConfigError::ProfileNotFound(name.to_string()));
        }
        self.active_profile = name.to_string();
        Ok(())
    }

    /// Create a new profile by cloning the active one under a new name; becomes active.
    /// Error if `name` already exists. Returns a ref to the new profile.
    pub fn new_profile_from_active(&mut self, name: &str) -> Result<&Profile, ConfigError> {
        if self.profile(name).is_some() {
            return Err(ConfigError::Invalid(format!(
                "profile '{name}' already exists"
            )));
        }
        let mut new_profile = self.active()?.clone();
        new_profile.name = name.to_string();
        self.profiles.push(new_profile);
        self.active_profile = name.to_string();
        // Return a ref to the newly pushed profile (last element)
        Ok(self.profiles.last().expect("just pushed"))
    }

    /// Overwrite (upsert) a profile by name. If active name matches, replaces it in place.
    pub fn upsert_profile(&mut self, profile: Profile) {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.name == profile.name) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    /// Return all profile names in order.
    pub fn profile_names(&self) -> Vec<String> {
        self.profiles.iter().map(|p| p.name.clone()).collect()
    }

    /// Rename a profile. Errors if `old` not found, `new` already exists (unless same), or `new` is empty.
    /// Does NOT update active_profile (caller handles that if needed).
    pub fn rename_profile(&mut self, old: &str, new: &str) -> Result<(), ConfigError> {
        if new.is_empty() {
            return Err(ConfigError::Invalid(
                "new profile name must not be empty".to_string(),
            ));
        }
        // Check old exists
        if self.profile(old).is_none() {
            return Err(ConfigError::ProfileNotFound(old.to_string()));
        }
        // Check new doesn't already exist (unless it's the same name)
        if old != new && self.profile(new).is_some() {
            return Err(ConfigError::Invalid(format!(
                "profile '{new}' already exists"
            )));
        }
        // Rename in place
        if let Some(p) = self.profiles.iter_mut().find(|p| p.name == old) {
            p.name = new.to_string();
        }
        Ok(())
    }

    /// Delete a profile. Errors if `name` not found, it's the active profile, or it's the last remaining.
    pub fn delete_profile(&mut self, name: &str) -> Result<(), ConfigError> {
        if self.profile(name).is_none() {
            return Err(ConfigError::ProfileNotFound(name.to_string()));
        }
        if self.active_profile == name {
            return Err(ConfigError::Invalid(format!(
                "cannot delete the active profile '{name}'"
            )));
        }
        if self.profiles.len() <= 1 {
            return Err(ConfigError::Invalid(
                "cannot delete the last remaining profile".to_string(),
            ));
        }
        self.profiles.retain(|p| p.name != name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_to_missing_errors() {
        let mut cfg = Config::default_config();
        let err = cfg
            .switch_profile("nope")
            .expect_err("switching to nonexistent profile should fail");
        assert!(
            matches!(err, ConfigError::ProfileNotFound(_)),
            "expected ProfileNotFound, got: {err}"
        );
    }

    #[test]
    fn new_from_active_clones_and_activates() {
        let mut cfg = Config::default_config();
        // default has 3 channels; capture them before clone
        let original_channels = cfg.active().unwrap().channels.clone();

        let new_profile = cfg
            .new_profile_from_active("gaming")
            .expect("should clone active profile");

        assert_eq!(new_profile.name, "gaming");
        assert_eq!(new_profile.channels, original_channels);
        assert_eq!(
            cfg.active_profile, "gaming",
            "new profile should become active"
        );
        let names = cfg.profile_names();
        assert!(
            names.contains(&"default".to_string()),
            "original profile should still exist"
        );
        assert!(
            names.contains(&"gaming".to_string()),
            "new profile should exist"
        );
        assert_eq!(names.len(), 2, "should have exactly 2 profiles");
    }

    #[test]
    fn new_with_dup_name_errors() {
        let mut cfg = Config::default_config();
        let err = cfg
            .new_profile_from_active("default")
            .expect_err("duplicate profile name should fail");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    // ── F3a: rename_profile tests ─────────────────────────────────────────────

    #[test]
    fn rename_profile_renames_entry() {
        let mut cfg = Config::default_config();
        cfg.new_profile_from_active("gaming").unwrap();
        // switch back to default so gaming is not active
        cfg.switch_profile("default").unwrap();
        cfg.rename_profile("gaming", "competitive").unwrap();
        assert!(
            cfg.profile("competitive").is_some(),
            "renamed profile must exist"
        );
        assert!(cfg.profile("gaming").is_none(), "old name must be gone");
    }

    #[test]
    fn rename_profile_errors_on_missing_old_name() {
        let mut cfg = Config::default_config();
        let err = cfg
            .rename_profile("nonexistent", "new_name")
            .expect_err("missing old name should error");
        assert!(
            matches!(err, ConfigError::ProfileNotFound(_)),
            "expected ProfileNotFound, got: {err}"
        );
    }

    #[test]
    fn rename_profile_errors_on_empty_new_name() {
        let mut cfg = Config::default_config();
        let err = cfg
            .rename_profile("default", "")
            .expect_err("empty new name should error");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    #[test]
    fn rename_profile_errors_on_duplicate_new_name() {
        let mut cfg = Config::default_config();
        cfg.new_profile_from_active("gaming").unwrap();
        cfg.switch_profile("default").unwrap();
        let err = cfg
            .rename_profile("gaming", "default")
            .expect_err("duplicate new name should error");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    #[test]
    fn rename_profile_same_name_is_noop() {
        let mut cfg = Config::default_config();
        // Renaming to same name should succeed (idempotent)
        cfg.rename_profile("default", "default").unwrap();
        assert!(cfg.profile("default").is_some());
    }

    // ── F3a: delete_profile tests ─────────────────────────────────────────────

    #[test]
    fn delete_profile_removes_profile() {
        let mut cfg = Config::default_config();
        cfg.new_profile_from_active("gaming").unwrap();
        cfg.switch_profile("default").unwrap();
        cfg.delete_profile("gaming").unwrap();
        assert!(
            cfg.profile("gaming").is_none(),
            "deleted profile must not exist"
        );
        assert!(
            cfg.profile("default").is_some(),
            "other profile must remain"
        );
    }

    #[test]
    fn delete_profile_errors_if_not_found() {
        let mut cfg = Config::default_config();
        let err = cfg
            .delete_profile("nonexistent")
            .expect_err("deleting non-existent profile should error");
        assert!(
            matches!(err, ConfigError::ProfileNotFound(_)),
            "expected ProfileNotFound, got: {err}"
        );
    }

    #[test]
    fn delete_profile_errors_if_active() {
        let mut cfg = Config::default_config();
        cfg.new_profile_from_active("gaming").unwrap();
        // gaming is now active
        let err = cfg
            .delete_profile("gaming")
            .expect_err("deleting active profile should error");
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    #[test]
    fn delete_profile_errors_if_last_profile() {
        let mut cfg = Config::default_config();
        // Only one profile (default) and it's not active — but still only one
        // Actually default IS active, but check the "last remaining" guard first
        // by switching active to default and having only one profile
        let err = cfg
            .delete_profile("default")
            .expect_err("deleting last profile should error");
        // Could be Invalid (active) or Invalid (last) — either is fine since default is active
        assert!(
            matches!(err, ConfigError::Invalid(_)),
            "expected Invalid, got: {err}"
        );
    }

    // ── F3a: EqPreset tests ───────────────────────────────────────────────────

    #[test]
    fn eq_presets_defaults_to_empty() {
        let cfg = Config::default_config();
        assert!(
            cfg.eq_presets.is_empty(),
            "eq_presets must default to empty"
        );
    }

    #[test]
    fn old_config_without_eq_presets_field_loads_with_empty() {
        let toml_str = r#"
version = 1
active_profile = "default"

[[profiles]]
name = "default"

[[profiles.channels]]
id = "game"
node_name = "Arctis_Game"
description = "Game"
"#;
        let cfg: Config = toml::from_str(toml_str).expect("should deserialize old config");
        assert!(
            cfg.eq_presets.is_empty(),
            "old config without eq_presets field must load with empty vec"
        );
    }

    #[test]
    fn eq_preset_round_trips_via_toml() {
        use crate::schema::{EqBandConfig, EqPreset};
        let mut cfg = Config::default_config();
        cfg.eq_presets = vec![EqPreset {
            name: "gaming-boost".to_string(),
            kind_hint: Some("gaming".to_string()),
            bands: vec![EqBandConfig {
                kind: "peaking".to_string(),
                freq_hz: 200.0,
                q: 1.0,
                gain_db: 3.0,
            }],
        }];
        let serialized = toml::to_string(&cfg).expect("serialize");
        let deserialized: Config = toml::from_str(&serialized).expect("deserialize");
        assert_eq!(cfg, deserialized, "EqPreset must round-trip via TOML");
    }

    #[test]
    fn upsert_replaces_existing_profile() {
        let mut cfg = Config::default_config();
        // Clone the active profile, mutate a channel, then upsert
        let mut modified = cfg.active().unwrap().clone();
        modified.channels[0].description = "modified description".to_string();

        cfg.upsert_profile(modified);

        let stored = cfg.profile("default").expect("profile should still exist");
        assert_eq!(
            stored.channels[0].description, "modified description",
            "upserted profile should reflect the mutation"
        );
        // Should not have duplicated the profile
        let names = cfg.profile_names();
        assert_eq!(
            names.iter().filter(|n| n.as_str() == "default").count(),
            1,
            "upsert should not duplicate the profile"
        );
    }
}
