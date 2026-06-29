# GUI Daemon Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Start/stop/restart the `asm-cli` daemon and toggle autostart from the GUI, via a hybrid systemd-or-spawn model with an idempotent autostart install.

**Architecture:** New `src-tauri/src/daemon_control.rs` owns all logic behind ONE injectable `DaemonEnv` seam (real impl over `std::process`/`arctis_client`; mock for tests). Pure helpers (path resolution, status parsing, argv/unit-file rendering) are split from side-effecting orchestration. Tauri commands wrap it and return a fresh `DaemonStatus`; a thin Svelte "Daemon" section on the Device page drives them with pure logic in `daemonControl.ts`.

**Tech Stack:** Rust (`src-tauri`, Tauri v2), `arctis_client` (socket + `Request::Shutdown`), Svelte 5 runes + bits-ui, `systemctl --user`.

## Global Constraints

- No device (HID) writes; device-write allowlist untouched (G2). This feature manages a process/service + writes ONE systemd unit under `~/.config/systemd/user/`.
- Daemon lifecycle changes are ALWAYS explicit/button-initiated (user consent); never automatic.
- `tauri` code only in `src-tauri`; do NOT modify engine/cli/config/audio crates (reuse `arctis_client::{socket_path, send_request_to, Request::Shutdown}`).
- Typed errors via `CommandError` (variants `DaemonUnavailable(String)`, `Daemon(String)`); NO `unwrap`/`expect`/`panic` on runtime paths (G7).
- GUI logic in pure testable helpers; `.svelte` is a thin view; NO jsdom/testing-library (logic → `.ts` + vitest).
- Autostart install is idempotent: exactly one unit file (atomic compare-then-overwrite, skip if identical) and one `enable --now`; never duplicates.
- Unit name = `arctis-sound-manager.service`; user unit dir = `~/.config/systemd/user/`.
- Design system: `--ss` tokens, bits-ui `Switch` + existing button styles.
- Build/test: `cargo test -p arctis-sound-manager-ui` (the src-tauri crate), `cd frontend && npm test && npm run check`. Commit after each task.

---

## File Structure

**New:**
- `src-tauri/src/daemon_control.rs` — seam trait, real+mock impls, pure helpers, orchestration. [Tasks 1–4]
- `frontend/src/lib/daemonControl.ts` — pure UI helpers. [Task 6]
- `frontend/src/lib/daemonControl.test.ts` — vitest. [Task 6]
- `frontend/src/lib/components/DaemonSection.svelte` — thin view. [Task 7]

**Modified:**
- `src-tauri/src/lib.rs` — `mod daemon_control;` + register 5 commands in `generate_handler!`. [Tasks 1, 5]
- `src-tauri/src/commands.rs` — 5 `#[tauri::command]` wrappers. [Task 5]
- `frontend/src/lib/ipc.ts` — `DaemonStatus` type + 5 wrappers. [Task 5]
- The Device page component (confirm exact file in Task 7) — embed `<DaemonSection/>`. [Task 7]

---

## Task 1: daemon_control scaffold — types, `DaemonEnv` seam, binary path resolution

**Files:**
- Create: `src-tauri/src/daemon_control.rs`
- Modify: `src-tauri/src/lib.rs` (add `mod daemon_control;` near the other `mod` lines)
- Test: inline `#[cfg(test)]` in `daemon_control.rs`

**Interfaces:**
- Produces: `ManagedBy { Systemd, Manual, Stopped }` (serde snake_case); `DaemonStatus { running, managed_by, autostart_enabled, systemd_available, binary_path: Option<String>, unit_installed }` (Serialize); `CmdOut { status: i32, stdout: String, stderr: String }`; `trait DaemonEnv` (see below); `MockEnv` (test); `pub fn resolve_binary(candidates: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf>`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-sound-manager-ui resolve_binary`
Expected: FAIL (module/fn not defined).

- [ ] **Step 3: Write minimal implementation**

```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p arctis-sound-manager-ui resolve_binary`
Expected: PASS (2 tests). Then `cargo build -p arctis-sound-manager-ui`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/daemon_control.rs src-tauri/src/lib.rs
git commit -m "feat(daemon): daemon_control scaffold — types, DaemonEnv seam, path resolution"
```

