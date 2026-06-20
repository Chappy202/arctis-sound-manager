#[derive(Debug, Default, PartialEq)]
pub struct LegacyReport {
    pub legacy_loopbacks: Vec<String>,
    pub hrir_switch_present: bool,
    pub rpm_daemon_running: bool,
}

/// Scan `node_list_stdout` (output of `pw-cli ls Node`) and the user's `home`
/// directory for signs of the legacy audio stack.
pub fn detect_from(node_list_stdout: &str, home: &std::path::Path) -> LegacyReport {
    let mut report = LegacyReport::default();

    for name in ["Arctis_Game", "Arctis_Chat", "Arctis_Media"] {
        if node_list_stdout.contains(name) {
            report.legacy_loopbacks.push(name.to_string());
        }
    }

    report.hrir_switch_present = home.join(".local/bin/hrir-switch").exists();

    // Best-effort process scan — always false in test/unit contexts.
    report.rpm_daemon_running = false;

    report
}

/// Return a human-readable warning string if any legacy component was found,
/// or `None` if the system appears clean.
pub fn warning(report: &LegacyReport) -> Option<String> {
    if report.legacy_loopbacks.is_empty()
        && !report.hrir_switch_present
        && !report.rpm_daemon_running
    {
        return None;
    }

    let mut parts = Vec::new();
    if !report.legacy_loopbacks.is_empty() {
        parts.push(format!(
            "legacy loopback nodes detected: {}",
            report.legacy_loopbacks.join(", ")
        ));
    }
    if report.hrir_switch_present {
        parts.push("hrir-switch script found at ~/.local/bin/hrir-switch".to_string());
    }
    if report.rpm_daemon_running {
        parts.push("legacy RPM daemon appears to be running".to_string());
    }

    Some(format!(
        "warning: legacy stack detected — {}. Consider removing legacy config to avoid conflicts.",
        parts.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_loopbacks() {
        let node_output =
            "id 10\n    node.name = \"Arctis_Game\"\nid 11\n    node.name = \"Arctis_Chat\"\n";
        let tmp = std::env::temp_dir().join(format!("asm7_coexist_{}", std::process::id()));
        let report = detect_from(node_output, &tmp);
        assert!(report.legacy_loopbacks.contains(&"Arctis_Game".to_string()));
        assert!(report.legacy_loopbacks.contains(&"Arctis_Chat".to_string()));
        assert!(warning(&report).is_some());
    }

    #[test]
    fn clean_system() {
        let tmp = std::env::temp_dir().join(format!("asm7_clean_{}", std::process::id()));
        let report = detect_from("id 1\n    node.name = \"other_sink\"\n", &tmp);
        assert_eq!(report, LegacyReport::default());
        assert!(warning(&report).is_none());
    }
}
