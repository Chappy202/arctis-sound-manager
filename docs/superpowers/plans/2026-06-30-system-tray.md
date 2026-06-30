# System Tray for the GUI — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the Tauri GUI a persistent system-tray icon — close-to-tray, autostart-hidden-on-login, a tray menu (Show/Hide · Mute · Profile ▸ · Quit), a live tooltip with battery + connection, and a Quit that stops the daemon and exits.

**Architecture:** All work is in `src-tauri` plus two CI workflow edits and one small frontend toggle. A new `tray.rs` holds the tray build, event handlers, and pure helpers. `lib.rs` wires it into `setup()`, intercepts window-close to hide, and extends the existing 250 ms state-poll to keep the tray in sync. The audio daemon (`asm-cli`, a separate systemd service) is untouched; the tray reuses existing IPC requests.

**Tech Stack:** Rust, Tauri v2 (native `tray-icon` + `menu` APIs), `tauri-plugin-autostart`, Svelte 5 frontend, `arctis-client` IPC.

## Global Constraints

- Tauri v2; `tauri` crate gains features `["tray-icon"]`. Tray icon reuses `app.default_window_icon()` (no `image-png` feature, no new asset).
- Linux build dep: **`libayatana-appindicator3-dev`** required to compile the tray; add to `ci.yml` + `release.yml` apt installs (mirrors the existing `libpipewire-0.3-dev` handling).
- IPC goes through `arctis_client::send_request_to(&socket, &Request::…)` on a blocking thread; the socket comes from `state::<Mutex<DaemonState>>().lock().await.socket.clone()` (see `commands.rs::call`).
- Engine state fields used (from `crates/engine/src/state.rs`): `master_mute: bool`, `active_profile: String`, `profiles: Vec<String>`, `device_present: bool`, `device_fields: BTreeMap<String,String>` (keys `"battery"`, `"model"`).
- Quit calls the same stop path as the `daemon_stop` command (`daemon_control::stop`), then `app.exit(0)`. It does NOT disable the daemon's systemd autostart.
- Tests run single-threaded in CI (`cargo test --workspace -- --test-threads=1`); rustfmt is advisory; clippy `-D warnings` is a gate (including `--features pw-watcher` for the cli crate — unrelated here but don't break it).
- Tray/window wiring is verified manually on the target (KDE/Wayland), per the project convention that live-GUI tests are out of the CI gate. Only the pure helpers are unit-tested.

---

### Task 1: Pure tray helpers (view-model, menu-id parsing, hidden-arg)

No Tauri dependencies — fully unit-testable. These are consumed by the wiring tasks.

**Files:**
- Create: `src-tauri/src/tray.rs`
- Modify: `src-tauri/src/lib.rs:1-5` (add `mod tray;`)

**Interfaces:**
- Produces:
  - `pub struct TrayView { pub mute_checked: bool, pub active_profile: String, pub profiles: Vec<String>, pub tooltip: String }`
  - `pub fn tray_view(state: &arctis_engine::EngineState) -> TrayView`
  - `pub fn tooltip_string(device_present: bool, battery: Option<&str>, model: Option<&str>) -> String`
  - `pub enum MenuAction { Show, Mute, Quit, SwitchProfile(String), Unknown }`
  - `pub fn profile_item_id(name: &str) -> String` (returns `"profile:<name>"`)
  - `pub fn parse_menu_id(id: &str) -> MenuAction`
  - `pub fn should_start_hidden(args: &[String]) -> bool` (true iff any arg == `"--hidden"`)

- [ ] **Step 1: Write the failing tests**

Append to `src-tauri/src/tray.rs`:

`EngineState` does not derive `Default` and has several required struct fields, so
the tests exercise the primitive-input helpers directly (these carry the real
logic). `tray_view` is a thin field-copy whose only non-trivial part —
`tooltip_string` — is tested here; it's covered indirectly.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_shows_battery_and_connected_when_present() {
        let t = tooltip_string(true, Some("80%"), Some("Arctis Nova Pro Wireless"));
        assert!(t.contains("Arctis Nova Pro Wireless"), "got: {t}");
        assert!(t.contains("80%"), "got: {t}");
        assert!(t.contains("connected"), "got: {t}");
    }

    #[test]
    fn tooltip_disconnected_omits_battery() {
        let t = tooltip_string(false, None, None);
        assert!(t.contains("disconnected"), "got: {t}");
        assert!(!t.contains('%'), "no battery when disconnected: {t}");
    }

    #[test]
    fn menu_id_round_trips_for_profiles() {
        let id = profile_item_id("My Profile");
        assert_eq!(parse_menu_id(&id), MenuAction::SwitchProfile("My Profile".to_string()));
    }

    #[test]
    fn menu_id_parses_fixed_actions_and_unknown() {
        assert_eq!(parse_menu_id("show"), MenuAction::Show);
        assert_eq!(parse_menu_id("mute"), MenuAction::Mute);
        assert_eq!(parse_menu_id("quit"), MenuAction::Quit);
        assert_eq!(parse_menu_id("bogus"), MenuAction::Unknown);
    }

    #[test]
    fn hidden_arg_detected() {
        assert!(should_start_hidden(&["app".into(), "--hidden".into()]));
        assert!(!should_start_hidden(&["app".into()]));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p arctis-sound-manager-ui tray:: 2>&1 | tail -20`
Expected: FAIL — `tray_view`, `tooltip_string`, `parse_menu_id`, etc. not found / `tray` module empty.

- [ ] **Step 3: Write the implementation**

Put this ABOVE the `#[cfg(test)]` block in `src-tauri/src/tray.rs`:

```rust
//! System-tray icon for the GUI: pure helpers + (later tasks) the Tauri wiring.

/// What the tray should currently display, derived from engine state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayView {
    pub mute_checked: bool,
    pub active_profile: String,
    pub profiles: Vec<String>,
    pub tooltip: String,
}

/// Build the tray tooltip line. Battery is shown only when connected and known.
pub fn tooltip_string(device_present: bool, battery: Option<&str>, model: Option<&str>) -> String {
    let name = model.unwrap_or("Arctis Sound Manager");
    if device_present {
        match battery {
            Some(b) if !b.is_empty() => format!("{name} — 🔋{b} · connected"),
            _ => format!("{name} — connected"),
        }
    } else {
        format!("{name} — disconnected")
    }
}

pub fn tray_view(state: &arctis_engine::EngineState) -> TrayView {
    let battery = state.device_fields.get("battery").map(String::as_str);
    let model = state.device_fields.get("model").map(String::as_str);
    TrayView {
        mute_checked: state.master_mute,
        active_profile: state.active_profile.clone(),
        profiles: state.profiles.clone(),
        tooltip: tooltip_string(state.device_present, battery, model),
    }
}

/// A tray menu click, decoded from its item id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    Show,
    Mute,
    Quit,
    SwitchProfile(String),
    Unknown,
}

const PROFILE_PREFIX: &str = "profile:";

/// Stable menu id for a profile submenu item.
pub fn profile_item_id(name: &str) -> String {
    format!("{PROFILE_PREFIX}{name}")
}

pub fn parse_menu_id(id: &str) -> MenuAction {
    match id {
        "show" => MenuAction::Show,
        "mute" => MenuAction::Mute,
        "quit" => MenuAction::Quit,
        other => match other.strip_prefix(PROFILE_PREFIX) {
            Some(name) => MenuAction::SwitchProfile(name.to_string()),
            None => MenuAction::Unknown,
        },
    }
}

/// True when the process was launched with `--hidden` (autostart-into-tray).
pub fn should_start_hidden(args: &[String]) -> bool {
    args.iter().any(|a| a == "--hidden")
}
```

Add to the top of `src-tauri/src/lib.rs` with the other `mod` lines:

```rust
mod tray;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p arctis-sound-manager-ui tray:: 2>&1 | tail -20`
Expected: PASS (5 tests). Note: `arctis-sound-manager-ui` must already build; the sidecar must be staged (`./scripts/stage-sidecar.sh`) if this is a fresh checkout.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tray.rs src-tauri/src/lib.rs
git commit -m "feat(tray): pure helpers — view-model, menu-id parsing, hidden-arg"
```

---

### Task 2: Enable the tray-icon feature + appindicator build dependency

Makes the `tauri` tray API available and ensures CI/release can compile it.

**Files:**
- Modify: `src-tauri/Cargo.toml` (the `tauri = { version = "2", features = [] }` line)
- Modify: `.github/workflows/ci.yml` (apt install list)
- Modify: `.github/workflows/release.yml` (apt install list)
- Modify: `CLAUDE.md` (build-deps note)

**Interfaces:**
- Produces: the `tauri::tray` and `tauri::menu` modules become available to later tasks.

- [ ] **Step 1: Install the build dep locally**

Run: `sudo dnf install -y libayatana-appindicator-gtk3-devel || sudo apt-get install -y libayatana-appindicator3-dev`
Expected: installed (Nobara/Fedora uses the `-gtk3-devel` name; Ubuntu uses `-dev`).

- [ ] **Step 2: Enable the tray-icon feature**

In `src-tauri/Cargo.toml`, change:

```toml
tauri = { version = "2", features = [] }
```
to:
```toml
tauri = { version = "2", features = ["tray-icon"] }
```

- [ ] **Step 3: Add the dep to CI**

In `.github/workflows/ci.yml`, add `libayatana-appindicator3-dev \` to the `apt-get install -y` list (after `libpipewire-0.3-dev`).

In `.github/workflows/release.yml`, add the same line to its `apt-get install -y` list.

- [ ] **Step 4: Document the dep**

In `CLAUDE.md`, in the Stack/build-deps bullet, append: "The system tray additionally needs `libayatana-appindicator3-dev` (`libayatana-appindicator-gtk3-devel` on Fedora/Nobara)."

- [ ] **Step 5: Verify it compiles**

Run: `./scripts/stage-sidecar.sh && cargo build -p arctis-sound-manager-ui 2>&1 | tail -5`
Expected: `Finished` (compiles with the tray-icon feature enabled).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/Cargo.toml .github/workflows/ci.yml .github/workflows/release.yml CLAUDE.md
git commit -m "build(tray): enable tauri tray-icon feature + libayatana-appindicator build dep"
```

---

### Task 3: Build the tray, close-to-tray, left-click toggle

Produces a visible tray icon with a static menu, hides the window on X, and toggles the window on left-click. Menu *actions* are wired in Task 4.

**Files:**
- Modify: `src-tauri/src/tray.rs` (add the builder + a window-toggle helper)
- Modify: `src-tauri/src/lib.rs` (call the builder in `setup()`, add `on_window_event`)
- Modify: `src-tauri/tauri.conf.json` (window `visible: false`)

**Interfaces:**
- Consumes: `profile_item_id` (Task 1).
- Produces:
  - `pub struct TrayHandles { pub tray: tauri::tray::TrayIcon, pub mute: tauri::menu::CheckMenuItem<tauri::Wry>, pub profile: tauri::menu::Submenu<tauri::Wry>, pub last_profiles: std::sync::Mutex<Vec<String>> }`
  - `pub fn build_tray(app: &tauri::AppHandle) -> tauri::Result<TrayHandles>`
  - `pub fn toggle_main_window(app: &tauri::AppHandle)`

- [ ] **Step 1: Add the tray builder + window toggle**

Add to `src-tauri/src/tray.rs` (above the test module). The menu starts with an empty Profile submenu (populated by Task 5's sync) and an unchecked Mute:

```rust
use tauri::menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

/// Handles kept in managed state so the poll task can update the tray live.
pub struct TrayHandles {
    pub tray: tauri::tray::TrayIcon,
    pub mute: tauri::menu::CheckMenuItem<Wry>,
    pub profile: tauri::menu::Submenu<Wry>,
    pub last_profiles: std::sync::Mutex<Vec<String>>,
}

/// Show the main window if hidden (and focus it), or hide it if visible.
pub fn toggle_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        match win.is_visible() {
            Ok(true) => {
                let _ = win.hide();
            }
            _ => {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }
    }
}

/// Build the tray icon + menu. Menu actions are attached by `attach_menu_handlers`
/// (Task 4); this task wires only the left-click toggle.
pub fn build_tray(app: &AppHandle) -> tauri::Result<TrayHandles> {
    let show = MenuItemBuilder::with_id("show", "Show/Hide Window").build(app)?;
    let mute = CheckMenuItemBuilder::with_id("mute", "Mute")
        .checked(false)
        .build(app)?;
    let profile = SubmenuBuilder::with_id(app, "profile_menu", "Profile").build()?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&show)
        .separator()
        .item(&mute)
        .item(&profile)
        .separator()
        .item(&quit)
        .build()?;

    let tray = TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().expect("bundle icon present").clone())
        .tooltip("Arctis Sound Manager")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(TrayHandles {
        tray,
        mute,
        profile,
        last_profiles: std::sync::Mutex::new(Vec::new()),
    })
}
```

- [ ] **Step 2: Wire it into setup() + close-to-tray**

In `src-tauri/src/lib.rs`, inside `.setup(|app| { … })`, BEFORE the `Ok(())`, add:

```rust
            // ── System tray ────────────────────────────────────────────────
            // Non-fatal: if the tray fails to build (e.g. no appindicator at
            // runtime) the app still runs as a normal window.
            match tray::build_tray(&app.handle().clone()) {
                Ok(handles) => {
                    app.manage(handles);
                }
                Err(e) => eprintln!("tray: build failed (continuing without tray): {e}"),
            }
```

Add an `on_window_event` to the builder chain in `run()` (right after `.setup(...)` closes, before `.run(...)`):

```rust
        .on_window_event(|window, event| {
            // X hides to tray instead of quitting; the process stays resident.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
```

- [ ] **Step 3: Make the window start hidden**

In `src-tauri/tauri.conf.json`, in the `app.windows[0]` object, add `"visible": false,` (Task 6 shows it on launch unless `--hidden`). To keep this task's manual check simple, ALSO add a temporary show in setup for now — Task 6 replaces it:

In `lib.rs` setup, after the tray `match`, add:

```rust
            // Temporary (Task 6 replaces with --hidden-aware logic): always show.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
            }
```

- [ ] **Step 4: Build and manually verify**

Run: `./scripts/stage-sidecar.sh && cargo build -p arctis-sound-manager-ui 2>&1 | tail -3`
Expected: `Finished`.
Then run the GUI (`pnpm gui` from repo root, daemon running) and confirm: a tray icon appears; clicking X hides the window (process stays — tray icon remains); left-clicking the tray icon shows it again.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tray.rs src-tauri/src/lib.rs src-tauri/tauri.conf.json
git commit -m "feat(tray): build tray icon, close-to-tray, left-click toggle"
```

---

### Task 4: Tray menu actions (Show, Mute, Profile, Quit)

Wires the menu item clicks to behavior. Mute and Profile reach the daemon; Quit stops the daemon and exits.

**Files:**
- Modify: `src-tauri/src/tray.rs` (add `attach_menu_handlers`)
- Modify: `src-tauri/src/lib.rs` (call it after `build_tray`)

**Interfaces:**
- Consumes: `parse_menu_id`, `MenuAction`, `toggle_main_window` (Tasks 1, 3); `arctis_client::{send_request_to, Request}`; `DaemonState` socket; `daemon_control::{RealEnv, stop, home_dir}`.
- Produces: `pub fn attach_menu_handlers(tray: &tauri::tray::TrayIcon)`

- [ ] **Step 1: Implement the menu handler**

Add to `src-tauri/src/tray.rs`:

```rust
use crate::state::DaemonState;
use arctis_client::{send_request_to, Request};
use tokio::sync::Mutex as AsyncMutex;

/// Attach the menu-click handler. Reuses existing daemon IPC + the daemon-stop
/// path (same as the `daemon_stop` command) for Quit.
pub fn attach_menu_handlers(tray: &tauri::tray::TrayIcon) {
    tray.on_menu_event(|app, event| {
        let app = app.clone();
        match parse_menu_id(event.id().as_ref()) {
            MenuAction::Show => toggle_main_window(&app),
            MenuAction::Mute => {
                tauri::async_runtime::spawn(async move {
                    let socket = {
                        let st = app.state::<AsyncMutex<DaemonState>>();
                        let g = st.lock().await;
                        g.socket.clone()
                    };
                    // Read current mute, then send the inverse.
                    let _ = tauri::async_runtime::spawn_blocking(move || {
                        if let Ok(resp) = send_request_to(&socket, &Request::GetState) {
                            if let Some(s) = resp.state {
                                let _ = send_request_to(
                                    &socket,
                                    &Request::SetMasterMute { muted: !s.master_mute },
                                );
                            }
                        }
                    })
                    .await;
                });
            }
            MenuAction::SwitchProfile(name) => {
                tauri::async_runtime::spawn(async move {
                    let socket = {
                        let st = app.state::<AsyncMutex<DaemonState>>();
                        let g = st.lock().await;
                        g.socket.clone()
                    };
                    let _ = tauri::async_runtime::spawn_blocking(move || {
                        send_request_to(&socket, &Request::SwitchProfile { name })
                    })
                    .await;
                });
            }
            MenuAction::Quit => {
                // Stop the daemon (same path as the daemon_stop command), then exit.
                tauri::async_runtime::spawn(async move {
                    let _ = tauri::async_runtime::spawn_blocking(|| {
                        let env = crate::daemon_control::RealEnv;
                        let socket = arctis_client::socket_path();
                        let home = crate::daemon_control::home_dir();
                        let _ = crate::daemon_control::stop(&env, &socket, &home);
                    })
                    .await;
                    app.exit(0);
                });
            }
            MenuAction::Unknown => {}
        }
    });
}
```

- [ ] **Step 2: Call it from setup()**

In `src-tauri/src/lib.rs`, in the tray `match Ok(handles)` arm, BEFORE `app.manage(handles);`, add:

```rust
                    tray::attach_menu_handlers(&handles.tray);
```

- [ ] **Step 3: Build + clippy**

Run: `cargo build -p arctis-sound-manager-ui 2>&1 | tail -3 && cargo clippy -p arctis-sound-manager-ui --all-targets -- -D warnings 2>&1 | tail -3`
Expected: `Finished` and no clippy errors.

- [ ] **Step 4: Manually verify**

Run the GUI (daemon running). Right-click the tray: `Mute` toggles master mute (confirm in the mixer); `Profile ▸` is present (empty until Task 5); `Quit` exits the GUI and stops the daemon (`systemctl --user is-active arctis-sound-manager.service` → `inactive`/`failed`).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tray.rs src-tauri/src/lib.rs
git commit -m "feat(tray): menu actions — show, mute, switch profile, quit-stops-daemon"
```

---

### Task 5: Live sync — mute check, profile submenu, tooltip

Keeps the tray reflecting the daemon: the existing 250 ms poll updates the Mute check, rebuilds the Profile submenu when the profile set changes, marks the active profile checked, and updates the tooltip.

**Files:**
- Modify: `src-tauri/src/tray.rs` (add `apply_view`)
- Modify: `src-tauri/src/lib.rs` (call `apply_view` from the state-poll on-change branch)

**Interfaces:**
- Consumes: `TrayHandles`, `tray_view`, `TrayView`, `profile_item_id` (Tasks 1, 3).
- Produces: `pub fn apply_view(app: &tauri::AppHandle, view: &TrayView)`

- [ ] **Step 1: Implement apply_view**

Add to `src-tauri/src/tray.rs`:

```rust
/// Push a view-model onto the live tray: mute check, profile submenu (rebuilt
/// only when the profile set changes), active-profile check, and tooltip.
pub fn apply_view(app: &AppHandle, view: &TrayView) {
    let Some(handles) = app.try_state::<TrayHandles>() else { return };
    let _ = handles.mute.set_checked(view.mute_checked);
    let _ = handles.tray.set_tooltip(Some(&view.tooltip));

    // Rebuild the profile submenu items only when the set of names changed.
    let mut last = handles.last_profiles.lock().unwrap();
    if *last != view.profiles {
        // Remove existing items, then append a CheckMenuItem per profile.
        if let Ok(items) = handles.profile.items() {
            for it in items {
                let _ = handles.profile.remove(&it);
            }
        }
        for name in &view.profiles {
            if let Ok(item) = CheckMenuItemBuilder::with_id(profile_item_id(name), name)
                .checked(*name == view.active_profile)
                .build(app)
            {
                let _ = handles.profile.append(&item);
            }
        }
        *last = view.profiles.clone();
    } else {
        // Set unchanged → just refresh which one is checked (reuse parse_menu_id).
        if let Ok(items) = handles.profile.items() {
            for it in items {
                if let Some(check) = it.as_check_menuitem() {
                    if let MenuAction::SwitchProfile(name) = parse_menu_id(check.id().as_ref()) {
                        let _ = check.set_checked(name == view.active_profile);
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 2: Call apply_view from the poll task**

In `src-tauri/src/lib.rs`, in the 250 ms state-poll task, inside the `if last_state.as_ref() != Some(&engine_state) {` block, AFTER the existing `let _ = handle.emit("state-changed", &engine_state);`, add:

```rust
                                    tray::apply_view(&handle, &tray::tray_view(&engine_state));
```

- [ ] **Step 3: Build + clippy**

Run: `cargo build -p arctis-sound-manager-ui 2>&1 | tail -3 && cargo clippy -p arctis-sound-manager-ui --all-targets -- -D warnings 2>&1 | tail -3`
Expected: `Finished`, no clippy errors.

- [ ] **Step 4: Manually verify**

Run the GUI + daemon. Confirm: the Profile submenu lists your profiles with the active one checked; switching profile (in-window or via tray) updates the check within ~250 ms; toggling mute updates the Mute check; the tray tooltip shows model + battery + "connected" (or "disconnected" with the headset off).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/tray.rs src-tauri/src/lib.rs
git commit -m "feat(tray): live sync of mute check, profile submenu, and tooltip"
```

---

### Task 6: GUI autostart-hidden-on-login + settings toggle

Adds the GUI's own login entry (separate from the daemon's systemd autostart), launching with `--hidden` so it starts in the tray; plus a UI toggle.

**Files:**
- Modify: `src-tauri/Cargo.toml` (add `tauri-plugin-autostart = "2"`)
- Modify: `src-tauri/src/lib.rs` (register plugin; replace the temporary show with `--hidden`-aware logic; register two commands)
- Modify: `src-tauri/src/commands.rs` (add `gui_set_autostart`, `gui_autostart_enabled`)
- Modify: `src-tauri/capabilities/default.json` (add `autostart:default`)
- Modify: `frontend/src/lib/ipc.ts` (add wrappers)
- Modify: `frontend/src/lib/components/DaemonSection.svelte` (add the toggle, mirroring the daemon-autostart switch)

**Interfaces:**
- Consumes: `should_start_hidden` (Task 1); `tauri_plugin_autostart::ManagerExt`.
- Produces: Tauri commands `gui_set_autostart(enabled: bool) -> Result<bool, String>` and `gui_autostart_enabled() -> Result<bool, String>`; frontend `guiSetAutostart`, `guiAutostartEnabled`.

- [ ] **Step 1: Add the plugin dependency**

In `src-tauri/Cargo.toml` add under the other `tauri-plugin-*` deps:

```toml
tauri-plugin-autostart = "2"
```

- [ ] **Step 2: Register the plugin + hidden-aware show**

In `src-tauri/src/lib.rs`, add the plugin to the builder chain (after the updater plugin):

```rust
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
```

Replace the Task 3 temporary show block with `--hidden`-aware logic:

```rust
            // Show the window unless launched hidden (autostart-into-tray).
            let argv: Vec<String> = std::env::args().collect();
            if !tray::should_start_hidden(&argv) {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.set_focus();
                }
            }
```

- [ ] **Step 3: Add the commands**

In `src-tauri/src/commands.rs` add (uses the autostart manager extension trait):

```rust
use tauri_plugin_autostart::ManagerExt;

/// Enable/disable the GUI's own login autostart (launches hidden into the tray).
/// Distinct from `daemon_set_autostart`, which manages the engine's systemd unit.
#[tauri::command]
pub async fn gui_set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let mgr = app.autolaunch();
    if enabled {
        mgr.enable().map_err(|e| e.to_string())?;
    } else {
        mgr.disable().map_err(|e| e.to_string())?;
    }
    mgr.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn gui_autostart_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}
```

Register both in `lib.rs`'s `invoke_handler![…]` list (next to `daemon_set_autostart`):

```rust
            commands::gui_set_autostart,
            commands::gui_autostart_enabled,
```

- [ ] **Step 4: Add the capability permission**

In `src-tauri/capabilities/default.json`, add `"autostart:default"` to the `"permissions"` array.

- [ ] **Step 5: Frontend IPC wrappers**

In `frontend/src/lib/ipc.ts` add (near `daemonSetAutostart`):

```ts
/** Enable/disable the GUI's own login autostart (launches hidden into the tray). */
export const guiSetAutostart = (enabled: boolean): Promise<boolean> =>
  invoke<boolean>("gui_set_autostart", { enabled });

export const guiAutostartEnabled = (): Promise<boolean> =>
  invoke<boolean>("gui_autostart_enabled");
```

- [ ] **Step 6: Frontend toggle**

In `frontend/src/lib/components/DaemonSection.svelte`, mirror the existing daemon-autostart switch. Add to the `<script>` imports: `guiSetAutostart, guiAutostartEnabled` from `../ipc.js`. Add state + load + handler:

```svelte
  let guiAutostart = $state(false);
  $effect(() => {
    guiAutostartEnabled().then((v) => (guiAutostart = v)).catch(() => {});
  });
  async function onToggleGuiAutostart(enabled: boolean) {
    try {
      guiAutostart = await guiSetAutostart(enabled);
    } catch (e) {
      console.error("gui autostart toggle failed", e);
    }
  }
```

In the markup, after the existing daemon "Autostart switch" block, add a second row:

```svelte
      <div class="autostart-row">
        <div class="autostart-label-group">
          <span>Launch app at login (hidden in tray)</span>
        </div>
        <Switch checked={guiAutostart} onCheckedChange={onToggleGuiAutostart} />
      </div>
```

(Match the existing row's wrapper classes/structure in this file; reuse the same `.autostart-row`/`.autostart-label-group` classes already defined there.)

- [ ] **Step 7: Build, type-check, manually verify**

Run: `cargo build -p arctis-sound-manager-ui 2>&1 | tail -3 && (cd frontend && pnpm check 2>&1 | tail -4)`
Expected: `Finished`; svelte-check 0 errors.
Manual: toggle "Launch app at login" on → confirm `~/.config/autostart/` gains an entry whose `Exec` includes `--hidden`; log out/in (or run the binary with `--hidden`) → GUI starts with no window, tray present; toggle off → entry removed.

- [ ] **Step 8: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/lib.rs src-tauri/src/commands.rs src-tauri/capabilities/default.json frontend/src/lib/ipc.ts frontend/src/lib/components/DaemonSection.svelte
git commit -m "feat(tray): GUI autostart hidden-in-tray on login + settings toggle"
```

---

### Task 7: Full verification, version bump, release

**Files:**
- Modify: `src-tauri/tauri.conf.json` (version bump)

**Interfaces:** none.

- [ ] **Step 1: Full local gate**

Run:
```bash
./scripts/stage-sidecar.sh
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p arctis-cli --features pw-watcher --all-targets -- -D warnings
cargo test --workspace -- --test-threads=1
(cd frontend && pnpm check && pnpm test)
```
Expected: all green.

- [ ] **Step 2: Bump version**

In `src-tauri/tauri.conf.json` change `"version"` from `0.2.4` to `0.2.5`.

- [ ] **Step 3: Commit + push + watch CI**

```bash
git add src-tauri/tauri.conf.json
git commit -m "chore(release): bump version to 0.2.5 (system tray)"
git push origin master
```
Then watch CI green (`gh run watch <id> --exit-status`). The CI tray build now needs the appindicator dep added in Task 2 — confirm the run is green.

- [ ] **Step 4: Tag + release (owner-confirmed)**

```bash
git tag -a v0.2.5 -m "Arctis Sound Manager v0.2.5" && git push origin v0.2.5
```
Watch the Release workflow. **Verify the produced AppImage actually shows a tray icon** on the target KDE/Wayland session (the appindicator runtime bundling is the main unknown — if the icon is missing, the AppImage needs the lib bundled, e.g. via linuxdeploy plugin or a bundled `.so`).

- [ ] **Step 5: Update local install**

Download the v0.2.5 AppImage, run `scripts/install-appimage.sh`, and confirm the tray behaviors end-to-end on the real install.

---

## Notes for the implementer

- The daemon (`asm-cli`) is a separate systemd service; never block on it. All IPC is best-effort on a blocking thread, exactly like `commands.rs::call`.
- Quit stops the running daemon but leaves its systemd autostart enabled, so the engine returns next login — this is intended.
- If `handles.profile.items()` / `remove` / `append` signatures differ slightly in the pinned Tauri version, keep the same approach (rebuild items on profile-set change); the ids must stay `profile:<name>` so `parse_menu_id` matches.
- Tray visuals can't be unit-tested here; the pure helpers (Task 1) carry the test coverage, and the manual steps cover the wiring.
