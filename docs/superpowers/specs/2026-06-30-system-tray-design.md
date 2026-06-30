# System Tray for the GUI — Design

**Date:** 2026-06-30
**Status:** Approved (brainstorming), pending implementation plan

## Problem

The Tauri GUI is a foreground window only: closing it (X) quits the GUI process,
and there is no persistent tray presence. The owner wants a system-tray icon
similar to the previous (Python) app — a resident tray entry that keeps the app
reachable, with quick controls and the ability to fully shut down.

## Key architectural context

Unlike the old single-process Python app, this project already has **two
processes**:

1. **The daemon** (`asm-cli`, a systemd *user* service) — the audio engine. It
   runs in the background independent of any window and already autostarts on
   login. It does all EQ/routing/surround/mic DSP work.
2. **The GUI** (this Tauri window) — a thin control client over the daemon.

Therefore the tray is a property of the **GUI window**, not a new background
worker: it lets the window hide instead of quit, be reopened, expose a few quick
daemon-backed actions, and shut everything down. The daemon keeps running while
the window is hidden.

## Requirements (decided during brainstorming)

- **Close (X) → hide to tray.** The window hides; the GUI process stays resident
  with a tray icon. It is never destroyed by X.
- **Tray left-click → toggle window** show/hide (and focus on show).
- **GUI autostart on login → start hidden in the tray** (no window pops up). This
  is the GUI's *own* login entry, separate from the daemon's systemd autostart.
  A settings toggle enables/disables it.
- **Tray right-click menu:**
  - `Show/Hide Window`
  - separator
  - `Mute` — checkable; reflects and toggles master mute (live daemon state)
  - `Profile ▸` — submenu of profiles; the active profile is checked; selecting
    one switches profile
  - separator
  - `Quit`
- **Tray tooltip:** `Arctis Sound Manager — 🔋<battery> · <connected|disconnected>`,
  updated live. Battery/connection omitted gracefully when unknown.
- **Quit → stop the daemon AND exit the GUI.** A clean "shut it all down now."
  The daemon's systemd autostart still brings it back next login; a clean
  `systemctl --user stop` does not trip `Restart=on-failure`.

### Out of scope (YAGNI)

- Surround on/off toggle in the tray.
- Click-the-icon-to-mute (mute is a menu item only).
- Status-colored / dynamic custom tray icons (use the app icon).
- A "minimize button → tray" affordance (only X hides; normal minimize is normal).

## Architecture

All work is in `src-tauri` (plus build-dependency additions to the two CI
workflows). The engine/daemon and Svelte frontend are unchanged except for a
small settings toggle wiring (GUI autostart) and reuse of existing IPC commands.

### Components

1. **Tray construction** (`src-tauri/src/tray.rs`, new) — builds the
   `TrayIcon` and its `Menu` in `lib.rs` `setup()` via Tauri v2's native
   `TrayIconBuilder` / `MenuBuilder`. Returns handles to the mutable items
   (the `Mute` `CheckMenuItem`, the `Profile` `Submenu`, the `TrayIcon` itself)
   which are stored in `tauri::State` so the poll task can update them.

2. **Window close interception** (`lib.rs`) — `on_window_event` handles
   `WindowEvent::CloseRequested`: `api.prevent_close()` then `window.hide()`.

3. **Live sync** — the existing 250 ms state-poll task in `lib.rs` already holds
   `EngineState` and diffs against `last_state`. Extend its on-change branch to
   update: the `Mute` check, the active-profile check in the submenu, and the
   tray tooltip. The profile *list* (submenu items) is rebuilt only when the set
   of profile names changes, not on every tick.

4. **Menu / tray event handling** (`tray.rs`) — `on_menu_event` maps item ids to
   actions, reusing existing daemon commands:
   - `show` → toggle window visibility + focus
   - `mute` → `SetMasterMute(!current)`
   - `profile:<name>` → `SwitchProfile(name)`
   - `quit` → run the daemon-stop path, then `app.exit(0)`
   - tray icon left-click (`TrayIconEvent`) → toggle window visibility

