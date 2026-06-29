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
}
