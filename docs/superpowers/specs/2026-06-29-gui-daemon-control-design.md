# GUI Daemon Control — Design Spec

**Date:** 2026-06-29
**Status:** Approved (owner) — hybrid model, Device/Settings-page section, idempotent autostart.
**Refs:** `ARCHITECTURE.md` (G1–G10), `DESIGN.md` (Sonar tokens), `packaging/systemd/arctis-sound-manager.service`, project memory ([[daemon-restart-after-engine-changes]], [[no-live-audio-writes-during-debug]]).

## 1. Goal

Let the user **start, stop, and restart the `asm-cli` daemon from the GUI**, see its status at a glance, and optionally **enable autostart at login** — all without dropping to a terminal. Motivation: the daemon is currently run by hand; stopping it requires finding the right terminal/PID.

## 2. Owner decisions

- **D1 — Hybrid management.** If the systemd **user** service is installed/active, drive it via `systemctl --user`. Otherwise spawn/stop the `asm-cli daemon` process directly. Works in both dev (manual) and production.
- **D2 — UI lives in a "Daemon" section on the Device/Settings page** (not a header chip). Status line + Start/Stop/Restart buttons + an "Autostart at login" toggle.
- **D3 — Include autostart install**, and it must be **idempotent**: never create duplicate units/enables; detect existing state; atomically overwrite; the toggle reflects the real `systemctl is-enabled` state.
- **D4 — Design-system aligned UI**: `--ss` tokens, bits-ui `Switch` for the toggle and the existing button styles — visually consistent with the rest of the app (DESIGN.md).

## 3. Architecture

**Key constraint:** daemon control **cannot** go through the daemon's IPC socket — the socket is dead exactly when you need to *start* the daemon. So these are **Tauri commands handled directly in the GUI's own Rust process** (`src-tauri`), independent of the normal daemon-IPC client path.

```
Svelte "Daemon" section ──Tauri invoke──► src-tauri commands ──► daemon_control.rs
                                                                   │
                          ┌────────────────────────────────────────┼─────────────────────┐
                          ▼ (systemd present)                       ▼ (manual fallback)    ▼ (status)
                  systemctl --user start/stop/restart        spawn asm-cli (detached)   socket liveness
                  enable/disable --now                       Shutdown IPC / SIGTERM     + systemctl is-active/is-enabled
```

