use thiserror::Error;

/// Errors from the audio backend. All fallible paths return this; no
/// `unwrap`/`expect` on runtime paths (ARCHITECTURE G7).
#[derive(Debug, Error)]
pub enum AudioError {
    /// A subprocess could not be spawned or its I/O failed.
    #[error("command `{program}` failed to run: {source_msg}")]
    Spawn { program: String, source_msg: String },
    /// A subprocess ran but exited non-zero.
    #[error("command `{program}` exited with status {status}: {stderr}")]
    NonZeroExit {
        program: String,
        status: i32,
        stderr: String,
    },
    /// A generated argument or config was invalid (programmer/data error).
    #[error("invalid audio request: {0}")]
    Invalid(String),
    /// Expected output (e.g. a node id) could not be parsed.
    #[error("could not parse `{what}` from output: {detail}")]
    Parse { what: String, detail: String },
    /// A subprocess exceeded its execution time bound and was killed.
    #[error("command `{program}` timed out after {millis} ms and was killed")]
    Timeout { program: String, millis: u128 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonzero_exit_displays_program_and_stderr() {
        let e = AudioError::NonZeroExit {
            program: "pw-cli".into(),
            status: 1,
            stderr: "boom".into(),
        };
        assert_eq!(e.to_string(), "command `pw-cli` exited with status 1: boom");
    }
}