---

## Task 2: status detection

**Files:**
- Modify: `src-tauri/src/daemon_control.rs`
- Test: inline

**Interfaces:**
- Consumes: `DaemonEnv`, `DaemonStatus`, `UNIT_NAME` [T1].
- Produces: `pub fn unit_path(home: &Path) -> PathBuf` (`<home>/.config/systemd/user/arctis-sound-manager.service`); `pub fn query_status(env: &impl DaemonEnv, socket: &Path, binary: Option<PathBuf>, home: &Path) -> DaemonStatus`.

Semantics: `systemd_available` = `run("systemctl", ["--user","--version"]).status == 0`. `unit_installed` = `path_exists(unit_path)`. `is_active` = `run("systemctl",["--user","is-active",UNIT_NAME]).status == 0`. `autostart_enabled` = `run("systemctl",["--user","is-enabled",UNIT_NAME]).status == 0`. `running` = `socket_live(socket) || is_active`. `managed_by` = if `is_active` → Systemd; else if `socket_live` → Manual; else Stopped.

- [ ] **Step 1: Write the failing test**

```rust
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
```

- [ ] **Step 2: Run → FAIL** (`cargo test -p arctis-sound-manager-ui status_`).

- [ ] **Step 3: Implement**

```rust
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
```

- [ ] **Step 4: Run → PASS** (3 tests). `cargo build -p arctis-sound-manager-ui`.

- [ ] **Step 5: Commit** — `git commit -am "feat(daemon): status detection (systemd/manual/stopped)"`

---

## Task 3: start / stop / restart orchestration

**Files:**
- Modify: `src-tauri/src/daemon_control.rs`
- Test: inline

**Interfaces:**
- Consumes: `DaemonEnv`, `DaemonStatus`, `ManagedBy`, `UNIT_NAME` [T1/T2].
- Produces: `pub fn start(env, socket, binary: &Path, home) -> Result<(), String>`, `pub fn stop(env, socket, home) -> Result<(), String>`, `pub fn restart(env, socket, binary: &Path, home) -> Result<(), String>`. Each decides systemd vs manual from current status.

