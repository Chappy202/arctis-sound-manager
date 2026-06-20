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

    /// Spawn a long-lived child WITHOUT waiting for it to exit; the child is
    /// detached/orphaned for v1 — full child ownership is a later (engine) concern.
    fn spawn_detached(&mut self, program: &str, args: &[&str]) -> Result<(), AudioError>;

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

impl CommandRunner for RealRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<CmdOutput, AudioError> {
        let out = Command::new(program)
            .args(args)
            .output()
            .map_err(|e| AudioError::Spawn {
                program: program.to_string(),
                source_msg: e.to_string(),
            })?;
        Ok(CmdOutput {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn spawn_detached(&mut self, program: &str, args: &[&str]) -> Result<(), AudioError> {
        // Spawn without waiting; dropping the Child handle detaches it.
        let _child = Command::new(program)
            .args(args)
            .spawn()
            .map_err(|e| AudioError::Spawn {
                program: program.to_string(),
                source_msg: e.to_string(),
            })?;
        // _child dropped here — child process continues running independently.
        Ok(())
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
        // Drop the Child — liveness is tracked by pgid in ChildOwner.
        // The process continues running; we kill via `kill(-pgid, SIGTERM)`.
        drop(child);
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
        Ok(())
    }
}

/// In-memory runner for tests: records every argv, replays queued outputs.
/// Mirrors `MockTransport` (G1).
#[derive(Default)]
pub struct MockRunner {
    /// Each recorded call is `[program, arg0, arg1, …]` (from `run` and `spawn_detached`).
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

    /// Records the invocation into the same `calls` vec as `run`, so existing
    /// argv assertions work unchanged. Returns Ok(()) unconditionally.
    fn spawn_detached(&mut self, program: &str, args: &[&str]) -> Result<(), AudioError> {
        let mut call = Vec::with_capacity(args.len() + 1);
        call.push(program.to_string());
        call.extend(args.iter().map(|a| a.to_string()));
        self.calls.push(call);
        Ok(())
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
    fn spawn_detached(&mut self, program: &str, args: &[&str]) -> Result<(), AudioError> {
        (**self).spawn_detached(program, args)
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
    fn mock_spawn_detached_records_into_calls_and_returns_ok() {
        let mut r = MockRunner::new();
        r.spawn_detached("pipewire", &["-c", "/tmp/foo.conf"])
            .expect("spawn_detached never errors in mock");
        assert_eq!(r.calls[0], vec!["pipewire", "-c", "/tmp/foo.conf"]);
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
