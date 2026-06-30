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