Semantics — let `systemd = systemd_available && unit_installed`:
- `start`: systemd → `run("systemctl",["--user","start",UNIT_NAME])` (err if status≠0); else if `socket_live` → Ok (already running); else `spawn_detached(binary, &["daemon"])`.
- `stop`: systemd → `run("systemctl",["--user","stop",UNIT_NAME])`; else if `socket_live` → `shutdown_ipc(socket)` (err if false); else Ok (already stopped).
- `restart`: systemd → `run("systemctl",["--user","restart",UNIT_NAME])`; else `stop` then `start` (the caller's command re-queries status between; here just call stop(...)? then start(...)).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn start_spawns_when_no_systemd_and_socket_dead() {
    let env = MockEnv::default(); // systemctl --version → default status 0? force unavailable:
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
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** (use `query_status` to decide; map non-zero/err to `Err(String)`):

```rust
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
```

- [ ] **Step 4: Run → PASS** (3 tests). Build.

- [ ] **Step 5: Commit** — `git commit -am "feat(daemon): start/stop/restart (systemd or spawn/shutdown-ipc)"`

---

## Task 4: idempotent autostart install + unit rendering

**Files:**
- Modify: `src-tauri/src/daemon_control.rs`
- Test: inline

**Interfaces:**
- Consumes: `DaemonEnv`, `unit_path`, `UNIT_NAME` [T1/T2].
- Produces: `pub fn render_unit(binary_path: &Path) -> String`; `pub fn install_autostart(env, binary: &Path, home) -> Result<(), String>`; `pub fn disable_autostart(env, home) -> Result<(), String>`.

`render_unit` produces a systemd user unit with `ExecStart=<binary> daemon`, `After/Wants=pipewire.service wireplumber.service`, `Restart=on-failure`, `RestartSec=3s`, `Environment=XDG_RUNTIME_DIR=%t`, `RuntimeDirectory=arctis-sound-manager`, `WantedBy=default.target`. (Mirror `packaging/systemd/arctis-sound-manager.service`, but with ExecStart set to the resolved path.)

`install_autostart` (idempotent): render → if `read_file(unit_path) == rendered` skip write; else `write_file_atomic`; if written, `run("systemctl",["--user","daemon-reload"])`; then `run("systemctl",["--user","enable","--now",UNIT_NAME])` (map non-zero → Err). `disable_autostart`: `run("systemctl",["--user","disable","--now",UNIT_NAME])` (leave file in place).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn render_unit_substitutes_execstart() {
    let u = render_unit(Path::new("/usr/bin/asm-cli"));
    assert!(u.contains("ExecStart=/usr/bin/asm-cli daemon"));
    assert!(u.contains("WantedBy=default.target"));
}
#[test]
fn install_writes_then_reloads_then_enables_when_absent() {
    let env = MockEnv::default();
    install_autostart(&env, Path::new("/usr/bin/asm-cli"), Path::new("/home/x")).unwrap();
    // unit file written
    assert!(env.files.borrow().contains_key(&unit_path(Path::new("/home/x"))));
    let runs = env.runs.borrow();
    assert!(runs.iter().any(|(p,a)| p=="systemctl" && a.contains(&"daemon-reload".to_string())));
    assert!(runs.iter().any(|(p,a)| p=="systemctl" && a.contains(&"enable".to_string()) && a.contains(&"--now".to_string())));
}
#[test]
fn install_is_idempotent_skips_write_when_identical() {
    let env = MockEnv::default();
    let p = unit_path(Path::new("/home/x"));
    let existing = render_unit(Path::new("/usr/bin/asm-cli"));
    env.files.borrow_mut().insert(p.clone(), existing);
    env.existing.borrow_mut().insert(p.clone());
    // clear runs after seeding
    install_autostart(&env, Path::new("/usr/bin/asm-cli"), Path::new("/home/x")).unwrap();
    let runs = env.runs.borrow();
    // identical content → NO daemon-reload (no write), but enable still runs (idempotent)
    assert!(!runs.iter().any(|(p,a)| p=="systemctl" && a.contains(&"daemon-reload".to_string())));
    assert!(runs.iter().any(|(p,a)| p=="systemctl" && a.contains(&"enable".to_string())));
}
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement**

```rust
pub fn render_unit(binary_path: &Path) -> String {
    format!(
"[Unit]
Description=Arctis Sound Manager daemon
After=pipewire.service wireplumber.service
Wants=pipewire.service wireplumber.service

[Service]
Type=simple
ExecStart={bin} daemon
Restart=on-failure
RestartSec=3s
Environment=XDG_RUNTIME_DIR=%t
RuntimeDirectory=arctis-sound-manager
RuntimeDirectoryMode=0700

[Install]
WantedBy=default.target
",
        bin = binary_path.display()
    )
}
pub fn install_autostart(env: &impl DaemonEnv, binary: &Path, home: &Path) -> Result<(), String> {
    let path = unit_path(home);
    let rendered = render_unit(binary);
    let identical = env.read_file(&path).map(|c| c == rendered).unwrap_or(false);
    if !identical {
        env.write_file_atomic(&path, &rendered).map_err(|e| format!("write unit failed: {e}"))?;
        match env.run("systemctl", &["--user", "daemon-reload"]) {
            Ok(o) if o.status == 0 => {}
            Ok(o) => return Err(format!("daemon-reload failed (exit {}): {}", o.status, o.stderr.trim())),
            Err(e) => return Err(format!("daemon-reload not runnable: {e}")),
        }
    }
    match env.run("systemctl", &["--user", "enable", "--now", UNIT_NAME]) {
        Ok(o) if o.status == 0 => Ok(()),
        Ok(o) => Err(format!("enable failed (exit {}): {}", o.status, o.stderr.trim())),
        Err(e) => Err(format!("enable not runnable: {e}")),
    }
}
pub fn disable_autostart(env: &impl DaemonEnv, _home: &Path) -> Result<(), String> {
    match env.run("systemctl", &["--user", "disable", "--now", UNIT_NAME]) {
        Ok(o) if o.status == 0 => Ok(()),
        Ok(o) => Err(format!("disable failed (exit {}): {}", o.status, o.stderr.trim())),
        Err(e) => Err(format!("disable not runnable: {e}")),
    }
}
```

- [ ] **Step 4: Run → PASS** (3 tests). Build.

- [ ] **Step 5: Commit** — `git commit -am "feat(daemon): idempotent autostart install + unit rendering"`

---

## Task 5: real `DaemonEnv` impl + Tauri commands + ipc

**Files:**
- Modify: `src-tauri/src/daemon_control.rs` (add `RealEnv` + candidate builder)
- Modify: `src-tauri/src/commands.rs` (5 commands)
- Modify: `src-tauri/src/lib.rs` (register in `generate_handler!`)
- Modify: `frontend/src/lib/ipc.ts` (types + wrappers)
- Test: inline `RealEnv` candidate-list test (pure part)

**Interfaces:**
- Consumes: all of T1–T4. `arctis_client::{socket_path, send_request_to, Request::Shutdown}`. `CommandError`.
- Produces: `pub struct RealEnv;` impl `DaemonEnv` (run = `std::process::Command::output`; spawn_detached = `Command.…pre_exec(setsid).spawn()`; socket_live = `UnixStream::connect(socket).is_ok()`; shutdown_ipc = `send_request_to(socket, &Request::Shutdown).map(|r| r.ok).unwrap_or(false)`; path_exists = `Path::exists`; read_file = `fs::read_to_string`; write_file_atomic = temp+rename in the same dir). `pub fn candidate_binaries() -> Vec<PathBuf>` (the §7 ordered list from `std::env::var("ASM_CLI_BIN")`, `current_exe`, `$HOME/.local/bin`, `/usr/bin`, `target/{release,debug}`). `pub fn home_dir() -> PathBuf`. Commands: `daemon_status/daemon_start/daemon_stop/daemon_restart/daemon_set_autostart(enabled: bool)` each `-> Result<DaemonStatus, CommandError>`.

- [ ] **Step 1: Write the failing test** (pure candidate-ordering — RealEnv I/O is owner-verified)

```rust
#[test]
fn candidate_binaries_starts_with_env_override_when_set() {
    // SAFETY: test-only env mutation
    std::env::set_var("ASM_CLI_BIN", "/custom/asm-cli");
    let c = candidate_binaries();
    std::env::remove_var("ASM_CLI_BIN");
    assert_eq!(c.first(), Some(&PathBuf::from("/custom/asm-cli")));
}
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement** `RealEnv`, `candidate_binaries`, `home_dir`, and the commands. Each command builds `RealEnv`, resolves `socket = arctis_client::socket_path()`, `home = home_dir()`, `binary = resolve_binary(&candidate_binaries(), &|p| p.exists())`, performs the action (mapping `Err(String)` → `CommandError::Daemon`), then returns `query_status(&env, &socket, binary, &home)`. `daemon_start`/`restart` require a resolved binary in the spawn fallback → if `binary` is None AND not systemd, return `CommandError::Daemon("asm-cli binary not found; set $ASM_CLI_BIN")`. Example command:

```rust
// src-tauri/src/commands.rs
use crate::daemon_control::{self as dc, DaemonStatus};
#[tauri::command]
pub async fn daemon_status() -> Result<DaemonStatus, CommandError> {
    tauri::async_runtime::spawn_blocking(|| {
        let env = dc::RealEnv;
        let socket = arctis_client::socket_path();
        let home = dc::home_dir();
        let binary = dc::resolve_binary(&dc::candidate_binaries(), &|p| p.exists());
        Ok(dc::query_status(&env, &socket, binary, &home))
    }).await.map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))?
}
#[tauri::command]
pub async fn daemon_start() -> Result<DaemonStatus, CommandError> {
    tauri::async_runtime::spawn_blocking(|| {
        let env = dc::RealEnv;
        let socket = arctis_client::socket_path();
        let home = dc::home_dir();
        let binary = dc::resolve_binary(&dc::candidate_binaries(), &|p| p.exists());
        if !dc::use_systemd_pub(&env, &home) {
            let b = binary.clone().ok_or_else(|| CommandError::Daemon("asm-cli binary not found; set $ASM_CLI_BIN".into()))?;
            dc::start(&env, &socket, &b, &home).map_err(CommandError::Daemon)?;
        } else {
            dc::start(&env, &socket, std::path::Path::new("/unused"), &home).map_err(CommandError::Daemon)?;
        }
        Ok(dc::query_status(&env, &socket, binary, &home))
    }).await.map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))?
}
// daemon_stop / daemon_restart / daemon_set_autostart(enabled) follow the same shape.
```
(Expose `use_systemd` as `pub fn use_systemd_pub` or make `use_systemd` `pub` for the command's binary-None guard.) Register all five in `src-tauri/src/lib.rs generate_handler![ … ]`. Add to `ipc.ts`:
```typescript
export type ManagedBy = "systemd" | "manual" | "stopped";
export interface DaemonStatus {
  running: boolean; managed_by: ManagedBy; autostart_enabled: boolean;
  systemd_available: boolean; binary_path: string | null; unit_installed: boolean;
}
export const daemonStatus = (): Promise<DaemonStatus> => invoke<DaemonStatus>("daemon_status");
export const daemonStart = (): Promise<DaemonStatus> => invoke<DaemonStatus>("daemon_start");
export const daemonStop = (): Promise<DaemonStatus> => invoke<DaemonStatus>("daemon_stop");
export const daemonRestart = (): Promise<DaemonStatus> => invoke<DaemonStatus>("daemon_restart");
export const daemonSetAutostart = (enabled: boolean): Promise<DaemonStatus> =>
  invoke<DaemonStatus>("daemon_set_autostart", { enabled });
