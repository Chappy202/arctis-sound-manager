use arctis_audio::{AudioError, ChildToken, CommandRunner};

/// Tracks every pipewire child the engine spawned, killing them on teardown.
///
/// # Drop and determinism
///
/// Because `kill_all` requires a runner (which is owned by the engine), `ChildOwner`
/// does NOT implement `Drop` with a runner. Instead the engine's `shutdown()` method
/// (and the engine's own `Drop`, holding the runner) calls `kill_all`. This gives a
/// deterministic guarantee: on engine teardown, every tracked process group receives
/// `SIGTERM`.
#[derive(Default)]
pub struct ChildOwner {
    tokens: Vec<ChildToken>,
}

impl ChildOwner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start tracking a spawned child token.
    pub fn track(&mut self, token: ChildToken) {
        self.tokens.push(token);
    }

    /// Number of currently tracked children.
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Returns true if no children are being tracked.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Kill all tracked process groups via the runner, then clear the list.
    ///
    /// Idempotent: calling again after a successful `kill_all` is a no-op.
    /// Errors on the first failure; remaining tokens are not killed in that case.
    pub fn kill_all<R: CommandRunner>(&mut self, runner: &mut R) -> Result<(), AudioError> {
        for token in self.tokens.drain(..) {
            runner.kill_owned(&token)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arctis_audio::MockRunner;

    #[test]
    fn child_owner_kills_all_via_runner() {
        let mut owner = ChildOwner::new();
        let mut runner = MockRunner::new();

        // Track two fake tokens (pgid 0 since MockRunner returns that).
        let t1 = runner
            .spawn_owned("pipewire", &["-c", "/tmp/a.conf"])
            .expect("spawn_owned");
        let t2 = runner
            .spawn_owned("pipewire", &["-c", "/tmp/b.conf"])
            .expect("spawn_owned");
        owner.track(t1);
        owner.track(t2);

        assert_eq!(owner.len(), 2);

        // Kill all — runner records kills.
        owner.kill_all(&mut runner).expect("kill_all");

        assert_eq!(runner.killed.len(), 2, "both tokens should be killed");
        assert_eq!(owner.len(), 0, "owner should be empty after kill_all");
    }

    #[test]
    fn child_owner_kill_all_is_idempotent() {
        let mut owner = ChildOwner::new();
        let mut runner = MockRunner::new();

        let t = runner
            .spawn_owned("pipewire", &["-c", "/tmp/c.conf"])
            .expect("spawn_owned");
        owner.track(t);

        owner.kill_all(&mut runner).expect("first kill_all");
        owner
            .kill_all(&mut runner)
            .expect("second kill_all should be noop");

        // Only one kill should have occurred (from the first call).
        assert_eq!(runner.killed.len(), 1);
        assert_eq!(owner.len(), 0);
    }

    #[test]
    fn child_owner_new_is_empty() {
        let owner = ChildOwner::new();
        assert_eq!(owner.len(), 0);
        assert!(owner.is_empty());
    }

    #[test]
    fn child_owner_track_increases_len() {
        let mut owner = ChildOwner::new();
        let mut runner = MockRunner::new();

        let t = runner
            .spawn_owned("pipewire", &["-c", "/tmp/d.conf"])
            .expect("spawn_owned");
        owner.track(t);
        assert_eq!(owner.len(), 1);
        assert!(!owner.is_empty());
    }
}
