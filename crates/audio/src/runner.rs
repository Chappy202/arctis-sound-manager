use crate::error::AudioError;
use std::process::Command;

/// Opaque handle to an owned child process group. The real runner stores the OS pid/pgid;
/// the mock stores a synthetic id. Killing happens through `CommandRunner::kill_owned`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildToken {
    /// Process group id (== child pid for a fresh group); 0 for mock.
    pub pgid: i32,
    /// Human/debug label, e.g. the conf path.
    pub label: String,
}

/// Captured result of a subprocess invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Runs an argv and captures its output. Mirrors the device crate's
/// `Transport` seam so the backend is testable with no live daemon (G1, G8).
pub trait CommandRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CmdOutput, AudioError>;

    /// Spawn a child in its OWN process group and return a token the caller stores.
    ///
    /// Real impl: `Command::new(program).args(args).process_group(0).spawn()`;
    /// token.pgid = child pid.
    ///
    /// The `Child` handle is dropped after recording the pid — liveness is tracked
    /// by the engine's `ChildOwner` and termination is by pgid via `kill_owned`.
    fn spawn_owned(&mut self, program: &str, args: &[&str]) -> Result<ChildToken, AudioError>;

    /// Terminate the process group named by the token: `libc::kill(-pgid, SIGTERM)`.
    ///
    /// Idempotent: a token for an already-dead group returns Ok.
    /// The mock records the call in `killed`.
    fn kill_owned(&mut self, token: &ChildToken) -> Result<(), AudioError>;
}

/// Real runner over `std::process::Command`.
#[derive(Default)]
pub struct RealRunner;

/// Upper bound for one `run()` subprocess. `run` executes while the daemon-wide
/// engine mutex is held, so an unbounded child (the old `Command::output()`)
/// could wedge the whole daemon on a single hung `pw-cli`/`pw-dump`.
const RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// How long `kill_owned` waits for a SIGTERM'd child to exit before escalating
/// to SIGKILL (which cannot be ignored, so the follow-up `wait()` is bounded).
const REAP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Children spawned via `RealRunner::spawn_owned`, kept alive so kill paths can
/// `wait(2)` on (reap) them — dropping the `Child` handle left every terminated
/// filter-chain process as a zombie until daemon exit. Process-global because
/// `RealRunner` is a stateless unit struct constructed at many call sites, and
/// all instances spawn children of this same process.
static OWNED_CHILDREN: std::sync::Mutex<Vec<std::process::Child>> =
    std::sync::Mutex::new(Vec::new());