**Module `src-tauri/src/daemon_control.rs`** — owns all logic behind a **command-runner seam** (a small trait mirroring the engine's `CommandRunner`, with a real impl over `std::process::Command` and a mock for tests). Pure, testable pieces are factored out from the side-effecting subprocess calls:
- **path resolution** (pure): pick the `asm-cli` binary from an ordered candidate list.
- **status parsing** (pure): map `systemctl is-active`/`is-enabled` exit codes + socket liveness → a `DaemonStatus`.
- **arg construction** (pure): build the exact `systemctl --user …` argv and the spawn argv.
- **unit-file rendering** (pure): produce the systemd unit text with the resolved `ExecStart` path.
- **side-effecting ops** (thin): run the constructed commands / write the unit file via the runner.

## 4. Data model

```rust
// src-tauri/src/daemon_control.rs
pub enum ManagedBy { Systemd, Manual, Stopped }   // serde rename_all = "snake_case"

pub struct DaemonStatus {
    pub running: bool,             // socket is live OR systemctl is-active
    pub managed_by: ManagedBy,     // systemd | manual | stopped
    pub autostart_enabled: bool,   // systemctl --user is-enabled == 0
    pub systemd_available: bool,   // `systemctl` exists AND user manager reachable
    pub binary_path: Option<String>, // resolved asm-cli path, or None if not found
    pub unit_installed: bool,      // ~/.config/systemd/user/<unit> exists
}
```

Tauri commands (registered in `src-tauri/src/lib.rs generate_handler!`):
- `daemon_status() -> Result<DaemonStatus, CommandError>`
- `daemon_start() -> Result<DaemonStatus, CommandError>`
- `daemon_stop() -> Result<DaemonStatus, CommandError>`
- `daemon_restart() -> Result<DaemonStatus, CommandError>`
- `daemon_set_autostart(enabled: bool) -> Result<DaemonStatus, CommandError>`

Each mutating command returns the **fresh `DaemonStatus`** (re-queried after the action) so the UI updates from one round-trip. `ipc.ts` mirrors the types + wrappers.

## 5. Operation semantics (hybrid)

Let `systemd = systemd_available && unit_installed`.

- **start:** `systemd` → `systemctl --user start <unit>`; else spawn `<asm-cli> daemon` **detached** (new session/process group via `setsid`-equivalent — `Command.process_group(0)` + `pre_exec setsid`, stdout/stderr to a log file or `/dev/null`) so it outlives the GUI. If already running, no-op (return status).
- **stop:** `systemd` → `systemctl --user stop <unit>`; else send the existing **`Request::Shutdown`** over the socket (graceful, reuses `arctis_client`); if the socket is unreachable but a process exists, fall back to `SIGTERM` by PID (discovered via the socket peer or `pgrep -f 'asm-cli daemon'`). If already stopped, no-op.
- **restart:** `systemd` → `systemctl --user restart <unit>`; else **stop → wait for socket-down (bounded poll) → start**.
- **set_autostart(true):** **idempotent install** (see §6).
- **set_autostart(false):** `systemctl --user disable --now <unit>` (idempotent: disabling an already-disabled unit is a no-op success); leave the unit file in place (so re-enabling is instant) — do NOT delete it.

`<unit>` = `arctis-sound-manager.service`.

## 6. Idempotent autostart install (D3 — explicit)

`daemon_set_autostart(true)` must be safe to click repeatedly and must never duplicate:

1. **Resolve** the `asm-cli` binary path (§7). If not found → typed error "asm-cli binary not found; set $ASM_CLI_BIN".
2. **Render** the unit text from the repo template with `ExecStart=<resolved-path> daemon` substituted (and `%h`/`%t` specifiers preserved).
3. **Compare-then-write:** target = `~/.config/systemd/user/arctis-sound-manager.service`. If the file exists AND its content already equals the rendered text → skip the write (no churn). Otherwise write **atomically** (temp file + rename) — overwriting in place, never creating a second/`.1` copy.
4. `systemctl --user daemon-reload` (only if the file was written/changed).
5. `systemctl --user enable --now arctis-sound-manager.service` — `enable` is idempotent (re-enabling re-points the symlink, no duplicate); `--now` also starts it if stopped. If already enabled+running, this is a clean no-op.
6. Return fresh status (`autostart_enabled: true`).

There is exactly **one** unit file path and **one** enable symlink, so "duplicate installs" are structurally impossible: step 3 overwrites the single file, step 5's `enable` is a single idempotent symlink op. Switching binary location (dev↔installed) just rewrites the same file with the new `ExecStart` and reloads.

## 7. Binary-path resolution (pure)

Ordered candidates, first existing wins:
1. `$ASM_CLI_BIN` (explicit override).
2. Sibling of the running GUI executable (`current_exe().parent()/asm-cli`).
3. `~/.local/bin/asm-cli`.
4. `/usr/bin/asm-cli`.
5. Dev: `<cwd-or-workspace>/target/release/asm-cli`, then `…/target/debug/asm-cli`.

Returns `Option<PathBuf>`. Surfaced in `DaemonStatus.binary_path` so the UI shows what it resolved (and the systemd unit uses it).

## 8. UI (D2 + D4)

A **"Daemon" section** on the Device page (or a Settings page if one exists; otherwise Device):
- **Status line:** a status dot + text — "Running (systemd)" / "Running (manual)" / "Stopped" — plus a muted sub-line with the resolved binary path. Uses `--ss` color tokens (green `--ss-...success`, red/danger for stopped).
- **Buttons:** Start / Stop / Restart, using the app's existing button styles; each disabled when not applicable (Start disabled while running; Stop/Restart disabled while stopped) and shows a busy state during the op.
- **Autostart toggle:** a bits-ui `Switch` ("Start daemon at login"), bound to `autostart_enabled`, calling `daemon_set_autostart`. Disabled with an explanatory hint when `systemd_available == false`.
- **Feedback:** inline message area for op results/errors. Refreshes status after each action and on a light poll/visibility.
- Pure logic (status→label/dot-color, per-button enablement, toggle disabled-reason) lives in `frontend/src/lib/daemonControl.ts` with vitest unit tests (no jsdom); the `.svelte` is a thin view.

## 9. Non-negotiable constraints

- **No device writes** (G2) — this feature only manages a process/service and writes one systemd unit file under `~/.config`. Device-write allowlist untouched.
- Start/stop/restart recreate PipeWire sinks (a live audio change) — acceptable because it is **explicitly button-initiated** (user consent); no automatic/implicit daemon lifecycle changes.
- `tauri` code stays in `src-tauri`; the engine/CLI crates are not modified (the feature reuses the existing `Request::Shutdown` + `arctis_client::socket_path`). If a tiny helper is needed (e.g. exposing the unit name), keep it minimal.
- Typed errors, no `unwrap`/`expect`/`panic` on runtime paths (G7); small focused files (G6); GUI logic in pure testable helpers.
- Idempotent, no-duplicate autostall install (§6).

## 10. Testing

- **Rust (src-tauri), mock runner + temp HOME:** path resolution (candidate ordering, `$ASM_CLI_BIN` override, none-found); status parsing (is-active/is-enabled exit codes + socket-liveness → `ManagedBy`/flags); systemctl/spawn argv construction; unit-file rendering (ExecStart substitution); **idempotency** — install when absent writes once; install when content-identical does NOT rewrite; install with changed path rewrites atomically; enable arg is a single `enable --now`.
- **Frontend (vitest, no jsdom):** `daemonControl.ts` helpers — status→label/dot, button enablement matrix, toggle disabled-reason.
- **Owner-manual-verify (can't unit-test):** actual Start/Stop/Restart against the real daemon; autostart install → `systemctl --user is-enabled` returns enabled, survives logout; double-click install creates no duplicate.

## 11. Out of scope (YAGNI)

- Header status chip (owner chose the page section).
- Live daemon log streaming in the GUI.
- Managing a system-wide (root) service — user service only.
- Auto-starting the daemon on GUI launch without the user enabling autostart.

## 12. Open items

- Which page hosts the section if no dedicated Settings page exists — default to the Device page; confirm during implementation by inspecting the current nav.
- Exact detached-spawn incantation (`process_group` + `setsid` via `pre_exec`) verified against the Tauri runtime during implementation.
