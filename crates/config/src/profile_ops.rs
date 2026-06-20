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