fn owned_children() -> std::sync::MutexGuard<'static, Vec<std::process::Child>> {
    OWNED_CHILDREN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Reap tracked children that have already exited (e.g. killed via the
/// pkill-by-conf-path fallback) so zombies never accumulate across recreates.
/// `try_wait` returning `Ok(Some(_))` IS the reap.
fn reap_exited(children: &mut Vec<std::process::Child>) {
    children.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
}

/// Read a piped stream to a lossy string (empty on any failure).
fn drain_pipe<R: std::io::Read>(pipe: Option<R>) -> String {
    let mut buf = Vec::new();
    if let Some(mut p) = pipe {
        use std::io::Read as _;
        let _ = p.read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// `run` with an explicit time bound: spawn, drain stdout/stderr on helper
/// threads (a full pipe must not deadlock the child), poll `try_wait`, and on
/// timeout kill + reap the child and return a typed `AudioError::Timeout`.
fn run_with_timeout(
    program: &str,
    args: &[&str],
    timeout: std::time::Duration,
) -> Result<CmdOutput, AudioError> {
    use std::process::Stdio;
    let spawn_err = |e: &dyn std::fmt::Display| AudioError::Spawn {
        program: program.to_string(),
        source_msg: e.to_string(),
    };
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| spawn_err(&e))?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let out_thread = std::thread::spawn(move || drain_pipe(stdout_pipe));
    let err_thread = std::thread::spawn(move || drain_pipe(stderr_pipe));

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait().map_err(|e| spawn_err(&e))? {
            Some(st) => break st,
            None if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            None => {
                // Timed out: SIGKILL + reap, then surface a typed error.
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_thread.join();
                let _ = err_thread.join();
                return Err(AudioError::Timeout {
                    program: program.to_string(),
                    millis: timeout.as_millis(),
                });
            }
        }
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok(CmdOutput {
        status: status.code().unwrap_or(-1),
        stdout,
        stderr,
    })
}

impl CommandRunner for RealRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CmdOutput, AudioError> {
        run_with_timeout(program, args, RUN_TIMEOUT)
    }

    fn spawn_owned(&mut self, program: &str, args: &[&str]) -> Result<ChildToken, AudioError> {
        use std::os::unix::process::CommandExt;
        let child = Command::new(program)
            .args(args)
            // Place the child in its own process group (pgid == child pid).
            .process_group(0)
            .spawn()
            .map_err(|e| AudioError::Spawn {
                program: program.to_string(),
                source_msg: e.to_string(),
            })?;
        let pgid = child.id() as i32;
        let label = {
            let mut parts = vec![program.to_string()];
            parts.extend(args.iter().map(|a| a.to_string()));
            parts.join(" ")
        };
        // Keep the Child handle so kill paths can wait(2) on it — and sweep any
        // already-exited children so zombies never accumulate across recreates.
        {
            let mut owned = owned_children();
            reap_exited(&mut owned);
            owned.push(child);
        }
        Ok(ChildToken { pgid, label })
    }

    fn kill_owned(&mut self, token: &ChildToken) -> Result<(), AudioError> {
        // SAFETY: libc::kill is safe to call with a valid signal number.
        // ESRCH (-3) means the process group is already gone — treat as success.
        let ret = unsafe { libc::kill(-token.pgid, libc::SIGTERM) };
        if ret != 0 {
            // Read errno immediately after the failed syscall, before any other call.
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno != libc::ESRCH {
                return Err(AudioError::Spawn {
                    program: format!("kill(-{}, SIGTERM)", token.pgid),
                    source_msg: format!("errno {errno}"),
                });
            }
        }
        // Reap the direct child so it never lingers as a zombie: poll try_wait
        // for up to REAP_TIMEOUT, then escalate to SIGKILL + blocking wait
        // (bounded — SIGKILL cannot be ignored). Take it out of the registry
        // first so the lock is not held across the wait.
        let child = {
            let mut owned = owned_children();
            owned
                .iter()
                .position(|c| c.id() as i32 == token.pgid)
                .map(|i| owned.swap_remove(i))
        };
        if let Some(mut child) = child {
            let deadline = std::time::Instant::now() + REAP_TIMEOUT;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break, // exited — reaped
                    Ok(None) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Ok(None) => {
                        // Ignored SIGTERM — escalate to the whole group and reap.
                        unsafe {
                            let _ = libc::kill(-token.pgid, libc::SIGKILL);
                        }
                        if let Err(e) = child.wait() {
                            eprintln!(
                                "audio: wait() after SIGKILL failed for pgid {}: {e}",
                                token.pgid
                            );
                        }
                        break;
                    }
                    Err(e) => {
                        eprintln!("audio: try_wait failed for pgid {}: {e}", token.pgid);
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

/// In-memory runner for tests: records every argv, replays queued outputs.
/// Mirrors `MockTransport` (G1).
#[derive(Default)]
pub struct MockRunner {
    /// Each recorded call is `[program, arg0, arg1, …]` (from `run`).
    pub calls: Vec<Vec<String>>,
    queued: std::collections::VecDeque<CmdOutput>,
    /// Records every `spawn_owned` call as `[program, arg0, arg1, …]`.
    pub spawned: Vec<Vec<String>>,
    /// Records every token passed to `kill_owned`.
    pub killed: Vec<ChildToken>,
    /// If set, `kill_owned` returns an error for the token with this label.
    pub fail_kill_label: Option<String>,
}

impl MockRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue an output for the next `run`.
    pub fn with_output(mut self, status: i32, stdout: &str, stderr: &str) -> Self {
        self.queued.push_back(CmdOutput {
            status,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        });
        self
    }

    /// The most recent recorded call, as `program` plus its args.
    pub fn last_call(&self) -> Option<&Vec<String>> {
        self.calls.last()
    }
}

impl CommandRunner for MockRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CmdOutput, AudioError> {
        let mut call = Vec::with_capacity(args.len() + 1);
        call.push(program.to_string());
        call.extend(args.iter().map(|a| a.to_string()));
        self.calls.push(call);
        Ok(self.queued.pop_front().unwrap_or(CmdOutput {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
        }))
    }

    fn spawn_owned(&mut self, program: &str, args: &[&str]) -> Result<ChildToken, AudioError> {
        let mut call = Vec::with_capacity(args.len() + 1);
        call.push(program.to_string());
        call.extend(args.iter().map(|a| a.to_string()));
        let label = call.join(" ");
        self.spawned.push(call);
        Ok(ChildToken { pgid: 0, label })
    }

    fn kill_owned(&mut self, token: &ChildToken) -> Result<(), AudioError> {
        self.killed.push(token.clone());
        if self
            .fail_kill_label
            .as_deref()
            .is_some_and(|l| l == token.label)
        {
            return Err(AudioError::Spawn {
                program: format!("kill({})", token.label),
                source_msg: "simulated kill failure".to_string(),
            });
        }
        Ok(())
    }
}

/// Forward `CommandRunner` through a mutable reference so one runner can be
/// shared across N per-channel `AudioBackend`s without cloning (G1 reuse seam).
impl<R: CommandRunner + ?Sized> CommandRunner for &mut R {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CmdOutput, AudioError> {
        (**self).run(program, args)
    }
    fn spawn_owned(&mut self, program: &str, args: &[&str]) -> Result<ChildToken, AudioError> {
        (**self).spawn_owned(program, args)
    }
    fn kill_owned(&mut self, token: &ChildToken) -> Result<(), AudioError> {
        (**self).kill_owned(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_records_argv_and_replays_output() {
        let mut r = MockRunner::new().with_output(0, "node-id 42", "");
        let out = r.run("pw-cli", &["ls", "Node"]).expect("mock never errors");
        assert_eq!(out.stdout, "node-id 42");
        assert_eq!(r.last_call().unwrap(), &vec!["pw-cli", "ls", "Node"]);
    }

    #[test]
    fn mock_defaults_to_success_when_queue_empty() {
        let mut r = MockRunner::new();
        let out = r.run("true", &[]).expect("mock never errors");
        assert_eq!(out.status, 0);
    }

    #[test]
    fn mut_ref_runner_forwards_and_records() {
        let mut r = MockRunner::new().with_output(0, "ok", "");
        {
            let by_ref = &mut r;
            let out = by_ref.run("pw-cli", &["ls", "Node"]).expect("forwards");
            assert_eq!(out.stdout, "ok");
        }
        assert_eq!(r.calls[0], vec!["pw-cli", "ls", "Node"]);
    }

    #[test]
    fn mock_spawn_owned_records_and_returns_token() {
        let mut r = MockRunner::new();
        let token = r
            .spawn_owned("pipewire", &["-c", "/tmp/x.conf"])
            .expect("mock spawn_owned never errors");
        assert_eq!(token.pgid, 0, "mock always returns pgid 0");
        assert_eq!(r.spawned[0], vec!["pipewire", "-c", "/tmp/x.conf"]);
    }

    #[test]
    fn mock_kill_owned_records() {
        let mut r = MockRunner::new();
        let token = r
            .spawn_owned("pipewire", &["-c", "/tmp/x.conf"])
            .expect("spawn_owned");
        r.kill_owned(&token)
            .expect("kill_owned never errors in mock");
        assert_eq!(r.killed.len(), 1);
        assert_eq!(r.killed[0], token);
    }

    /// True when `/proc/<pid>` exists AND its state field is 'Z' (zombie).
    /// A missing /proc entry means the pid was fully reaped — not a zombie.
    fn is_zombie(pid: i32) -> bool {
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            // State is the first field after the ')' closing the comm name.
            Ok(stat) => {
                stat.rsplit(')')
                    .next()
                    .and_then(|rest| rest.trim_start().chars().next())
                    == Some('Z')
            }
            Err(_) => false,
        }
    }

    /// Zombie fix: kill_owned must kill AND wait() the tracked child — the pid
    /// must be fully reaped afterwards (no zombie left until daemon exit).
    #[test]
    fn real_runner_kill_owned_reaps_child_no_zombie() {
        let mut r = RealRunner;
        let token = r.spawn_owned("sleep", &["30"]).expect("spawn sleep");
        let pid = token.pgid;
        r.kill_owned(&token).expect("kill_owned");
        assert!(
            !is_zombie(pid),
            "pid {pid} must be fully reaped after kill_owned (no zombie)"
        );
    }

    /// Zombie fix: a child that exits on its own (or is pkill'd by the conf-path
    /// fallback) is swept + reaped by the next spawn_owned.
    #[test]
    fn real_runner_spawn_owned_sweeps_exited_children() {
        let mut r = RealRunner;
        let t1 = r.spawn_owned("true", &[]).expect("spawn true");
        // Give `true` a moment to exit (it exits immediately).
        std::thread::sleep(std::time::Duration::from_millis(200));
        // The next spawn sweeps the registry, reaping the exited child.
        let t2 = r.spawn_owned("sleep", &["30"]).expect("spawn sleep");
        assert!(
            !is_zombie(t1.pgid),
            "exited child {} must be reaped by the sweep in spawn_owned",
            t1.pgid
        );
        r.kill_owned(&t2).expect("cleanup kill");
    }

    /// Subprocess time bound: a hung `run()` must return a typed Timeout error
    /// promptly instead of blocking the (mutex-holding) caller indefinitely.
    #[test]
    fn run_with_timeout_kills_hung_subprocess() {
        let start = std::time::Instant::now();
        let err = super::run_with_timeout("sleep", &["30"], std::time::Duration::from_millis(100))
            .expect_err("hung subprocess must yield an error");
        assert!(
            matches!(err, AudioError::Timeout { .. }),
            "must be the typed Timeout variant, got: {err}"
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(3),
            "run must give up promptly"
        );
    }

    /// The bounded run still captures stdout/stderr and exit status correctly.
    #[test]
    fn run_with_timeout_captures_output() {
        let out = super::run_with_timeout("echo", &["hi"], std::time::Duration::from_secs(5))
            .expect("echo runs");
        assert_eq!(out.status, 0);
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.stderr, "");
    }

    #[test]
    fn mut_ref_runner_forwards_spawn_owned_and_kill_owned() {
        let mut r = MockRunner::new();
        let token = {
            let by_ref = &mut r;
            by_ref
                .spawn_owned("pipewire", &["-c", "/tmp/y.conf"])
                .expect("forwards spawn_owned")
        };
        r.kill_owned(&token).expect("forwards kill_owned");
        assert_eq!(r.spawned.len(), 1);
        assert_eq!(r.killed.len(), 1);
    }
}
