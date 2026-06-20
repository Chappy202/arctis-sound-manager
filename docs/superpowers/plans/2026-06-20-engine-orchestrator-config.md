# Engine + Orchestrator + Unified Config — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to execute this plan task-by-task. Each task is an isolated TDD unit (write failing test → run & confirm failure → implement → run & confirm pass → commit). OWNER-RUN tasks are manual hardware validation, not auto-executed — stop and hand them to the human owner; do not fabricate their output.

---

## Goal

Make the proven-but-disconnected crates (`domain`, `device`, `audio`, `cli`) into a cohesive
application by adding the two keystone crates the architecture has been building toward:

1. **`arctis-config`** — the single source of truth (ARCHITECTURE G4): ONE schema-versioned,
   atomically-written file holding the channel set, per-channel EQ, the routing rules (absorbing
   Plan 4's scattered `routes.json`), and named **profiles** + the active profile name.
2. **`arctis-engine`** — the UI-agnostic orchestrator that *composes* `device` + `audio` + `config`
   to reconcile the live PipeWire graph to the active profile (re-apply-on-startup, idempotent),
   **owns the spawned `pipewire -c` child processes with deterministic teardown** (G10), and exposes
   an async, UI-agnostic API + event stream.
3. **A resident daemon** (`asm-cli daemon` subcommand, reusing the existing `cli` crate) that loads
   config, applies the active profile on start, owns the children, stays resident, and serves a tiny
   Unix-socket control protocol.
4. **Wiring** `asm-cli` profile/apply/daemon subcommands so changes persist to the unified config and
   flow through the engine — while keeping every existing low-level subcommand working.

The engine **composes**; it never reimplements audio or routing (G1). It reuses `ChannelManager`,
`AudioBackend`, `Router`, `EqModel`, and `Registry` verbatim.

---

## Architecture

### Crate graph (dependency direction — arrows point toward dependents)

```
domain ──┬──> device ─┐
         ├──> audio ──┤
         └──> config ─┴──> engine ──> cli (asm-cli: one-shot cmds + `daemon` subcommand)
                                  └──> (future) src-tauri   [NOT in this plan]
```

`config` depends only on `domain` (+ serde/toml). `engine` depends on `domain`, `device`, `audio`,
`config`. **No crate at or below `engine` depends on `tauri` or `tokio`.**

### Runtime composition (config → engine → {cli, daemon}; engine → {audio, device})

```
                       ┌──────────────────────────── asm-cli ────────────────────────────┐
   one-shot cmds ──────┤  list / probe / sink / eq / channels / route / channel (existing)│
   (direct engine)     │  profile list|show|switch|save|new   apply   daemon (NEW)        │
                       └───────────────┬──────────────────────────────┬──────────────────┘
                                       │ direct calls                  │ run resident
                                       v                               v
                              ┌──────────────────┐            ┌─────────────────────────┐
   ~/.config/arctis-sound-    │   arctis-engine  │            │  daemon loop (UnixListener│
   manager/config.toml  <───> │  Engine<R>       │ <───events │  + line-JSON protocol)   │
   (single source of truth)   │  - reconcile()   │            │  owns Engine, stays up   │
        ^      load/save       │  - switch_profile│            └─────────────────────────┘
        │                      │  - apply_eq_band │                     ^
   arctis-config (schema,      │  - set_route     │   control reqs:     │
   versioned, migrations,      │  - state()/events│   get-state, switch-profile,
   atomic write, profiles)     │  - ChildOwner    │   set-eq-band, route, reload, shutdown
                               └───┬──────────┬───┘
                       composes    │          │   composes
                                   v          v
                         ┌──────────────┐  ┌──────────────────────────────────────────┐
                         │ arctis-audio │  │ arctis-device (READ-ONLY status, best-    │
                         │ ChannelMgr   │  │ effort; engine never blocks on device)    │
                         │ AudioBackend │  └──────────────────────────────────────────┘
                         │ Router, Eq   │
                         │ CommandRunner│ <── ChildOwner spawns `pipewire -c` in its own
                         └──────────────┘     process group; teardown kills the group.
```

### Child-ownership mechanism (the key correctness improvement)

The current `AudioBackend::create` calls `runner.spawn_detached("pipewire", ["-c", conf])` and **drops
the handle** — teardown is best-effort `pkill -f <conf>`. That leaves orphaned `pipewire -c` processes
and races the conf path.

This plan **extends the `CommandRunner` seam** with a `spawn_owned` method that returns an opaque
`ChildHandle` the engine *stores* in a `ChildOwner`. The real runner launches the child in **its own
process group** (`std::os::unix::process::CommandExt::process_group(0)`, stable since Rust 1.64 /
RFC 3228) so that on teardown the engine signals the **whole group** with
`libc::kill(-pgid, SIGTERM)` — deterministically reaping `pipewire` *and any grandchildren it spawns*.
`ChildOwner` also implements `Drop` (kill-on-drop) so a panicking/`?`-propagating engine still cleans
up. The `MockRunner` records `spawn_owned` calls and returns a synthetic handle for argv assertions.

Note: `spawn_detached` is retained on the trait (existing low-level CLI paths use it) but the engine
exclusively uses `spawn_owned`.

---

## Tech Stack

- **Rust 2021**, workspace edition (`rust-version = "1.78"` in `[workspace.package]`; `process_group`
  is stable well before that).
- **Serialization:** `serde` + `toml` (already workspace deps) for config; `serde_json` (already a
  dep) for the daemon line-protocol and reuse of the audio crate's JSON paths.
- **Errors:** `thiserror` (already a dep) — one typed error enum per new crate.
- **IPC:** `std::os::unix::net::UnixListener` + newline-delimited JSON. No new dependency.
- **Concurrency:** `std::thread` + `std::sync::mpsc`. **No tokio.** The engine's "async, UI-agnostic
  API + event stream" is realized as: synchronous request methods on `Engine` + an `mpsc::Receiver<Event>`
  the caller drains. (Justification under Research basis.)
- **Process control:** `std` + the `libc` crate (NEW, small, ubiquitous) for `kill(-pgid, SIGTERM)`.

---

## Research basis (cited; verified 2026-06)

- **(a) IPC — std `UnixListener` + line-JSON, NOT zbus/D-Bus.** zbus is runtime-agnostic and idiomatic
  on KDE, but it pulls a non-trivial dependency tree and a macro/interface surface that is overkill for
  a 6-verb private control channel (`get-state`, `switch-profile`, `set-eq-band`, `route`, `reload`,
  `shutdown`). A Unix domain socket is the commonly-used baseline IPC on Unix and needs zero new deps.
  Sources: zbus docs (https://docs.rs/zbus/latest/zbus/), Rust std `UnixListener`
  (RFC 1479, https://rust-lang.github.io/rfcs/1479-unix-socket.html). **Decision: UnixListener.** If a
  future Tauri/desktop-integration phase needs bus discovery, a zbus front-end can wrap the same
  `Engine` API without changes (the API is transport-agnostic).
- **(b) Child ownership — std `process_group(0)` + `kill(-pgid, SIGTERM)`, kill-on-drop via `Drop`.**
  `CommandExt::process_group` is stable (RFC 3228,
  https://rust-lang.github.io/rfcs/3228-process-process_group.html); PGID 0 makes the child its own
  group leader, and signalling the **negative pid** reaps the whole group including grandchildren
  (`libc::kill(-pid, libc::SIGTERM)`). This is the lightweight, dependency-minimal version of what
  crates like `command-group`/`ProcessKit` do (whole-tree kill-on-drop;
  https://github.com/ZelAnton/ProcessKit-rs). We stay on std + `libc` to avoid an async runtime
  dependency the rest of the engine doesn't need. **Decision: std process group + libc.**
- **(c) Async runtime — std threads + channels, NOT tokio.** The engine's only "async" needs are
  (1) a long-lived control-accept loop and (2) an outbound event stream. Both are cleanly served by a
  blocking accept loop on its own thread and an `mpsc` channel. All PipeWire interaction here is
  *subprocess* (`pw-cli`/`pw-dump`/`pw-metadata`/`pipewire -c`), so there is **no `!Send` pipewire-rs
  constraint** in this phase (that constraint, per ARCHITECTURE §5, only applies to the future
  in-process pipewire-rs path). Adding tokio now would be premature weight. **Decision: std threads +
  mpsc.** Revisit if/when the in-process pipewire-rs binding lands.

Source list (web-verified): ProcessKit/command-group whole-tree kill
(https://github.com/ZelAnton/ProcessKit-rs), Rust `process_group` RFC 3228, std `UnixListener`
RFC 1479, zbus docs (https://docs.rs/zbus/latest/zbus/).

---

## Non-goals (explicitly deferred — do NOT implement here)

- HID device **WRITES** (Plan 2). The engine READS device status best-effort and **never blocks** on
  the device; if no device is present, reconcile still fully succeeds.
- The **mic chain** (Plan 5), **HRIR / surround / convolver** (later phase).
- The **Tauri UI** (Plan 7) — engine must remain `tauri`-free.
- **Packaging / OTA** (Plan 8).
- Fixing **KI-1** (HW enumeration bug) — engine tolerates its absence.
- Full headset **Game/Chat dial integration** (read-only at most; not wired to volumes here).
- A separate `arctis-daemon` crate: we use an `asm-cli daemon` subcommand (simplest resident owner).

---

## Global Constraints

- Config file path: `~/.config/arctis-sound-manager/config.toml` (override via env `ASM_CONFIG_HOME`
  for tests; never touch real `~/.config` in tests).
- Config format: **TOML** (matches device descriptors + workspace `toml` dep; human-editable).
- Current config schema version: `version = 1`. Migration path: `v0 → v1` plus `routes.json` import.
- Legacy file absorbed: `~/.config/arctis-sound-manager/routes.json` (imported, then ignored).
- Daemon control socket: `$XDG_RUNTIME_DIR/arctis-sound-manager.sock` (fallback `/tmp/arctis-sound-manager-$UID.sock`).
- Daemon protocol: newline-delimited JSON; one request object per line, one response object per line.
- Sample rate: 48000 Hz (inherited from `audio` crate; never override).
- EQ bounds inherited from `audio`: gain ±12 dB, Q 0.3–10, freq 20–20000 Hz, max 10 bands.
- Atomic writes: write to `<path>.tmp` then `std::fs::rename` into place (same directory).
- Errors: typed `thiserror` enums per crate; engine error wraps `ConfigError`/`AudioError`/`TransportError`.
- No `unwrap()`/`expect()` on fallible runtime paths (tests may use them).
- Child spawn: every `pipewire -c` runs in its own process group; teardown signals `-pgid`.
- New crate names: `arctis-config` (lib `arctis_config`), `arctis-engine` (lib `arctis_engine`).
- Reconcile order (idempotent): channels up → per-channel EQ apply_all → per-channel output set →
  routing persistent+live.
- Cargo invoked as `~/.cargo/bin/cargo` (not on PATH). `.superpowers/` and `.claude/` are gitignored —
  never `git add` them.

---

# TASKS

Conventions for every task: run tests with `~/.cargo/bin/cargo test -p <crate>`; build with
`~/.cargo/bin/cargo build`. Commit message footer per repo policy. Steps are one action each:
write-failing-test → run (confirm fail) → implement → run (confirm pass) → commit.

---

## Task 1 — `arctis-config` crate scaffold + versioned schema types + workspace wiring

**Files**
- `Cargo.toml` (root — edit: add member + workspace dep)
- `crates/config/Cargo.toml` (new)
- `crates/config/src/lib.rs` (new)
- `crates/config/src/schema.rs` (new)
- `crates/config/src/error.rs` (new)

**Interfaces** (exact)

```rust
// crates/config/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error on {path}: {source_msg}")]
    Io { path: String, source_msg: String },
    #[error("failed to parse config: {0}")]
    Parse(String),
    #[error("failed to serialize config: {0}")]
    Serialize(String),
    #[error("unsupported config version {found}; max supported is {max}")]
    UnsupportedVersion { found: u32, max: u32 },
    #[error("profile not found: {0}")]
    ProfileNotFound(String),
    #[error("invalid config: {0}")]
    Invalid(String),
}
```

```rust
// crates/config/src/schema.rs
use serde::{Deserialize, Serialize};

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqBandConfig {
    pub kind: String,      // "peaking" | "lowshelf" | "highshelf"
    pub freq_hz: f32,
    pub q: f32,
    pub gain_db: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub id: String,                       // "game" | "chat" | "media"
    pub node_name: String,                // e.g. "Arctis_Game"
    pub description: String,
    #[serde(default)]
    pub output_device: Option<String>,    // hardware sink node.name, or None = default
    #[serde(default)]
    pub eq: Vec<EqBandConfig>,            // empty => flat 10-band default at apply time
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteConfig {
    pub app_binary: String,
    pub target_sink: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub channels: Vec<ChannelConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub active_profile: String,
    pub profiles: Vec<Profile>,
}

impl Config {
    /// A valid, ready-to-apply default: one "default" profile with the Sonar 3-channel set,
    /// flat EQ, no routes, following the default output.
    pub fn default_config() -> Self { /* impl */ unimplemented!() }

    pub fn active(&self) -> Result<&Profile, crate::error::ConfigError> { unimplemented!() }
    pub fn profile(&self, name: &str) -> Option<&Profile> { unimplemented!() }
    pub fn profile_mut(&mut self, name: &str) -> Option<&mut Profile> { unimplemented!() }

    /// Structural validation: version supported, active_profile exists, channel ids unique,
    /// EQ within audio bounds. Pure, no I/O.
    pub fn validate(&self) -> Result<(), crate::error::ConfigError> { unimplemented!() }
}
```

```rust
// crates/config/src/lib.rs
pub mod error;
pub mod schema;
pub use error::ConfigError;
pub use schema::{ChannelConfig, Config, EqBandConfig, Profile, RouteConfig, CURRENT_VERSION};
```

**Steps**
1. Edit root `Cargo.toml`: add `"crates/config"` to `members`; add `arctis-config = { path = "crates/config" }`
   to `[workspace.dependencies]`; add `libc = "0.2"` to `[workspace.dependencies]` (used in Task 4).
2. Write `crates/config/Cargo.toml`:
   ```toml
   [package]
   name = "arctis-config"
   version = "0.1.0"
   edition = "2021"

   [dependencies]
   arctis-domain = { workspace = true }
   serde = { workspace = true }
   toml = { workspace = true }
   serde_json = { workspace = true }
   thiserror = { workspace = true }

   [dev-dependencies]
   ```
   (If `thiserror`/`serde_json`/`toml` are not yet `workspace = true` aliases, reference them as in
   sibling crates: `thiserror = "1"`, `serde_json = "1"`, `toml = "0.8"`.)
3. Write FAILING test `crates/config/src/schema.rs` `#[cfg(test)] mod tests`:
   - `default_config_is_valid`: `Config::default_config().validate()` is `Ok`, has version
     `CURRENT_VERSION`, `active_profile == "default"`, exactly 3 channels (`game`,`chat`,`media`).
   - `validate_rejects_unknown_active`: a config whose `active_profile` names a missing profile →
     `Err(ConfigError::ProfileNotFound(_))`.
   - `validate_rejects_bad_version`: `version = 999` → `Err(UnsupportedVersion { found: 999, .. })`.
   - `toml_round_trips`: `toml::to_string(&cfg)` then `toml::from_str` equals original.
4. Run `~/.cargo/bin/cargo test -p arctis-config` — confirm compile/assert FAILURE.
5. Implement the `impl Config` methods and `default_config` (mirror `ChannelSetConfig::default_sonar`
   ids/node_names: `game`/`Arctis_Game`, `chat`/`Arctis_Chat`, `media`/`Arctis_Media`).
6. Run tests — confirm PASS.
7. `~/.cargo/bin/cargo build` (whole workspace) — confirm it still builds.
8. Commit: `feat(config): scaffold arctis-config crate with versioned schema + validation`.

---

## Task 2 — config load/save + atomic write + migration (incl. routes.json import) — TDD with temp paths

**Files**
- `crates/config/src/store.rs` (new)
- `crates/config/src/migrate.rs` (new)
- `crates/config/src/lib.rs` (edit: `pub mod store; pub mod migrate;` + re-exports)
- `crates/config/tests/store_roundtrip.rs` (new integration test)
- `crates/config/tests/fixtures/routes.json` (new fixture)
- `crates/config/tests/fixtures/config_v0.toml` (new fixture)

**Interfaces** (exact)

```rust
// crates/config/src/store.rs
use std::path::{Path, PathBuf};
use crate::{error::ConfigError, schema::Config};

/// Resolve the config dir: env `ASM_CONFIG_HOME` if set, else `$HOME/.config/arctis-sound-manager`.
pub fn config_dir() -> PathBuf { unimplemented!() }
pub fn config_path() -> PathBuf { unimplemented!() }       // <dir>/config.toml
pub fn legacy_routes_path() -> PathBuf { unimplemented!() } // <dir>/routes.json

/// Load config from an explicit path. If the file is absent, returns Config::default_config()
/// (after attempting routes.json import from the same dir). Runs migration on raw text first.
pub fn load_from(path: &Path) -> Result<Config, ConfigError> { unimplemented!() }

/// Load from the resolved `config_path()`.
pub fn load() -> Result<Config, ConfigError> { unimplemented!() }

/// Atomically write: serialize to TOML, write `<path>.tmp`, fsync-then-rename into place.
/// Creates parent dirs. Validates before writing.
pub fn save_to(path: &Path, cfg: &Config) -> Result<(), ConfigError> { unimplemented!() }
pub fn save(cfg: &Config) -> Result<(), ConfigError> { unimplemented!() }
```

```rust
// crates/config/src/migrate.rs
use crate::{error::ConfigError, schema::Config};

/// Inspect raw TOML, read its `version` (absent/0 = v0), and migrate forward to CURRENT_VERSION.
/// v0 -> v1: wrap a flat/legacy doc into the profile-bearing schema (stub: if it already parses as
/// v1, no-op; if it's a pre-version doc, build a single "default" profile from any channel data).
pub fn migrate_str(raw: &str) -> Result<Config, ConfigError> { unimplemented!() }

/// Import an existing routes.json (Plan 4 format: [{ "app_binary", "target_sink" }, ...]) into the
/// given profile's `routes`. Missing file => no-op Ok. Used once during initial load when config
/// is absent. Returns number of rules imported.
pub fn import_routes_json(profile: &mut crate::schema::Profile, routes_json_path: &std::path::Path)
    -> Result<usize, ConfigError> { unimplemented!() }
```

**Fixtures**

`crates/config/tests/fixtures/routes.json`:
```json
[
  { "app_binary": "firefox", "target_sink": "Arctis_Media" },
  { "app_binary": "discord", "target_sink": "Arctis_Chat" }
]
```

`crates/config/tests/fixtures/config_v0.toml` (a pre-version doc — no `version`, no `profiles`):
```toml
active_profile = "default"

[[channels]]
id = "game"
node_name = "Arctis_Game"
description = "Game"

[[channels]]
id = "chat"
node_name = "Arctis_Chat"
description = "Chat"
```

**Steps**
1. Write FAILING `crates/config/tests/store_roundtrip.rs`:
   - `save_then_load_roundtrips`: set `ASM_CONFIG_HOME` to a `tempdir`; `save(&default_config())`;
     `load()` equals it; assert `config_path()` exists and `<path>.tmp` does NOT remain.
   - `load_absent_returns_default_and_imports_routes`: tempdir with only `routes.json` (copy fixture);
     `load()` returns default config whose active profile has 2 routes
     (`firefox→Arctis_Media`, `discord→Arctis_Chat`).
   - `migrate_v0_to_v1`: `migrate_str(include_str!("fixtures/config_v0.toml"))` yields a v1 config
     with `version == 1`, one `default` profile containing the 2 channels.
   - `atomic_write_no_partial`: after `save_to`, the temp sibling is gone and the target parses.
   - (use a serialized guard or unique tempdirs per test — `ASM_CONFIG_HOME` is process-global; prefer
     `load_from`/`save_to` with explicit paths in tests to avoid env races; keep ONE env-based test
     behind a mutex.)
2. Run `~/.cargo/bin/cargo test -p arctis-config` — confirm FAILURE.
3. Implement `store.rs` (atomic write helper: write tmp, `File::sync_all`, `rename`) and `migrate.rs`.
   `load_from`: read text → if absent, build default + `import_routes_json` → else `migrate_str`.
4. Run tests — confirm PASS.
5. Commit: `feat(config): atomic load/save + v0→v1 migration + routes.json import (temp-path tested)`.

---

## Task 3 — profile model operations: switch / save / new on the Config — TDD (pure)

**Files**
- `crates/config/src/profile_ops.rs` (new)
- `crates/config/src/lib.rs` (edit: `pub mod profile_ops;` + re-exports)

**Interfaces** (exact) — pure functions on `Config`, no I/O:

```rust
// crates/config/src/profile_ops.rs
use crate::{error::ConfigError, schema::{Config, Profile}};

impl Config {
    /// Set active_profile to `name`; error if it doesn't exist.
    pub fn switch_profile(&mut self, name: &str) -> Result<(), ConfigError> { unimplemented!() }

    /// Create a new profile by cloning the active one under a new name; becomes active.
    /// Error if `name` already exists. Returns a ref to the new profile.
    pub fn new_profile_from_active(&mut self, name: &str) -> Result<&Profile, ConfigError> { unimplemented!() }

    /// Overwrite (upsert) a profile by name. If active name matches, replaces it in place.
    pub fn upsert_profile(&mut self, profile: Profile) { unimplemented!() }

    pub fn profile_names(&self) -> Vec<String> { unimplemented!() }
}
```

**Steps**
1. Write FAILING `#[cfg(test)] mod tests` in `profile_ops.rs`:
   - `switch_to_missing_errors`: `switch_profile("nope")` → `Err(ProfileNotFound)`.
   - `new_from_active_clones_and_activates`: start from default; `new_profile_from_active("gaming")`;
     active is now `"gaming"`; `profile_names()` contains both; channels equal the cloned set.
   - `new_with_dup_name_errors`: `new_profile_from_active("default")` → `Err(Invalid(_))`.
   - `upsert_replaces`: mutate a clone's channel EQ, `upsert_profile`, assert stored profile reflects it.
2. Run `~/.cargo/bin/cargo test -p arctis-config` — confirm FAILURE.
3. Implement.
4. Run tests — confirm PASS.
5. Commit: `feat(config): profile switch/new/upsert operations (pure)`.

---

## Task 4 — `arctis-engine` scaffold + error type + ChildOwner / `spawn_owned` runner-seam extension — TDD with MockRunner

**Files**
- `Cargo.toml` (root — already edited in Task 1 to add `arctis-engine` member + `libc` dep; add the
  member line here if not done: `"crates/engine"`, and `arctis-engine = { path = "crates/engine" }`)
- `crates/audio/src/runner.rs` (edit: extend `CommandRunner` trait + impls)
- `crates/audio/src/lib.rs` (edit: export `ChildToken` if added here)
- `crates/engine/Cargo.toml` (new)
- `crates/engine/src/lib.rs` (new)
- `crates/engine/src/error.rs` (new)
- `crates/engine/src/children.rs` (new — `ChildOwner`)

**Interfaces** (exact)

Extend the runner seam in `arctis-audio` (this is the deliberate, reviewed extension of the proven
seam — keep `spawn_detached` for back-compat):

```rust
// crates/audio/src/runner.rs  (additions)

/// Opaque handle to an owned child process group. The real runner stores the OS pid/pgid;
/// the mock stores a synthetic id. Killing happens through `CommandRunner::kill_owned`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildToken {
    pub pgid: i32,        // process group id (== child pid for a fresh group); 0 for mock
    pub label: String,    // human/debug label, e.g. the conf path
}

pub trait CommandRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CmdOutput, AudioError>;
    fn spawn_detached(&mut self, program: &str, args: &[&str]) -> Result<(), AudioError>;

    /// Spawn a child in ITS OWN process group and return a token the caller stores.
    /// Real impl: Command::new(program).args(args).process_group(0).spawn(); token.pgid = child pid.
    fn spawn_owned(&mut self, program: &str, args: &[&str]) -> Result<ChildToken, AudioError>;

    /// Terminate the process group named by the token: libc::kill(-pgid, SIGTERM).
    /// Idempotent: a token for an already-dead group is Ok. Mock records the call.
    fn kill_owned(&mut self, token: &ChildToken) -> Result<(), AudioError>;
}
```

- `RealRunner::spawn_owned`: build `std::process::Command`, call
  `std::os::unix::process::CommandExt::process_group(0)`, `.spawn()`, capture `child.id()` as `pgid`,
  return `ChildToken { pgid: child.id() as i32, label: args.join(" ") }`. (Do not store the `Child`
  in the runner; ownership of *liveness* is the engine's `ChildOwner`, but the *kill* is by pgid so
  the runner need not retain the handle. Optionally `std::mem::forget`/drop the `Child` after
  recording pid — document that reaping is via kill, and add `child.try_wait()` reaping in
  `kill_owned` if you keep the handle. Simplest correct choice: drop the `Child`, kill by pgid.)
- `RealRunner::kill_owned`: `unsafe { libc::kill(-token.pgid, libc::SIGTERM); }` then return Ok;
  treat ESRCH (no such process) as success.
- `MockRunner`: extend with `pub spawned: Vec<Vec<String>>` and `pub killed: Vec<ChildToken>`;
  `spawn_owned` pushes `[program, args...]`, returns `ChildToken { pgid: 0, label }`; `kill_owned`
  pushes the token. Update the `impl<R: CommandRunner + ?Sized> CommandRunner for &mut R` blanket impl
  to forward both new methods.

Engine error + ChildOwner:

```rust
// crates/engine/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Config(#[from] arctis_config::ConfigError),
    #[error(transparent)]
    Audio(#[from] arctis_audio::AudioError),
    #[error("device: {0}")]
    Device(String),       // wrap arctis_device::TransportError as String (device is best-effort)
    #[error("reconcile failed: {0}")]
    Reconcile(String),
    #[error("ipc error: {0}")]
    Ipc(String),
    #[error("bad request: {0}")]
    BadRequest(String),
}
```

```rust
// crates/engine/src/children.rs
use arctis_audio::{ChildToken, CommandRunner};

/// Tracks every pipewire child the engine spawned, killing them on teardown / Drop.
#[derive(Default)]
pub struct ChildOwner {
    tokens: Vec<ChildToken>,
}

impl ChildOwner {
    pub fn new() -> Self { Self::default() }
    pub fn track(&mut self, token: ChildToken) { self.tokens.push(token); }
    pub fn len(&self) -> usize { self.tokens.len() }

    /// Kill all tracked groups via the runner; clears the list. Idempotent.
    pub fn kill_all<R: CommandRunner>(&mut self, runner: &mut R) -> Result<(), arctis_audio::AudioError> {
        unimplemented!()
    }
}
// NOTE on Drop: because kill requires a runner, ChildOwner does NOT impl Drop directly with a runner.
// Instead the engine's `shutdown()` (and the engine's own Drop, holding the runner) calls kill_all.
// Document this; the deterministic guarantee is engine-Drop calling kill_all on its owned runner.
```

**Steps**
1. Root `Cargo.toml`: ensure `"crates/engine"` in members, `arctis-engine` + `libc` workspace deps.
2. Write `crates/engine/Cargo.toml`:
   ```toml
   [package]
   name = "arctis-engine"
   version = "0.1.0"
   edition = "2021"

   [dependencies]
   arctis-domain  = { workspace = true }
   arctis-device  = { workspace = true }
   arctis-audio   = { workspace = true }
   arctis-config  = { workspace = true }
   thiserror = "1"
   serde = { workspace = true }
   serde_json = "1"
   ```
3. FAILING test in `crates/audio/src/runner.rs` tests: `mock_spawn_owned_records_and_returns_token`
   (`spawn_owned("pipewire", &["-c","/tmp/x.conf"])` pushes to `spawned`, returns `pgid == 0`);
   `mock_kill_owned_records` (kill pushes token to `killed`).
4. Run `~/.cargo/bin/cargo test -p arctis-audio` — confirm FAILURE (trait method missing).
5. Implement the trait extension on `CommandRunner`, `RealRunner`, `MockRunner`, and the `&mut R`
   blanket impl. Export `ChildToken` from `arctis-audio` `lib.rs`. Add `libc` to `crates/audio/Cargo.toml`
   deps (`libc = { workspace = true }`).
6. Run `-p arctis-audio` tests — confirm PASS (existing audio tests still green).
7. FAILING test in `crates/engine/src/children.rs` tests: `child_owner_kills_all_via_runner` — track
   2 tokens, `kill_all(&mut MockRunner::new())`, assert `runner.killed.len() == 2` and `owner.len()==0`.
8. Run `~/.cargo/bin/cargo test -p arctis-engine` — confirm FAILURE.
9. Implement `error.rs`, `children.rs`, and `lib.rs` (`pub mod error; pub mod children;
   pub use error::EngineError; pub use children::ChildOwner;`).
10. Run `-p arctis-engine` tests — confirm PASS. `~/.cargo/bin/cargo build` — confirm workspace builds.
11. Commit: `feat(engine,audio): owned-child spawn seam (process-group kill) + engine scaffold/errors`.

---

## Task 5 — Engine reconcile/apply (config → graph via ChannelManager/Router/EQ) — TDD asserting argv sequence with MockRunner

**Files**
- `crates/engine/src/engine.rs` (new — the `Engine` type + `reconcile`)
- `crates/engine/src/convert.rs` (new — config↔audio type mapping)
- `crates/engine/src/lib.rs` (edit: `pub mod engine; pub mod convert; pub use engine::Engine;`)

**Interfaces** (exact)

```rust
// crates/engine/src/convert.rs
use arctis_audio::{BandKind, ChannelDef, ChannelSetConfig, EqBand, EqModel};
use arctis_config::{ChannelConfig, Config, EqBandConfig, RouteConfig};
use arctis_audio::RouteRule;
use crate::error::EngineError;

pub fn band_kind_from_str(s: &str) -> Result<BandKind, EngineError> { unimplemented!() }
pub fn eq_band_from_cfg(c: &EqBandConfig) -> Result<EqBand, EngineError> { unimplemented!() }
/// Empty cfg eq => EqModel::default_10band(); else map each band.
pub fn eq_model_for(channel: &ChannelConfig) -> Result<EqModel, EngineError> { unimplemented!() }
pub fn channel_def_from_cfg(c: &ChannelConfig) -> ChannelDef { unimplemented!() }
pub fn channel_set_from_profile(p: &arctis_config::Profile) -> ChannelSetConfig { unimplemented!() }
pub fn route_rules_from_profile(p: &arctis_config::Profile) -> Vec<RouteRule> { unimplemented!() }
```

```rust
// crates/engine/src/engine.rs
use arctis_audio::{ChannelManager, CommandRunner, Router};
use arctis_config::Config;
use crate::{children::ChildOwner, error::EngineError};

/// A reconcile-step descriptor used for pure planning + test assertions before any I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileStep {
    ChannelsUp,
    ApplyEq { channel_id: String },
    SetOutput { channel_id: String, device: Option<String> },
    RouteSet { app_binary: String, target_sink: String },
}

/// Pure planner: compute the ordered step list for a profile (no I/O). Lets us unit-test the
/// reconcile PLAN independently of execution (G8).
pub fn plan_reconcile(cfg: &Config) -> Result<Vec<ReconcileStep>, EngineError> { unimplemented!() }

pub struct Engine<R: CommandRunner> {
    runner: R,                // the ONE runner shared via &mut into sub-managers (G1 seam)
    config: Config,
    children: ChildOwner,
}

impl<R: CommandRunner> Engine<R> {
    pub fn new(runner: R, config: Config) -> Self { unimplemented!() }
    pub fn config(&self) -> &Config { &self.config }

    /// Bring the live graph to match the active profile. Idempotent. Order:
    ///   1. ChannelManager::up(default flat eq) — creates sinks, tracking spawn_owned tokens
    ///   2. per channel: AudioBackend/ChannelManager apply_all(eq_model_for(channel))
    ///   3. per channel with output_device: ChannelManager::set_output(...)
    ///   4. Router: set_rule for each route, save_persistent, then apply_live best-effort
    /// Reuses ChannelManager/Router/AudioBackend — does NOT reimplement.
    pub fn reconcile(&mut self) -> Result<(), EngineError> { unimplemented!() }

    /// Kill all owned pipewire children. Called on shutdown and from Drop.
    pub fn shutdown(&mut self) -> Result<(), EngineError> { unimplemented!() }
}

impl<R: CommandRunner> Drop for Engine<R> {
    fn drop(&mut self) { let _ = self.children.kill_all(&mut self.runner); }
}
```

**Reuse note for the implementer (no PipeWire knowledge needed):** construct
`ChannelManager::new(&mut self.runner, channel_set_from_profile(active))` so the engine's single
runner is borrowed (the `&mut R: CommandRunner` blanket impl makes this work). `ChannelManager::up`
internally calls `AudioBackend::create`, which is where `spawn_owned` now fires; capture returned
`ConfHandle`s. For tracking child tokens: the simplest wiring is to have `ChannelManager::up`/`create`
already use `spawn_owned` (Task 4 changed the seam), and have them **return** the `ChildToken`s OR
expose them — if threading tokens out of `ChannelManager` is intrusive, an acceptable alternative is:
`Engine` queries the MockRunner's recorded `spawned` calls is NOT viable for Real; instead extend
`AudioBackend::create`/`ChannelManager::up` signatures to return `(ConfHandle, Option<ChildToken>)`.
**Decide in this task and document:** preferred = `create` returns `ConfHandle` and the spawn happens
through a runner the engine owns, so the engine reads `ChildToken`s back by having `spawn_owned` also
recorded — cleanest is to thread tokens through return values. Pick the minimal change and assert it.

**Steps**
1. FAILING pure test in `engine.rs`: `plan_reconcile_orders_steps` — default config (3 channels, no
   eq, no output overrides, no routes) yields `[ChannelsUp, ApplyEq{game}, ApplyEq{chat}, ApplyEq{media}]`
   (no SetOutput, no RouteSet). A second profile with `media.output_device = Some("speakers")` and one
   route `firefox→Arctis_Media` yields the SetOutput + RouteSet steps appended in order.
2. Run `~/.cargo/bin/cargo test -p arctis-engine` — confirm FAILURE.
3. Implement `convert.rs` + `plan_reconcile`.
4. Run — confirm PASS. Commit checkpoint optional.
5. FAILING argv-sequence test `reconcile_emits_expected_argv` using `MockRunner`:
   - Build `Engine::new(MockRunner::new().with_output(0,"","")...)` queued so each `pw-cli ls Node`
     returns a stub listing the channel `node.name`s with ids (reuse audio's existing fixture shape;
     queue enough `CmdOutput`s). Profile: default 3 channels.
   - After `reconcile()`, inspect the MockRunner call log (`calls`, `spawned`) and assert the ORDER:
     for each channel a `pw-cli ls Node` existence check, then a `spawn_owned("pipewire", ["-c", ...])`
     for sinks that don't exist (channels up), then `pw-cli s <id> Props <json>` calls (eq apply_all),
     and finally — for the routed profile variant — `pw-metadata` live-move argv. Assert NO real
     `pkill` is used for teardown (teardown uses `kill_owned`).
   - `reconcile_is_idempotent`: queue `ls Node` outputs that REPORT the sinks already present; assert
     NO `spawn_owned` calls happen on the second path (create is a no-op), EQ still applied.
   - `shutdown_kills_tracked_children`: after a reconcile that spawned N sinks, `shutdown()` →
     `runner.killed.len() == N`.
6. Run — confirm FAILURE (engine logic not implemented).
7. Implement `reconcile` + `shutdown` by composing `ChannelManager` + `Router` (+ `eq_model_for`).
   Track each `ChildToken` into `self.children`.
8. Run — confirm PASS. `~/.cargo/bin/cargo build` — confirm workspace builds.
9. Commit: `feat(engine): idempotent reconcile composing channels/EQ/routing + deterministic teardown`.

---

## Task 6 — Profile switching + EQ/route mutation through the Engine (persists to config) — TDD with MockRunner

**Files**
- `crates/engine/src/engine.rs` (edit: add mutation + switch methods)
- `crates/engine/src/state.rs` (new — the UI-agnostic state + event types)
- `crates/engine/src/lib.rs` (edit: `pub mod state;` + re-exports)

**Interfaces** (exact)

```rust
// crates/engine/src/state.rs
use serde::{Deserialize, Serialize};

/// A flat, UI-agnostic snapshot the CLI/daemon/(future UI) render.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineState {
    pub active_profile: String,
    pub profiles: Vec<String>,
    pub channels: Vec<ChannelSnapshot>,
    pub routes: Vec<(String, String)>,        // (app_binary, target_sink)
    pub device_present: bool,
    pub device_fields: std::collections::BTreeMap<String, String>, // best-effort, may be empty
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelSnapshot {
    pub id: String,
    pub node_name: String,
    pub output_device: Option<String>,
    pub eq_bands: usize,
}

/// Events emitted on the engine's outbound stream (mpsc::Receiver<Event> for the daemon/UI).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    ProfileSwitched { name: String },
    Reconciled,
    EqBandSet { channel_id: String, band: usize },
    RouteSet { app_binary: String, target_sink: String },
    DeviceState { fields: std::collections::BTreeMap<String, String> },
}
```

```rust
// crates/engine/src/engine.rs  (additions to impl Engine<R>)
use crate::state::{EngineState, Event};

impl<R: CommandRunner> Engine<R> {
    pub fn state(&self) -> EngineState { unimplemented!() }      // pure snapshot, no device I/O

    /// Set the engine's event sink. Events are pushed here (daemon owns the Receiver).
    pub fn set_event_sink(&mut self, tx: std::sync::mpsc::Sender<Event>) { unimplemented!() }

    /// Switch active profile in config, persist, then reconcile the graph to it.
    pub fn switch_profile(&mut self, name: &str) -> Result<(), EngineError> { unimplemented!() }

    /// Mutate one EQ band in the active profile's channel, persist config, apply live via audio.
    pub fn set_eq_band(&mut self, channel_id: &str, band: usize,
                       cfg: arctis_config::EqBandConfig) -> Result<(), EngineError> { unimplemented!() }

    /// Add/upsert a route in the active profile, persist, set_rule + save_persistent + apply_live.
    pub fn set_route(&mut self, app_binary: &str, target_sink: &str) -> Result<(), EngineError> { unimplemented!() }

    /// Persist the in-memory config via arctis_config::store::save (path from ASM_CONFIG_HOME/HOME).
    pub fn save_config(&self) -> Result<(), EngineError> { unimplemented!() }

    /// Best-effort device status read; never errors the caller (returns empty on failure).
    pub fn refresh_device(&mut self) -> std::collections::BTreeMap<String, String> { unimplemented!() }
}
```

**Steps**
1. FAILING tests in `engine.rs` using `MockRunner` + a tempdir `ASM_CONFIG_HOME`:
   - `state_reflects_active_profile`: default engine `state().active_profile == "default"`, 3 channels.
   - `switch_profile_persists_and_reconciles`: seed a 2-profile config; `switch_profile("gaming")` →
     `config().active_profile == "gaming"`, the on-disk config file now has `active_profile = "gaming"`,
     and the MockRunner shows a reconcile happened (channel `ls Node` + eq Props calls), event
     `ProfileSwitched { name: "gaming" }` received on the channel.
   - `switch_unknown_errors`: `switch_profile("nope")` → `Err(Config(ProfileNotFound))`, NO disk write.
   - `set_eq_band_persists_and_applies_live`: `set_eq_band("game", 2, band)` updates active profile on
     disk and emits a `pw-cli s <id> Props` argv for band 2; event `EqBandSet` received.
   - `set_route_persists`: `set_route("firefox","Arctis_Media")` adds to active profile + routes.json
     equivalent saved; MockRunner shows a `pw-metadata` live move; event `RouteSet` received.
2. Run `~/.cargo/bin/cargo test -p arctis-engine` — confirm FAILURE.
3. Implement, reusing `Router::set_rule/apply_live/save_persistent` and `AudioBackend::apply_band`
   (via `ChannelManager`/direct `AudioBackend` on the channel's `SinkSpec`). Persist with
   `arctis_config::store::save`. Push events through the optional `Sender` (ignore send errors).
4. Run — confirm PASS. `~/.cargo/bin/cargo build`.
5. Commit: `feat(engine): profile switch + live EQ/route mutation persisted to unified config + events`.

---

## Task 7 — Resident daemon (`asm-cli daemon`) + profile/apply CLI + coexistence detection + OWNER-RUN E2E

**Files**
- `crates/cli/Cargo.toml` (edit: add `arctis-config`, `arctis-engine` deps)
- `crates/cli/src/main.rs` (edit: new subcommands; route existing channel/eq/route through engine where sensible)
- `crates/cli/src/daemon.rs` (new — resident loop + UnixListener protocol + client)
- `crates/cli/src/coexist.rs` (new — legacy-stack detection)

**Interfaces** (exact)

New clap subcommands (added to the existing `Command` enum — keep all existing variants):

```rust
/// Profile management (reads/writes the unified config).
Profile {
    #[command(subcommand)]
    action: ProfileAction,
},
/// Reconcile the live graph to the active profile in config.
Apply,
/// Run the resident daemon: load config, apply, own pipewire children, serve control socket.
Daemon {
    /// Run in foreground (default); reserved flag for future detach.
    #[arg(long, default_value_t = true)]
    foreground: bool,
},

#[derive(Subcommand, Debug)]
enum ProfileAction {
    List,
    Show { name: Option<String> },          // default: active
    Switch { name: String },
    Save,                                    // persist current in-memory config
    New { name: String },                    // clone active → name, activate
}
```

Daemon protocol (newline-delimited JSON). Requests (one per line):

```jsonc
{"cmd":"get-state"}
{"cmd":"switch-profile","name":"gaming"}
{"cmd":"set-eq-band","channel":"game","band":2,"kind":"peaking","freq_hz":1000,"q":1.0,"gain_db":-3.0}
{"cmd":"route","app_binary":"firefox","target_sink":"Arctis_Media"}
{"cmd":"reload"}
{"cmd":"shutdown"}
```

Responses (one per line): `{"ok":true,"state":{...EngineState...}}` or `{"ok":false,"error":"..."}`.

```rust
// crates/cli/src/daemon.rs
use arctis_engine::{Engine, EngineError};
use arctis_audio::RealRunner;

/// Resolve the control socket path (XDG_RUNTIME_DIR or /tmp fallback).
pub fn socket_path() -> std::path::PathBuf { unimplemented!() }

/// Parse one request line into an internal command enum (pure, serde_json). Public for unit tests.
#[derive(Debug, PartialEq, serde::Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum Request {
    GetState,
    SwitchProfile { name: String },
    SetEqBand { channel: String, band: usize, kind: String, freq_hz: f32, q: f32, gain_db: f32 },
    Route { app_binary: String, target_sink: String },
    Reload,
    Shutdown,
}

#[derive(Debug, serde::Serialize)]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<arctis_engine::EngineState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Apply one parsed request to the engine, returning the response (pure-ish: no socket I/O).
/// Public so it can be unit-tested with a MockRunner-backed Engine.
pub fn handle_request<R: arctis_audio::CommandRunner>(
    engine: &mut Engine<R>, req: Request,
) -> Response { unimplemented!() }

/// Run the resident daemon: bind UnixListener, load+apply config, serve until `shutdown`.
/// On exit (or signal/Drop) the Engine's Drop kills owned children. Removes the socket file.
pub fn run_daemon() -> Result<(), EngineError> { unimplemented!() }

/// One-shot client: connect, send a single Request line, return the Response. Used so CLI
/// commands talk to a running daemon if the socket exists, else fall back to a direct Engine call.
pub fn send_request(req: &Request) -> Result<Response, EngineError> { unimplemented!() }
```

```rust
// crates/cli/src/coexist.rs
/// Best-effort detection of the legacy stack (ARCHITECTURE G10). No teardown of THEIR objects.
#[derive(Debug, Default, PartialEq)]
pub struct LegacyReport {
    pub legacy_loopbacks: Vec<String>,   // Arctis_Game/Chat/Media loopback node names seen
    pub hrir_switch_present: bool,        // ~/.local/bin/hrir-switch exists
    pub rpm_daemon_running: bool,         // process scan (best-effort)
}

/// Scan a provided `pw-dump`/`pw-cli ls Node` text + a HOME path for legacy markers (pure; testable).
pub fn detect_from(node_list_stdout: &str, home: &std::path::Path) -> LegacyReport { unimplemented!() }

/// Returns a human warning string if anything was detected, else None.
pub fn warning(report: &LegacyReport) -> Option<String> { unimplemented!() }
```

**CLI wiring rules (keep existing low-level commands working):**
- `profile list/show/switch/save/new`, `apply`: build a `RealRunner` + load config via
  `arctis_config::store::load`, construct `Engine`, call the matching method, print result.
  If a daemon socket exists, `switch`/`apply` prefer `send_request` (so the resident owner re-applies).
- `eq set`, `route set`, `channel output set` (EXISTING): keep their current direct behavior, but ALSO
  route through `Engine::set_eq_band`/`set_route`/`set_output`+`save_config` so the change PERSISTS to
  the unified config (no more silent non-persistence). The low-level `sink`/`channels up|down` stay
  as-is (they're the raw escape hatch).
- On `daemon` startup, run `coexist::detect_from(...)` and print `warning(...)` if any.

**Steps**
1. Edit `crates/cli/Cargo.toml`: add `arctis-config = { workspace = true }`,
   `arctis-engine = { workspace = true }`, `serde = { workspace = true }`, `serde_json = "1"`.
2. FAILING unit tests in `daemon.rs`:
   - `parse_get_state` / `parse_switch` / `parse_set_eq_band` / `parse_route` / `parse_shutdown`:
     `serde_json::from_str::<Request>(line)` equals the expected variant for each protocol line above.
   - `handle_switch_returns_state`: Engine over `MockRunner` (tempdir config, 2 profiles);
     `handle_request(&mut engine, Request::SwitchProfile{name:"gaming"})` → `Response{ok:true,..}`
     with `state.active_profile == "gaming"`.
   - `handle_unknown_profile_errors`: `Response{ok:false, error: Some(_)}`.
3. FAILING unit tests in `coexist.rs`:
   - `detects_loopbacks`: feed a `ls Node` stub containing `Arctis_Game`/`Arctis_Chat` → report lists
     them; `warning` is `Some`.
   - `clean_system`: empty stub + tempdir HOME without `hrir-switch` → empty report; `warning` None.
4. Run `~/.cargo/bin/cargo test -p arctis-cli` — confirm FAILURE.
5. Implement `daemon.rs` (`Request`/`Response`/`handle_request`/`socket_path`/`send_request`/
   `run_daemon` accept loop reading lines, dispatching `handle_request`, writing JSON lines; remove
   stale socket on bind; break loop on `Shutdown`) and `coexist.rs`.
6. Add the clap variants + dispatch in `main.rs`; wire existing eq/route/channel through the engine
   for persistence. Run `~/.cargo/bin/cargo test -p arctis-cli` — confirm PASS.
7. `~/.cargo/bin/cargo build` whole workspace — confirm green; `~/.cargo/bin/cargo clippy` if available.
8. Commit: `feat(cli): resident daemon (UnixListener) + profile/apply subcommands + coexistence detect`.

### Task 7-E2E — OWNER-RUN (manual, real PipeWire; OUT OF CI — do not auto-execute)

> Hand this to the human owner. Record actual observed output; do not fabricate.

1. Build: `~/.cargo/bin/cargo build --release`.
2. Author a 2-profile `config.toml` (e.g. `default` = all → headset flat; `media-to-speakers` = Media
   channel `output_device` set to a real speaker sink node.name, plus a `firefox→Arctis_Media` route).
   Place at `~/.config/arctis-sound-manager/config.toml` (back up any existing one first).
3. `asm-cli apply` — verify with `pw-dump`/`pw-cli ls Node` that `Arctis_Game/Chat/Media` sinks exist
   and Media feeds the speaker sink. Confirm live EQ via `pw-cli s <id> Props`.
4. Start the daemon: `asm-cli daemon` (foreground in one terminal).
5. In another terminal: `asm-cli profile switch media-to-speakers` — confirm the daemon re-applies and
   the graph changes (Media retargets to speakers) via `pw-dump`.
6. Note the `pipewire -c` child PIDs (`pgrep -af 'pipewire -c'`). Stop the daemon (Ctrl-C / send
   `{"cmd":"shutdown"}` via `socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/arctis-sound-manager.sock`).
   **Confirm ALL `pipewire -c` children are gone** (`pgrep -af 'pipewire -c'` empty) — verifying the
   process-group teardown (the key improvement over `pkill`).
7. Restart `asm-cli daemon` — confirm it re-applies the active profile on start (re-creates sinks,
   re-applies EQ) without manual steps (the re-apply-on-startup requirement).
8. Confirm no orphaned sinks / clean teardown after a final shutdown. Restore the backed-up config.

---

## Self-Review

**Spec coverage:**
- §9 single source of truth + schema-versioned + migrations → Tasks 1–3 (`Config.version`,
  `migrate_str`, `import_routes_json`). routes.json ABSORBED (Task 2). ✔ (G4)
- §6 re-apply-on-startup (runtime Props not persisted) → `Engine::reconcile` re-applies all EQ; daemon
  calls it on start (Task 7-E2E step 7). ✔ (G3)
- §4 engine composes device+audio+config, async UI-agnostic API + event stream → `Engine` methods +
  `mpsc::Sender<Event>` + `EngineState` (Tasks 5–6); no `tauri`/`tokio`. ✔
- §10 coexistence detection + own clean teardown → `coexist.rs` (detect+warn) + process-group kill
  on shutdown/Drop (Tasks 4–7). ✔ (G10)
- G1 reuse: engine composes `ChannelManager`/`AudioBackend`/`Router`/`EqModel`/`Registry`; only the
  runner SEAM is extended (`spawn_owned`/`kill_owned`), not audio logic. ✔
- G3 idempotency: `reconcile_is_idempotent` test; 48 kHz inherited from `audio`. ✔
- G7 typed errors: `ConfigError`, `EngineError` (wraps audio/config/device); no unwrap on runtime. ✔
- G8 testing: pure planner (`plan_reconcile`), config schema/migration unit-tested with temp paths,
  argv-sequence asserted over `MockRunner`; real PipeWire is OWNER-RUN/out-of-CI. ✔

**Placeholder scan:** all interface bodies are `unimplemented!()`/`/* impl */` by design (TDD targets);
NO TODO/FIXME left in shipped code — each is filled in its task's implement step. Real config fixtures
(`routes.json`, `config_v0.toml`) and exact protocol JSON lines are concrete, not placeholders.

**Type consistency:** `EqBandConfig`(config) ↔ `EqBand`/`BandKind`(audio) bridged by `convert.rs`;
`RouteConfig`(config) ↔ `RouteRule`(audio) bridged by `route_rules_from_profile`;
`ChannelConfig`(config) ↔ `ChannelDef`/`ChannelSetConfig`(audio) bridged by `channel_set_from_profile`.
`ChildToken` defined once in `arctis-audio`, used by `ChildOwner`/`Engine`. EQ bounds are inherited
from `audio` constants, not redefined.

**Open questions / on-machine items (resolve in OWNER-RUN, not by guessing):**
- Whether `AudioBackend::create`/`ChannelManager::up` should *return* `ChildToken`s vs. the engine
  reading them another way — Task 5 forces a decision; preferred = thread tokens through return values
  (minimal, testable). Confirm the chosen signature change keeps existing audio tests green.
- Exact real speaker sink `node.name` for the E2E 2-profile config is environment-specific (owner fills).
- Signal handling for the daemon (SIGINT/SIGTERM) beyond Ctrl-C: relying on `Engine::Drop` covers the
  common path; a future task may add an explicit signal handler — out of scope here.

**Note:** Hardware/daemon real-PipeWire E2E (Task 7-E2E) is OWNER-RUN and OUT OF CI. All other tasks
are fully unit-testable with `MockRunner`/temp paths and run in CI.
