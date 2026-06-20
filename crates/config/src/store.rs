use std::{
    fs::{self, File},
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::{
    error::ConfigError,
    migrate::{import_routes_json, migrate_str},
    schema::Config,
};

// ── path resolution ───────────────────────────────────────────────────────────

/// Resolve the config directory.
///
/// If the environment variable `ASM_CONFIG_HOME` is set, use it; otherwise fall back to
/// `$HOME/.config/arctis-sound-manager`.
pub fn config_dir() -> PathBuf {
    if let Ok(val) = std::env::var("ASM_CONFIG_HOME") {
        PathBuf::from(val)
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        PathBuf::from(home)
            .join(".config")
            .join("arctis-sound-manager")
    }
}

/// `<config_dir>/config.toml`
pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// `<config_dir>/routes.json`
pub fn legacy_routes_path() -> PathBuf {
    config_dir().join("routes.json")
}

// ── I/O ───────────────────────────────────────────────────────────────────────

/// Load config from an explicit path.
///
/// * File present → run `migrate_str` on its contents.
/// * File absent  → build `Config::default_config()`, then try to import `routes.json` from the
///   same directory.
pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
    if !path.exists() {
        // File absent: use defaults and import routes if present.
        let mut cfg = Config::default_config();
        let routes_path = path.parent().unwrap_or(Path::new(".")).join("routes.json");
        if let Some(profile_name) = Some(cfg.active_profile.clone()) {
            if let Some(profile) = cfg.profile_mut(&profile_name) {
                import_routes_json(profile, &routes_path)?;
            }
        }
        return Ok(cfg);
    }

    let raw = fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        source_msg: e.to_string(),
    })?;

    migrate_str(&raw)
}

/// Load from the resolved `config_path()`.
pub fn load() -> Result<Config, ConfigError> {
    load_from(&config_path())
}

/// Atomically write the config:
/// 1. Validate the config.
/// 2. Serialize to TOML.
/// 3. Write to `<path>.tmp`.
/// 4. `fsync` the temp file.
/// 5. `rename` temp → target (atomic on Linux).
///
/// Creates parent directories as needed.
pub fn save_to(path: &Path, cfg: &Config) -> Result<(), ConfigError> {
    cfg.validate()?;

    let toml_str =
        toml::to_string_pretty(cfg).map_err(|e| ConfigError::Serialize(e.to_string()))?;

    // Ensure parent directory exists.
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
            path: parent.display().to_string(),
            source_msg: e.to_string(),
        })?;
    }

    // Build the .tmp sibling path.
    let tmp_path = {
        let mut p = path.as_os_str().to_owned();
        p.push(".tmp");
        PathBuf::from(p)
    };

    // Write to .tmp
    {
        let mut file = File::create(&tmp_path).map_err(|e| ConfigError::Io {
            path: tmp_path.display().to_string(),
            source_msg: e.to_string(),
        })?;
        file.write_all(toml_str.as_bytes())
            .map_err(|e| ConfigError::Io {
                path: tmp_path.display().to_string(),
                source_msg: e.to_string(),
            })?;
        file.sync_all().map_err(|e| ConfigError::Io {
            path: tmp_path.display().to_string(),
            source_msg: e.to_string(),
        })?;
    }

    // Atomic rename .tmp → target
    fs::rename(&tmp_path, path).map_err(|e| ConfigError::Io {
        path: path.display().to_string(),
        source_msg: e.to_string(),
    })?;

    Ok(())
}

/// Save to the resolved `config_path()`.
pub fn save(cfg: &Config) -> Result<(), ConfigError> {
    save_to(&config_path(), cfg)
}
