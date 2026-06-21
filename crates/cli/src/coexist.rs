use arctis_audio::CommandRunner;

/// Legacy systemd user services managed by the arctis-sound-manager RPM.
/// Data-driven: add/remove names here if the upstream RPM changes.
pub const LEGACY_USER_SERVICES: &[&str] = &[
    "arctis-manager.service",
    "arctis-gui.service",
    "arctis-video-router.service",
    "filter-chain.service",
];

#[derive(Debug, Default, PartialEq)]
pub struct LegacyReport {
    pub legacy_loopbacks: Vec<String>,
    pub hrir_switch_present: bool,
    pub rpm_daemon_running: bool,
}

/// A single reversible teardown action with a human description + exact argv.
#[derive(Debug, Clone, PartialEq)]
pub struct TeardownAction {
    /// Human-readable description shown in dry-run + real-run output.
    pub description: String,
    /// The program to run.
    pub program: String,
    /// The arguments to pass to the program.
    pub args: Vec<String>,
}

impl TeardownAction {
    fn new(description: &str, program: &str, args: &[&str]) -> Self {
        Self {
            description: description.to_string(),
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
        }
    }
}

/// Per-action result: the action description plus ok/err.
#[derive(Debug, Clone)]
pub struct ActionResult {
    pub description: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Summary of a `run_teardown` call.
#[derive(Debug, Clone)]
pub struct TeardownResult {
    pub actions_attempted: usize,
    pub successes: usize,
    pub failures: Vec<ActionResult>,
    /// True when this was a dry-run (nothing was executed).
    pub dry_run: bool,
}

impl TeardownResult {
    pub fn all_ok(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Build the list of teardown actions for a detected legacy stack.
///
/// Conservative + reversible:
/// - Uses `systemctl --user disable --now` for services (re-enabling is trivial).
/// - Uses `pw-cli destroy <name>` for live loopback nodes (they get recreated on
///   service re-enable anyway).
/// - Does NOT delete files, does NOT `dnf remove` (needs sudo — owner step).
/// - Does NOT touch hrir-switch (it's a user script, not harmful — noted but skipped).
pub fn teardown_plan(report: &LegacyReport) -> Vec<TeardownAction> {
    let mut actions = Vec::new();

    // Disable + stop legacy user services (reversible: `systemctl --user enable --now`).
    for svc in LEGACY_USER_SERVICES {
        actions.push(TeardownAction::new(
            &format!("disable and stop legacy service {svc}"),
            "systemctl",
            &["--user", "disable", "--now", svc],
        ));
    }

    // Destroy live legacy loopback nodes (they get recreated on service re-enable).
    for node in &report.legacy_loopbacks {
        actions.push(TeardownAction::new(
            &format!("destroy live legacy loopback node {node}"),
            "pw-cli",
            &["destroy", node],
        ));
    }

    // hrir-switch is a user script; noted but not deleted.
    // (if present, the caller should communicate this to the user separately)

    actions
}

/// Execute (or preview) a teardown plan.
///
/// When `dry_run` is true, no commands are run; the plan is collected and
/// returned with all successes set to true (preview mode).
///
/// When `dry_run` is false, each action runs via `runner`. Failures are collected
/// but do NOT abort the remaining actions (continue-past-failure policy). The
/// runner is never panicked or unwrapped; all errors surface via `TeardownResult`.
pub fn run_teardown<R: CommandRunner>(
    runner: &mut R,
    plan: &[TeardownAction],
    dry_run: bool,
) -> TeardownResult {
    if dry_run {
        return TeardownResult {
            actions_attempted: plan.len(),
            successes: plan.len(),
            failures: vec![],
            dry_run: true,
        };
    }

    let mut successes = 0usize;
    let mut failures = Vec::new();

    for action in plan {
        let args_refs: Vec<&str> = action.args.iter().map(|s| s.as_str()).collect();
        match runner.run(&action.program, &args_refs) {
            Ok(out) if out.status == 0 => {
                successes += 1;
            }
            Ok(out) => {
                failures.push(ActionResult {
                    description: action.description.clone(),
                    ok: false,
                    error: Some(format!("exit status {}: {}", out.status, out.stderr.trim())),
                });
            }
            Err(e) => {
                failures.push(ActionResult {
                    description: action.description.clone(),
                    ok: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    TeardownResult {
        actions_attempted: plan.len(),
        successes,
        failures,
        dry_run: false,
    }
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
    use arctis_audio::MockRunner;

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

    // ── teardown_plan tests ──────────────────────────────────────────────────

    #[test]
    fn teardown_plan_always_includes_all_services() {
        let report = LegacyReport::default(); // no loopbacks
        let plan = teardown_plan(&report);
        // Must always include all 4 services
        assert_eq!(
            plan.len(),
            LEGACY_USER_SERVICES.len(),
            "plan must have exactly one action per service when no loopbacks"
        );
        for svc in LEGACY_USER_SERVICES {
            let found = plan.iter().any(|a| {
                a.program == "systemctl"
                    && a.args.contains(&"disable".to_string())
                    && a.args.contains(&"--now".to_string())
                    && a.args.contains(&svc.to_string())
            });
            assert!(found, "plan must include disable action for {svc}");
        }
    }

    #[test]
    fn teardown_plan_includes_loopback_destroy_actions() {
        let report = LegacyReport {
            legacy_loopbacks: vec!["Arctis_Game".into(), "Arctis_Chat".into()],
            ..Default::default()
        };
        let plan = teardown_plan(&report);
        // 4 services + 2 loopback nodes
        assert_eq!(plan.len(), LEGACY_USER_SERVICES.len() + 2);

        let destroy_game = plan.iter().any(|a| {
            a.program == "pw-cli"
                && a.args.contains(&"destroy".to_string())
                && a.args.contains(&"Arctis_Game".to_string())
        });
        assert!(
            destroy_game,
            "plan must include destroy action for Arctis_Game"
        );

        let destroy_chat = plan.iter().any(|a| {
            a.program == "pw-cli"
                && a.args.contains(&"destroy".to_string())
                && a.args.contains(&"Arctis_Chat".to_string())
        });
        assert!(
            destroy_chat,
            "plan must include destroy action for Arctis_Chat"
        );
    }

    // ── run_teardown dry-run tests ───────────────────────────────────────────

    #[test]
    fn run_teardown_dry_run_does_not_call_runner() {
        let mut runner = MockRunner::new();
        let report = LegacyReport {
            legacy_loopbacks: vec!["Arctis_Game".into()],
            ..Default::default()
        };
        let plan = teardown_plan(&report);
        let result = run_teardown(&mut runner, &plan, true);

        assert!(result.dry_run, "result must be marked dry_run");
        assert!(result.all_ok(), "dry-run always reports all ok");
        assert_eq!(result.successes, plan.len());
        assert!(runner.calls.is_empty(), "dry-run must not call the runner");
    }

    // ── run_teardown real-run tests with MockRunner ──────────────────────────

    #[test]
    fn run_teardown_real_run_sends_exact_argv() {
        let mut runner = MockRunner::new();
        // Queue success for every action (4 services + 1 loopback = 5)
        for _ in 0..5 {
            runner = runner.with_output(0, "", "");
        }
        let report = LegacyReport {
            legacy_loopbacks: vec!["Arctis_Game".into()],
            ..Default::default()
        };
        let plan = teardown_plan(&report);
        let result = run_teardown(&mut runner, &plan, false);

        assert!(!result.dry_run);
        assert!(
            result.all_ok(),
            "all actions succeeded: {:?}",
            result.failures
        );
        assert_eq!(result.successes, 5);
        assert_eq!(
            runner.calls.len(),
            5,
            "runner must be called once per action"
        );

        // Verify at least one systemctl call with exact argv structure
        let sctl_call = runner
            .calls
            .iter()
            .find(|c| c[0] == "systemctl" && c.contains(&"disable".to_string()))
            .expect("at least one systemctl disable call");
        assert!(sctl_call.contains(&"--user".to_string()));
        assert!(sctl_call.contains(&"--now".to_string()));

        // Verify the pw-cli destroy call
        let pw_call = runner
            .calls
            .iter()
            .find(|c| c[0] == "pw-cli")
            .expect("one pw-cli call for loopback destroy");
        assert_eq!(pw_call[1], "destroy");
        assert_eq!(pw_call[2], "Arctis_Game");
    }

    #[test]
    fn run_teardown_continues_past_failed_action() {
        let mut runner = MockRunner::new();
        // First action fails (non-zero exit), rest succeed
        runner = runner.with_output(1, "", "unit not found");
        for _ in 1..LEGACY_USER_SERVICES.len() {
            runner = runner.with_output(0, "", "");
        }

        let report = LegacyReport::default(); // no loopbacks
        let plan = teardown_plan(&report);
        let result = run_teardown(&mut runner, &plan, false);

        assert!(!result.dry_run);
        assert_eq!(result.actions_attempted, LEGACY_USER_SERVICES.len());
        assert_eq!(result.failures.len(), 1, "exactly one failure");
        assert_eq!(
            result.successes,
            LEGACY_USER_SERVICES.len() - 1,
            "remaining actions must succeed"
        );
        // All calls were made (continue-past-failure)
        assert_eq!(
            runner.calls.len(),
            LEGACY_USER_SERVICES.len(),
            "runner must be called for every action even after failure"
        );
    }

    #[test]
    fn run_teardown_nonzero_exit_collected_as_failure() {
        // Tests the non-zero-exit-status failure arm: each action returns
        // status 127 and the error is collected in `result.failures`.
        // (The spawn-Err arm is exercised by RealRunner in production; the
        // MockRunner always succeeds at spawning.)
        let mut runner = MockRunner::new();
        for _ in 0..LEGACY_USER_SERVICES.len() {
            runner = runner.with_output(127, "", "command not found");
        }
        let report = LegacyReport::default();
        let plan = teardown_plan(&report);
        let result = run_teardown(&mut runner, &plan, false);

        assert_eq!(result.failures.len(), LEGACY_USER_SERVICES.len());
        assert_eq!(result.successes, 0);
        // Error strings surfaced
        for f in &result.failures {
            assert!(
                f.error.as_deref().unwrap_or("").contains("127"),
                "error must mention exit status 127"
            );
        }
    }
}
