use serde::Deserialize;

use crate::{
    error::ConfigError,
    schema::{
        ChannelConfig, Config, MicChainConfig, Profile, RouteConfig, SurroundConfig,
        CURRENT_VERSION,
    },
};

use arctis_domain::db_to_volume_pct;

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

    // version == 1: migrate v1 → v2
    if version == 1 {
        return migrate_v1(raw);
    }

    // version == 0: migrate v0 → current
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
        master_volume_pct: 100,
        master_mute: false,
        chatmix_position: 4,
        default_sink_channel: None,
    };

    let mut cfg = Config {
        version: CURRENT_VERSION,
        active_profile: doc.active_profile,
        profiles: vec![profile],
        eq_presets: Vec::new(),
        dial_controls_balance: true,
    };
    backfill_volume_pct(&mut cfg);
    Ok(cfg)
}

/// Migrate a v1 document to CURRENT_VERSION (v2).
///
/// v1 → v2: fill `volume_pct` and `master_volume_pct` from the existing `volume_db` /
/// `master_volume_db` fields using the canonical `db_to_volume_pct` formula.
/// `mic.volume_pct` has no predecessor dB field, so it keeps the serde default (100).
fn migrate_v1(raw: &str) -> Result<Config, ConfigError> {
    let mut cfg: Config = toml::from_str(raw).map_err(|e| ConfigError::Parse(e.to_string()))?;
    cfg.version = CURRENT_VERSION;
    backfill_volume_pct(&mut cfg);
    Ok(cfg)
}

/// Backfill `volume_pct` / `master_volume_pct` from the legacy dB fields for every profile.
///
/// Called by both v0 and v1 migration paths so the logic lives in one place.
fn backfill_volume_pct(cfg: &mut Config) {
    for profile in &mut cfg.profiles {
        for channel in &mut profile.channels {
            channel.volume_pct = db_to_volume_pct(channel.volume_db);
        }
        profile.master_volume_pct = db_to_volume_pct(profile.master_volume_db);
        // mic.volume_pct: no old dB field exists; serde default (100) is already in place.
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// An old v1 config with volume_db = −6.0 on a channel must migrate to volume_pct ≈ 50.
    #[test]
    fn migrate_v1_fills_volume_pct_from_volume_db() {
        let toml_str = r#"
version = 1
active_profile = "default"

[[profiles]]
name = "default"
master_volume_db = -6.0

[[profiles.channels]]
id = "game"
node_name = "Arctis_Game"
description = "Game"
volume_db = -6.0

[[profiles.channels]]
id = "chat"
node_name = "Arctis_Chat"
description = "Chat"
"#;
        let cfg = migrate_str(toml_str).expect("migrate_str should succeed for v1 doc");
        assert_eq!(
            cfg.version, CURRENT_VERSION,
            "migrated config must have version == CURRENT_VERSION"
        );
        let profile = cfg.active().expect("active profile");
        assert_eq!(
            profile.channels[0].volume_pct, 50,
            "channel volume_pct must be ~50 for volume_db = -6.0 dB"
        );
        // chat channel has no volume_db (defaults to 0.0) → should map to 100
        assert_eq!(
            profile.channels[1].volume_pct, 100,
            "channel with default volume_db (0.0) must map to volume_pct = 100"
        );
        assert_eq!(
            profile.master_volume_pct, 50,
            "master_volume_pct must be ~50 for master_volume_db = -6.0 dB"
        );
    }

    /// A v2 config with explicit volume_pct values must round-trip unchanged through migrate_str.
    #[test]
    fn migrate_current_version_round_trips_volume_pct() {
        let toml_str = r#"
version = 2
active_profile = "default"

[[profiles]]
name = "default"
master_volume_pct = 75

[[profiles.channels]]
id = "game"
node_name = "Arctis_Game"
description = "Game"
volume_pct = 75
"#;
        let cfg = migrate_str(toml_str).expect("migrate_str should succeed for v2 doc");
        let profile = cfg.active().expect("active profile");
        assert_eq!(profile.channels[0].volume_pct, 75);
        assert_eq!(profile.master_volume_pct, 75);
    }
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
