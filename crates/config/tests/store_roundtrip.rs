use std::fs;
use std::sync::Mutex;

use arctis_config::{
    migrate::migrate_str,
    schema::Config,
    store::{config_path, load, load_from, save, save_to},
};

/// Mutex to serialise the single env-based test that sets ASM_CONFIG_HOME.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

// ── helpers ──────────────────────────────────────────────────────────────────

fn routes_fixture() -> &'static str {
    include_str!("fixtures/routes.json")
}

fn v0_fixture() -> &'static str {
    include_str!("fixtures/config_v0.toml")
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[test]
fn save_then_load_roundtrips() {
    let _guard = ENV_MUTEX.lock().unwrap();
    let tmp = tempfile::tempdir().expect("tempdir");
    // Point ASM_CONFIG_HOME at the tempdir so config_path() resolves there.
    std::env::set_var("ASM_CONFIG_HOME", tmp.path());

    let cfg = Config::default_config();
    save(&cfg).expect("save should succeed");

    // The .tmp sibling must NOT remain after atomic rename.
    let p = config_path();
    let tmp_sibling = std::path::PathBuf::from(format!("{}.tmp", p.display()));
    assert!(
        !tmp_sibling.exists(),
        "temp file {tmp_sibling:?} should be cleaned up after atomic rename"
    );

    // Round-trip: loaded config equals what we saved.
    let loaded = load().expect("load should succeed");
    assert_eq!(cfg, loaded, "round-trip config must match");

    std::env::remove_var("ASM_CONFIG_HOME");
}

#[test]
fn load_absent_returns_default_and_imports_routes() {
    // Use load_from with an explicit tempdir — no env variable needed.
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg_path = tmp.path().join("config.toml");

    // Put routes.json in the same dir (same as what load_from looks at for import).
    let routes_dst = tmp.path().join("routes.json");
    fs::write(&routes_dst, routes_fixture()).expect("write fixture routes.json");

    // File is absent → should return default config with routes imported.
    let cfg = load_from(&cfg_path).expect("load_from absent should succeed");

    // Version must be current.
    assert_eq!(cfg.version, arctis_config::CURRENT_VERSION);

    // Active profile must have 2 imported routes.
    let profile = cfg.active().expect("active profile");
    assert_eq!(profile.routes.len(), 2, "should have 2 imported routes");

    let app_binaries: Vec<&str> = profile
        .routes
        .iter()
        .map(|r| r.app_binary.as_str())
        .collect();
    assert!(app_binaries.contains(&"firefox"), "firefox route expected");
    assert!(app_binaries.contains(&"discord"), "discord route expected");

    let firefox_route = profile
        .routes
        .iter()
        .find(|r| r.app_binary == "firefox")
        .unwrap();
    assert_eq!(firefox_route.target_sink, "Arctis_Media");

    let discord_route = profile
        .routes
        .iter()
        .find(|r| r.app_binary == "discord")
        .unwrap();
    assert_eq!(discord_route.target_sink, "Arctis_Chat");
}

#[test]
fn migrate_v0_to_v1() {
    let cfg = migrate_str(v0_fixture()).expect("migrate_str should succeed");
    assert_eq!(
        cfg.version,
        arctis_config::CURRENT_VERSION,
        "version must be 1 after migration"
    );
    // Should have a default profile
    assert_eq!(cfg.active_profile, "default");
    let profile = cfg
        .active()
        .expect("active profile should exist after migration");
    // The v0 fixture has 2 channels (game + chat)
    assert_eq!(profile.channels.len(), 2, "should have migrated 2 channels");
    let ids: Vec<&str> = profile.channels.iter().map(|c| c.id.as_str()).collect();
    assert!(ids.contains(&"game"), "should have 'game' channel");
    assert!(ids.contains(&"chat"), "should have 'chat' channel");
}

#[test]
fn atomic_write_no_partial() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("config.toml");
    let tmp_sibling = std::path::PathBuf::from(format!("{}.tmp", target.display()));

    let cfg = Config::default_config();
    save_to(&target, &cfg).expect("save_to should succeed");

    // Temp file must be gone.
    assert!(
        !tmp_sibling.exists(),
        "temp file {tmp_sibling:?} must not remain"
    );

    // Target must exist and parse correctly.
    assert!(target.exists(), "target file must exist");
    let loaded = load_from(&target).expect("should load saved file");
    assert_eq!(cfg, loaded, "saved file must round-trip");
}
