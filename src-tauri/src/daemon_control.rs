//! GUI-side daemon lifecycle control (start/stop/restart/autostart). Acts directly
//! in the Tauri process — NOT via the daemon IPC (which is down when starting).
//! All orchestration goes through the `DaemonEnv` seam so it is unit-testable.
use std::path::{Path, PathBuf};

pub const UNIT_NAME: &str = "arctis-sound-manager.service";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedBy { Systemd, Manual, Stopped }

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DaemonStatus {
    pub running: bool,
    pub managed_by: ManagedBy,
    pub autostart_enabled: bool,
    pub systemd_available: bool,
    pub binary_path: Option<String>,
    pub unit_installed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOut { pub status: i32, pub stdout: String, pub stderr: String }

/// Seam for every side effect. Real impl uses std::process / arctis_client / fs;
/// tests use MockEnv.
pub trait DaemonEnv {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CmdOut>;
    fn spawn_detached(&self, program: &str, args: &[&str]) -> std::io::Result<()>;
    fn socket_live(&self, socket: &Path) -> bool;
    fn shutdown_ipc(&self, socket: &Path) -> bool;
    fn path_exists(&self, p: &Path) -> bool;
    fn read_file(&self, p: &Path) -> std::io::Result<String>;
    fn write_file_atomic(&self, p: &Path, contents: &str) -> std::io::Result<()>;
}

/// First existing candidate (caller builds the ordered list; `exists` is injected).
pub fn resolve_binary(candidates: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    candidates.iter().find(|p| exists(p)).cloned()
}

pub fn unit_path(home: &Path) -> PathBuf {
    home.join(".config/systemd/user").join(UNIT_NAME)
}

pub fn query_status(env: &impl DaemonEnv, socket: &Path, binary: Option<PathBuf>, home: &Path) -> DaemonStatus {
    let systemd_available = env.run("systemctl", &["--user", "--version"]).map(|o| o.status == 0).unwrap_or(false);
    let unit_installed = env.path_exists(&unit_path(home));
    let is_active = systemd_available
        && env.run("systemctl", &["--user", "is-active", UNIT_NAME]).map(|o| o.status == 0).unwrap_or(false);
    let autostart_enabled = systemd_available
        && env.run("systemctl", &["--user", "is-enabled", UNIT_NAME]).map(|o| o.status == 0).unwrap_or(false);
    let socket_live = env.socket_live(socket);
    let running = socket_live || is_active;
    let managed_by = if is_active { ManagedBy::Systemd }
        else if socket_live { ManagedBy::Manual }
        else { ManagedBy::Stopped };
    DaemonStatus {
        running, managed_by, autostart_enabled, systemd_available,
        binary_path: binary.map(|p| p.to_string_lossy().into_owned()),
        unit_installed,
    }
}

fn use_systemd(env: &impl DaemonEnv, home: &Path) -> bool {
    let available = env.run("systemctl", &["--user", "--version"]).map(|o| o.status == 0).unwrap_or(false);
    available && env.path_exists(&unit_path(home))
}

fn systemctl(env: &impl DaemonEnv, verb: &str) -> Result<(), String> {
    match env.run("systemctl", &["--user", verb, UNIT_NAME]) {
        Ok(o) if o.status == 0 => Ok(()),
        Ok(o) => Err(format!("systemctl --user {verb} failed (exit {}): {}", o.status, o.stderr.trim())),
        Err(e) => Err(format!("systemctl not runnable: {e}")),
    }
}

pub fn start(env: &impl DaemonEnv, socket: &Path, binary: &Path, home: &Path) -> Result<(), String> {
    if use_systemd(env, home) { return systemctl(env, "start"); }
    if env.socket_live(socket) { return Ok(()); }
    env.spawn_detached(&binary.to_string_lossy(), &["daemon"])
        .map_err(|e| format!("failed to spawn daemon: {e}"))
}

pub fn stop(env: &impl DaemonEnv, socket: &Path, home: &Path) -> Result<(), String> {
    if use_systemd(env, home) { return systemctl(env, "stop"); }
    if env.socket_live(socket) {
        return if env.shutdown_ipc(socket) { Ok(()) } else { Err("daemon did not acknowledge shutdown".into()) };
    }
    Ok(())
}

pub fn restart(env: &impl DaemonEnv, socket: &Path, binary: &Path, home: &Path) -> Result<(), String> {
    if use_systemd(env, home) { return systemctl(env, "restart"); }
    stop(env, socket, home)?;
    start(env, socket, binary, home)
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct MockEnv {
    pub runs: std::cell::RefCell<Vec<(String, Vec<String>)>>,
    pub spawns: std::cell::RefCell<Vec<(String, Vec<String>)>>,
    pub run_results: std::cell::RefCell<std::collections::HashMap<String, CmdOut>>, // key = joined argv
    pub socket_live: std::cell::Cell<bool>,
    pub shutdown_ok: std::cell::Cell<bool>,
    pub existing: std::cell::RefCell<std::collections::HashSet<PathBuf>>,
    pub files: std::cell::RefCell<std::collections::HashMap<PathBuf, String>>,
}

#[cfg(test)]
impl DaemonEnv for MockEnv {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CmdOut> {
        let key = std::iter::once(program.to_string()).chain(args.iter().map(|s| s.to_string()))
            .collect::<Vec<_>>().join(" ");
        self.runs.borrow_mut().push((program.into(), args.iter().map(|s| s.to_string()).collect()));
        Ok(self.run_results.borrow().get(&key).cloned()
            .unwrap_or(CmdOut { status: 0, stdout: String::new(), stderr: String::new() }))
    }
    fn spawn_detached(&self, program: &str, args: &[&str]) -> std::io::Result<()> {
        self.spawns.borrow_mut().push((program.into(), args.iter().map(|s| s.to_string()).collect()));
        Ok(())
    }
    fn socket_live(&self, _socket: &Path) -> bool { self.socket_live.get() }
    fn shutdown_ipc(&self, _socket: &Path) -> bool { self.shutdown_ok.get() }
    fn path_exists(&self, p: &Path) -> bool { self.existing.borrow().contains(p) }
    fn read_file(&self, p: &Path) -> std::io::Result<String> {
        self.files.borrow().get(p).cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no file"))
    }
    fn write_file_atomic(&self, p: &Path, contents: &str) -> std::io::Result<()> {
        self.files.borrow_mut().insert(p.to_path_buf(), contents.to_string());
        self.existing.borrow_mut().insert(p.to_path_buf());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_binary_returns_first_existing_in_order() {
        let present: std::collections::HashSet<PathBuf> =
            [PathBuf::from("/usr/bin/asm-cli")].into_iter().collect();
        let exists = |p: &Path| present.contains(p);
        let candidates = vec![
            PathBuf::from("/override/asm-cli"),
            PathBuf::from("/usr/bin/asm-cli"),
            PathBuf::from("/home/x/.local/bin/asm-cli"),
        ];
        assert_eq!(resolve_binary(&candidates, &exists), Some(PathBuf::from("/usr/bin/asm-cli")));
    }

    #[test]
    fn resolve_binary_none_when_no_candidate_exists() {
        let exists = |_: &Path| false;
        assert_eq!(resolve_binary(&[PathBuf::from("/x/asm-cli")], &exists), None);
    }

    #[test]
    fn status_manual_when_socket_live_but_no_systemd_unit() {
        let env = MockEnv::default();
        env.socket_live.set(true);
        // systemctl --version ok (available), but is-active/is-enabled fail (1)
        env.run_results.borrow_mut().insert("systemctl --user is-active arctis-sound-manager.service".into(),
            CmdOut { status: 3, stdout: "inactive".into(), stderr: String::new() });
        env.run_results.borrow_mut().insert("systemctl --user is-enabled arctis-sound-manager.service".into(),
            CmdOut { status: 1, stdout: "disabled".into(), stderr: String::new() });
        let s = query_status(&env, Path::new("/run/x.sock"), None, Path::new("/home/x"));
        assert!(s.running);
        assert_eq!(s.managed_by, ManagedBy::Manual);
        assert!(!s.autostart_enabled);
    }

    #[test]
    fn status_systemd_when_is_active_zero() {
        let env = MockEnv::default();
        env.run_results.borrow_mut().insert("systemctl --user is-active arctis-sound-manager.service".into(),
            CmdOut { status: 0, stdout: "active".into(), stderr: String::new() });
        env.run_results.borrow_mut().insert("systemctl --user is-enabled arctis-sound-manager.service".into(),
            CmdOut { status: 0, stdout: "enabled".into(), stderr: String::new() });
        env.existing.borrow_mut().insert(unit_path(Path::new("/home/x")));
        let s = query_status(&env, Path::new("/run/x.sock"), None, Path::new("/home/x"));
        assert_eq!(s.managed_by, ManagedBy::Systemd);
        assert!(s.running && s.autostart_enabled && s.unit_installed);
    }

    #[test]
    fn status_stopped_when_nothing_live() {
        let env = MockEnv::default();
        env.run_results.borrow_mut().insert("systemctl --user is-active arctis-sound-manager.service".into(),
            CmdOut { status: 3, stdout: String::new(), stderr: String::new() });
        let s = query_status(&env, Path::new("/run/x.sock"), None, Path::new("/home/x"));
        assert_eq!(s.managed_by, ManagedBy::Stopped);
        assert!(!s.running);
    }

    #[test]
    fn start_spawns_when_no_systemd_and_socket_dead() {
        let env = MockEnv::default();
        env.run_results.borrow_mut().insert("systemctl --user --version".into(),
            CmdOut { status: 127, stdout: String::new(), stderr: "not found".into() });
        env.socket_live.set(false);
        start(&env, Path::new("/run/x.sock"), Path::new("/usr/bin/asm-cli"), Path::new("/home/x")).unwrap();
        let spawns = env.spawns.borrow();
        assert_eq!(spawns.len(), 1);
        assert_eq!(spawns[0].0, "/usr/bin/asm-cli");
        assert_eq!(spawns[0].1, vec!["daemon".to_string()]);
    }

    #[test]
    fn stop_sends_shutdown_ipc_when_manual() {
        let env = MockEnv::default();
        env.run_results.borrow_mut().insert("systemctl --user --version".into(),
            CmdOut { status: 127, stdout: String::new(), stderr: String::new() });
        env.socket_live.set(true);
        env.shutdown_ok.set(true);
        stop(&env, Path::new("/run/x.sock"), Path::new("/home/x")).unwrap();
        // No systemctl stop attempted (no unit); shutdown_ipc was used → success.
    }

    #[test]
    fn start_uses_systemctl_when_unit_installed_and_available() {
        let env = MockEnv::default();
        env.existing.borrow_mut().insert(unit_path(Path::new("/home/x")));
        // systemctl --version default status 0 (available)
        start(&env, Path::new("/run/x.sock"), Path::new("/usr/bin/asm-cli"), Path::new("/home/x")).unwrap();
        let runs = env.runs.borrow();
        assert!(runs.iter().any(|(p, a)| p == "systemctl" && a == &vec!["--user","start","arctis-sound-manager.service"]
            .iter().map(|s| s.to_string()).collect::<Vec<_>>()));
        assert!(env.spawns.borrow().is_empty());
    }
}
