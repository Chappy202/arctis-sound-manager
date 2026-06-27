use serde::Deserialize;

use crate::{
    error::ConfigError,
    schema::{
        ChannelConfig, Config, MicChainConfig, Profile, RouteConfig, SurroundConfig,
        CURRENT_VERSION,
    },
};

// ── v0 schema helpers (private) ───────────────────────────────────────────────

/// Minimal representation of a v0 document (no `version`, no `profiles`).
#[derive(Debug, Deserialize)]
struct V0Doc {
    #[serde(default = "default_active_profile")]
    active_profile: String,
    #[serde(default)]
    channels: Vec<ChannelConfig>,
}

fn default_active_profile() -> String {
    "default".to_string()
}

/// Partial parse used only to read the `version` field (or absence thereof).
#[derive(Debug, Deserialize)]
struct VersionProbe {
    version: Option<u32>,
}

// ── public API ────────────────────────────────────────────────────────────────

/// Inspect raw TOML, read its `version` (absent/0 = v0), and migrate forward to CURRENT_VERSION.
///
/// * If the document already has `version == CURRENT_VERSION` and parses cleanly, return as-is.
/// * If the document has no `version` field (or `version = 0`), treat as v0: wrap its flat channel
///   list into a single "default" profile to produce a v1 `Config`.
pub fn migrate_str(raw: &str) -> Result<Config, ConfigError> {
    let probe: VersionProbe = toml::from_str(raw).map_err(|e| ConfigError::Parse(e.to_string()))?;

    let version = probe.version.unwrap_or(0);

    if version > CURRENT_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            found: version,
            max: CURRENT_VERSION,
        });
    }

    if version == CURRENT_VERSION {
        // Already at current — try to parse as the full Config.
        let cfg: Config = toml::from_str(raw).map_err(|e| ConfigError::Parse(e.to_string()))?;
        return Ok(cfg);
    }

    // version == 0: migrate v0 → v1
    migrate_v0(raw)
}

fn migrate_v0(raw: &str) -> Result<Config, ConfigError> {
    let doc: V0Doc = toml::from_str(raw).map_err(|e| ConfigError::Parse(e.to_string()))?;

    let profile = Profile {
        name: doc.active_profile.clone(),
        channels: doc.channels,
        routes: Vec::new(),
        mic: MicChainConfig::default(),
        surround: SurroundConfig::default(),
        master_volume_db: 0.0,
        master_mute: false,
        chatmix_position: 4,
        default_sink_channel: None,
    };

    Ok(Config {
        version: CURRENT_VERSION,
        active_profile: doc.active_profile,
        profiles: vec![profile],
        eq_presets: Vec::new(),
        dial_controls_balance: true,
    })
}

// ── routes.json import ────────────────────────────────────────────────────────

/// Shape of one entry in the Plan-4 routes.json file.
#[derive(Debug, Deserialize)]
struct LegacyRoute {
    app_binary: String,
    target_sink: String,
}

/// Import an existing routes.json (Plan 4 format: `[{ "app_binary", "target_sink" }, ...]`) into
/// the given profile's `routes`. Missing file => no-op Ok. Returns number of rules imported.
pub fn import_routes_json(
    profile: &mut crate::schema::Profile,
    routes_json_path: &std::path::Path,
) -> Result<usize, ConfigError> {
    if !routes_json_path.exists() {
        return Ok(0);
    }

    let raw = std::fs::read_to_string(routes_json_path).map_err(|e| ConfigError::Io {
        path: routes_json_path.display().to_string(),
        source_msg: e.to_string(),
    })?;

    let legacy: Vec<LegacyRoute> =
        serde_json::from_str(&raw).map_err(|e| ConfigError::Parse(e.to_string()))?;

    let count = legacy.len();
    for entry in legacy {
        profile.routes.push(RouteConfig {
            app_binary: entry.app_binary,
            target_sink: entry.target_sink,
        });
    }

    Ok(count)
}
