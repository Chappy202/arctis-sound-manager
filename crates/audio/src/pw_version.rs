//! PipeWire runtime version detection.
//!
//! Runs `pipewire --version` via the `CommandRunner` seam (so it's testable
//! without a live daemon) and parses the `libpipewire X.Y.Z` line into a
//! `(major, minor, patch)` triple. On any parse or spawn failure, returns
//! `None` — callers must treat `None` as "version unknown / unsupported".

use crate::runner::CommandRunner;

/// Parse the output of `pipewire --version` and return `(major, minor, patch)`.
///
/// The canonical line to match is:
/// ```text
/// Compiled with libpipewire 1.4.11
/// ```
/// or just:
/// ```text
/// libpipewire 1.6.7
/// ```
/// We scan every line for a token that starts with `libpipewire` followed by
/// a version string `X.Y.Z`.
///
/// Returns `None` if no matching line is found or the version cannot be parsed.
pub fn parse_pw_version(output: &str) -> Option<(u32, u32, u32)> {
    for line in output.lines() {
        // Find "libpipewire" on the line, then grab the next whitespace-separated token.
        let idx = line.find("libpipewire")?;
        let after = line[idx + "libpipewire".len()..].trim_start();
        // The version string is the first space-delimited token (stops at space or end).
        let version_str = after.split_whitespace().next()?;
        if let Some(triple) = parse_semver(version_str) {
            return Some(triple);
        }
    }
    None
}

fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.splitn(3, '.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next()?.parse::<u32>().ok()?;
    // patch may have a suffix like "-rc1"; only parse the leading digits.
    let patch_str = parts.next()?;
    let patch_digits: String = patch_str
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let patch = patch_digits.parse::<u32>().ok()?;
    Some((major, minor, patch))
}

/// Return true if the given PipeWire version ships the builtin `noisegate`
/// filter-chain plugin (available in ≥ 1.6.0).
pub fn supports_builtin_noisegate(version: (u32, u32, u32)) -> bool {
    version >= (1, 6, 0)
}

/// Query the running PipeWire version via `pipewire --version`.
///
/// Returns `None` on spawn failure or if the version line cannot be parsed.
/// Never panics.
pub fn query_pw_version<R: CommandRunner>(runner: &mut R) -> Option<(u32, u32, u32)> {
    match runner.run("pipewire", &["--version"]) {
        Ok(out) if out.status == 0 => parse_pw_version(&out.stdout),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_pw_version unit tests ────────────────────────────────────────────

    #[test]
    fn parse_version_installed_1_4_11() {
        let out = "Compiled with libpipewire 1.4.11\nUsing libpipewire 1.4.11\n";
        assert_eq!(parse_pw_version(out), Some((1, 4, 11)));
    }

    #[test]
    fn parse_version_1_6_7() {
        let out = "Compiled with libpipewire 1.6.7\n";
        assert_eq!(parse_pw_version(out), Some((1, 6, 7)));
    }

    #[test]
    fn parse_version_bare_line() {
        // Some builds just emit "libpipewire X.Y.Z"
        let out = "libpipewire 1.6.0\n";
        assert_eq!(parse_pw_version(out), Some((1, 6, 0)));
    }

    #[test]
    fn parse_version_patch_with_suffix() {
        // Pre-release tags should still parse the numeric prefix.
        let out = "Compiled with libpipewire 1.6.0-rc1\n";
        assert_eq!(parse_pw_version(out), Some((1, 6, 0)));
    }

    #[test]
    fn parse_version_garbage_returns_none() {
        let out = "some random text\nnot a version\n";
        assert_eq!(parse_pw_version(out), None);
    }

    #[test]
    fn parse_version_empty_returns_none() {
        assert_eq!(parse_pw_version(""), None);
    }

    // ── supports_builtin_noisegate ─────────────────────────────────────────────

    #[test]
    fn noisegate_supported_at_1_6_0() {
        assert!(supports_builtin_noisegate((1, 6, 0)));
    }

    #[test]
    fn noisegate_supported_at_1_6_7() {
        assert!(supports_builtin_noisegate((1, 6, 7)));
    }

    #[test]
    fn noisegate_not_supported_at_1_4_11() {
        assert!(!supports_builtin_noisegate((1, 4, 11)));
    }

    #[test]
    fn noisegate_not_supported_at_1_5_99() {
        assert!(!supports_builtin_noisegate((1, 5, 99)));
    }

    // ── query_pw_version via MockRunner ───────────────────────────────────────

    #[test]
    fn query_pw_version_parses_output() {
        use crate::runner::MockRunner;
        let mut runner = MockRunner::new().with_output(0, "Compiled with libpipewire 1.4.11\n", "");
        let v = query_pw_version(&mut runner);
        assert_eq!(v, Some((1, 4, 11)));
    }

    #[test]
    fn query_pw_version_returns_none_on_failure() {
        use crate::runner::MockRunner;
        let mut runner = MockRunner::new().with_output(1, "", "pipewire not found");
        let v = query_pw_version(&mut runner);
        assert_eq!(v, None);
    }

    #[test]
    fn query_pw_version_returns_none_on_garbage_output() {
        use crate::runner::MockRunner;
        let mut runner = MockRunner::new().with_output(0, "some garbage\n", "");
        let v = query_pw_version(&mut runner);
        assert_eq!(v, None);
    }
}