```

- [ ] **Step 4: Run** `cargo test -p arctis-sound-manager-ui` (all green), `cargo build -p arctis-sound-manager-ui`, `cd frontend && npm run check`.

- [ ] **Step 5: Commit** — `git commit -am "feat(daemon): RealEnv + Tauri daemon_* commands + ipc wrappers"`

---

## Task 6: frontend pure helpers `daemonControl.ts`

**Files:**
- Create: `frontend/src/lib/daemonControl.ts`
- Create: `frontend/src/lib/daemonControl.test.ts`

**Interfaces:**
- Consumes: `DaemonStatus`/`ManagedBy` from `./ipc.js` [T5].
- Produces: `statusLabel(s) -> string` ("Running (systemd)" / "Running (manual)" / "Stopped"); `dotKind(s) -> "ok" | "off"`; `canStart(s)/canStop(s)/canRestart(s) -> boolean`; `autostartDisabledReason(s) -> string | null` (null = enabled control; string when `!s.systemd_available`).

- [ ] **Step 1: Write the failing test**

```ts
import { describe, it, expect } from "vitest";
import { statusLabel, dotKind, canStart, canStop, autostartDisabledReason } from "./daemonControl";
import type { DaemonStatus } from "./ipc";
const base: DaemonStatus = { running:false, managed_by:"stopped", autostart_enabled:false, systemd_available:true, binary_path:"/usr/bin/asm-cli", unit_installed:false };
describe("daemonControl", () => {
  it("labels", () => {
    expect(statusLabel({...base, running:true, managed_by:"systemd"})).toBe("Running (systemd)");
    expect(statusLabel({...base, running:true, managed_by:"manual"})).toBe("Running (manual)");
    expect(statusLabel(base)).toBe("Stopped");
  });
  it("button enablement", () => {
    expect(canStart(base)).toBe(true);
    expect(canStop(base)).toBe(false);
    expect(canStop({...base, running:true})).toBe(true);
    expect(canStart({...base, running:true})).toBe(false);
  });
  it("autostart disabled without systemd", () => {
    expect(autostartDisabledReason(base)).toBeNull();
    expect(autostartDisabledReason({...base, systemd_available:false})).toMatch(/systemd/i);
  });
});
```

- [ ] **Step 2: Run → FAIL** (`cd frontend && npm test -- daemonControl`).

- [ ] **Step 3: Implement**

```ts
import type { DaemonStatus } from "./ipc.js";
export function statusLabel(s: DaemonStatus): string {
  if (!s.running) return "Stopped";
  return s.managed_by === "systemd" ? "Running (systemd)" : "Running (manual)";
}
export function dotKind(s: DaemonStatus): "ok" | "off" { return s.running ? "ok" : "off"; }
export function canStart(s: DaemonStatus): boolean { return !s.running; }
export function canStop(s: DaemonStatus): boolean { return s.running; }
export function canRestart(s: DaemonStatus): boolean { return s.running; }
export function autostartDisabledReason(s: DaemonStatus): string | null {
  return s.systemd_available ? null : "Autostart needs systemd (user manager) — not available here.";
}
```

- [ ] **Step 4: Run → PASS.** `cd frontend && npm run check`.

- [ ] **Step 5: Commit** — `git commit -am "feat(daemon): pure daemonControl UI helpers + tests"`

---

## Task 7: Daemon section UI + wire into Device page

**Files:**
- Create: `frontend/src/lib/components/DaemonSection.svelte`
- Modify: the Device page component (find it: `grep -ril "device" frontend/src/lib/components` / inspect the nav/router in `App.svelte`/`AppShell.svelte`; embed `<DaemonSection/>`)
- Test: none (thin view; logic covered in T6) — owner-manual-verify

**Interfaces:**
- Consumes: `daemonStatus/Start/Stop/Restart/SetAutostart` [T5], the T6 helpers, bits-ui `Switch` (`frontend/src/lib/ui/Switch.svelte`).

- [ ] **Step 1: Build the component** — load `daemonStatus()` on mount into `let status = $state<DaemonStatus|null>(null)`; render: a status dot (color from `dotKind`, using `--ss` success/danger tokens) + `statusLabel(status)`, a muted sub-line `status.binary_path ?? "asm-cli not found"`; three buttons (existing `.ss-btn` style) Start/Stop/Restart with `disabled={!canStart(status)}` etc. and a shared `busy` flag; a bits-ui `Switch` bound to `status.autostart_enabled` → `daemonSetAutostart(v)`, disabled with a title from `autostartDisabledReason(status)`. Every action does `busy=true; try { status = await daemonX(); } catch(e){ msg = String(e); } finally { busy=false; }`. Inline `msg` feedback area. Section heading "Daemon", consistent with other Device-page sections.

- [ ] **Step 2: Embed** `<DaemonSection/>` in the Device page (after confirming the file). 

- [ ] **Step 3: Verify** `cd frontend && npm run check` (0 errors) and `npm test` (T6 green). Build the app if quick.

- [ ] **Step 4: Commit** — `git commit -am "feat(daemon): Daemon control section on the Device page"`

- [ ] **Step 5 (owner-manual-verify, document in commit/PR):** Start/Stop/Restart drive the real daemon; autostart toggle → `systemctl --user is-enabled arctis-sound-manager.service` shows enabled; clicking install twice produces no duplicate unit (one file, one symlink).

---

## Self-Review Notes (author)

- **Spec coverage:** §3 module/seam → T1; §4 data model + commands → T1/T5; §5 ops → T3; §6 idempotent install → T4; §7 path resolution → T1/T5; §8 UI → T6/T7; §10 testing → per-task tests. All covered.
- **Type consistency:** `DaemonStatus`/`ManagedBy` identical across T1 (Rust) and T5 (ipc.ts); `query_status`/`start`/`stop`/`restart`/`install_autostart`/`disable_autostart`/`render_unit`/`resolve_binary`/`unit_path`/`candidate_binaries` signatures stable across tasks.
- **Idempotency (D3):** T4 proves install-when-identical skips the write + reload; one unit path, one enable.
- **No device writes (G2):** nothing in any task touches the device crate/allowlist.
- **No-jsdom:** T6 logic in `.ts` + vitest; T7 `.svelte` thin, owner-verified.
- **Open item:** exact Device-page file + the `pre_exec(setsid)` detached-spawn confirmed during T5/T7 implementation against the real tree.
