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
}