5. **GUI autostart** — add `tauri-plugin-autostart`. The registered login command
   includes a `--hidden` flag. The main window config becomes `"visible": false`;
   in `setup()` the window is shown **unless** `--hidden` is present in the launch
   args. So a manual launch shows the window; the login launch stays in the tray.
   A frontend settings toggle calls a new command that enables/disables the
   autostart entry via the plugin.

### Pure, unit-testable pieces

The Tauri wiring (tray creation, event callbacks, window show/hide) is verified
manually on the target machine, consistent with the project convention that
live-GUI/PipeWire tests are out of the CI gate. The logic that *can* be pure is
extracted and unit-tested:

- `tray_view_model(&EngineState) -> { mute_checked: bool, active_profile:
  Option<String>, profiles: Vec<String>, tooltip: String }` — the mapping from
  engine state to what the tray should display. Covers tooltip formatting
  (battery present/absent, connected/disconnected) and mute/profile derivation.
- `parse_menu_id(&str) -> MenuAction` — maps ids (`show`, `mute`, `quit`,
  `profile:<name>`) to a typed action; round-trips with the id builder.
- `should_start_hidden(args: &[String]) -> bool` — `--hidden` detection.

### Data flow

```
login ──(autostart entry: `arctis-sound-manager --hidden`)──▶ GUI starts, window hidden, tray shown
daemon (systemd) runs independently ──▶ 250ms poll ──▶ EngineState ──▶ tray_view_model ──▶ update Mute/Profile/tooltip
user clicks tray icon ─────────────▶ toggle window show/hide
user picks Mute/Profile ───────────▶ IPC to daemon (SetMasterMute / SwitchProfile)
user picks Quit ───────────────────▶ daemon stop (systemctl --user stop) ──▶ app.exit(0)
user clicks X ─────────────────────▶ prevent_close + window.hide  (process stays in tray)
```

### Error handling

- Tray construction failure is logged and non-fatal — the app still runs as a
  normal window (degraded, no tray). Never panic in `setup()`.
- Menu actions that hit the daemon use the existing best-effort IPC; failures are
  logged (and surfaced in-window as today) and never crash the tray.
- If the daemon is already stopped when Quit runs, the stop is a no-op; the GUI
  still exits.
- Tooltip/menu updates tolerate a missing/`None` `EngineState` (daemon down):
  show "disconnected", omit battery, leave the last known profile list.

## Build / packaging considerations (important)

- **Linux build dependency:** the tray needs the `tauri` `tray-icon` feature and,
  on Linux, **`libayatana-appindicator3-dev`** at build time. Add it to
  `ci.yml` and `release.yml` apt installs and to the local build docs
  (`CLAUDE.md` / `docs/PACKAGING.md`). This mirrors the `pw-watcher` /
  `libpipewire-0.3-dev` situation already handled in the workflows.
- **Runtime:** the AppImage must carry `libayatana-appindicator3` (linuxdeploy
  generally bundles it). Verify the produced AppImage shows a tray icon on the
  target KDE/Wayland session, which supports the StatusNotifier protocol.
- **Capabilities:** add the tray and `autostart` plugin permissions to the Tauri
  v2 capabilities file.
- **Icon:** reuse the bundled app icon (`icons/`) for the tray; no new asset.

## Testing

- Unit tests (Rust, in `src-tauri`): `tray_view_model`, `parse_menu_id`,
  `should_start_hidden`.
- Manual on-target verification (owner): tray icon appears on KDE/Wayland; X hides
  to tray; left-click toggles the window; Mute item reflects and toggles state;
  Profile submenu reflects/switches the active profile; tooltip shows battery +
  connection; Quit stops the daemon and exits; GUI autostart launches hidden in
  the tray on login; toggling GUI autostart off removes the login entry.

## Rollout

Ships in the next release (v0.2.5) through the existing tagged-release pipeline.
After the release, verify the tray on the freshly-installed AppImage (the
appindicator runtime bundling is the main unknown).
