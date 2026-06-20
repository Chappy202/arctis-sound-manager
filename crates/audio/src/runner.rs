use crate::error::AudioError;
use std::process::Command;

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
}

/// In-memory runner for tests: records every argv, replays queued outputs.
/// Mirrors `MockTransport` (G1).
#[derive(Default)]
pub struct MockRunner {
    /// Each recorded call is `[program, arg0, arg1, …]`.
    pub calls: Vec<Vec<String>>,
    queued: std::collections::VecDeque<CmdOutput>,
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
}
