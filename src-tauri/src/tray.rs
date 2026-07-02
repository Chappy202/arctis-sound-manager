//! System-tray icon for the GUI: pure helpers + (later tasks) the Tauri wiring.

use crate::state::DaemonState;
use arctis_client::{send_request_to, Request};
use tauri::menu::{CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};
use tokio::sync::Mutex as AsyncMutex;

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
                crate::mark_visible(app, false);
            }
            _ => {
                let _ = win.show();
                let _ = win.set_focus();
                crate::mark_visible(app, true);
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

    let icon = app.default_window_icon().cloned().ok_or_else(|| {
        tauri::Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "bundle icon not present",
        ))
    })?;

    let tray = TrayIconBuilder::with_id("main")
        .icon(icon)
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

/// Push a view-model onto the live tray: mute check, profile submenu (rebuilt
/// only when the profile set changes), active-profile check, and tooltip.
pub fn apply_view(app: &AppHandle, view: &TrayView) {
    let Some(handles) = app.try_state::<TrayHandles>() else { return };
    let _ = handles.mute.set_checked(view.mute_checked);
    let _ = handles.tray.set_tooltip(Some(&view.tooltip));

    // Rebuild the profile submenu items only when the set of names changed.
    let mut last = handles.last_profiles.lock().unwrap_or_else(|e| e.into_inner());
    if *last != view.profiles {
        // Remove existing items; only append new items and update the cache
        // when removal succeeded, so a transient error leaves last_profiles
        // unchanged and the next tick retries the full rebuild.
        if let Ok(items) = handles.profile.items() {
            for it in items {
                let _ = handles.profile.remove(&it);
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
        }
    } else {
        // Profile set unchanged — just refresh which item is checked.
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

/// True when the process was launched with `--hidden` (autostart-into-tray).
pub fn should_start_hidden(args: &[String]) -> bool {
    args.iter().any(|a| a == "--hidden")
}

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
