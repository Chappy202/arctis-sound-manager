use arctis_audio::{AudioError, ChildToken, CommandRunner};

/// Tracks every pipewire child the engine spawned, killing them on teardown.
///
/// # Deduplication
///
/// Tokens are keyed by the `ChildToken::label` string (typically the conf path).
/// `track(token)` REPLACES any existing entry for the same label, so reconcile
/// can be called repeatedly without accumulating stale dead tokens.  The old
/// instance is already killed by `recreate`/`remove` before re-tracking, so
/// replacing is always safe.
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
    /// Key = token label (conf path or argv string); value = live token.
    tokens: std::collections::HashMap<String, ChildToken>,
}

impl ChildOwner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start tracking a spawned child token, replacing any previous entry with
    /// the same label.  The old entry's process was already killed by the caller
    /// (via `recreate`/`remove`) before this call, so replacement is safe.
    pub fn track(&mut self, token: ChildToken) {
        self.tokens.insert(token.label.clone(), token);
    }

    /// Number of currently tracked children.
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// Returns true if no children are being tracked.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Kill all tracked process groups via the runner, then clear the map.
    ///
    /// Idempotent: calling again after a successful `kill_all` is a no-op.
    ///
    /// Every token is attempted even if an earlier kill fails. The first error
    /// encountered (if any) is returned after all kills have been tried.
    pub fn kill_all<R: CommandRunner>(&mut self, runner: &mut R) -> Result<(), AudioError> {
        let mut first_err: Option<AudioError> = None;
        for (_label, token) in self.tokens.drain() {
            if let Err(e) = runner.kill_owned(&token) {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
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
    fn child_owner_kill_all_continues_after_first_kill_error() {
        // Verify: if kill of the first token errors, the second token is still killed.
        let mut owner = ChildOwner::new();
        let mut runner = MockRunner::new();

        let t1 = runner
            .spawn_owned("pipewire", &["-c", "/tmp/fail.conf"])
            .expect("spawn_owned");
        let t2 = runner
            .spawn_owned("pipewire", &["-c", "/tmp/ok.conf"])
            .expect("spawn_owned");
        // Make kill of t1 fail by matching its label.
        runner.fail_kill_label = Some(t1.label.clone());
        owner.track(t1);
        owner.track(t2);

        let result = owner.kill_all(&mut runner);

        // kill_all should return an error (from t1's failed kill)...
        assert!(result.is_err(), "expected error from first kill failure");
        // ...but BOTH tokens must have been attempted.
        assert_eq!(
            runner.killed.len(),
            2,
            "both tokens must be attempted even on partial failure"
        );
        // Owner list cleared regardless.
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

    /// Fix #3: re-tracking a token with the same label replaces the old one,
    /// keeping the map bounded to one entry per live channel.
    #[test]
    fn track_deduplicates_by_label_bounds_to_one_per_channel() {
        let mut owner = ChildOwner::new();
        let mut runner = MockRunner::new();

        // Simulate initial reconcile: 3 channels spawn tokens.
        let t_game1 = runner
            .spawn_owned("pipewire", &["-c", "/tmp/Arctis_Game.conf"])
            .expect("spawn_owned game1");
        let t_chat1 = runner
            .spawn_owned("pipewire", &["-c", "/tmp/Arctis_Chat.conf"])
            .expect("spawn_owned chat1");
        let t_media1 = runner
            .spawn_owned("pipewire", &["-c", "/tmp/Arctis_Media.conf"])
            .expect("spawn_owned media1");
        owner.track(t_game1);
        owner.track(t_chat1);
        owner.track(t_media1);
        assert_eq!(owner.len(), 3, "after first reconcile: 3 tracked");

        // Simulate a respawn of the game channel (same label, new token).
        // The old process was already killed by recreate/remove before this call.
        let t_game2 = runner
            .spawn_owned("pipewire", &["-c", "/tmp/Arctis_Game.conf"])
            .expect("spawn_owned game2");
        owner.track(t_game2);

        // Must still be 3, not 4 — the old game token was replaced.
        assert_eq!(
            owner.len(),
            3,
            "re-tracking same label must replace, not append (dedup bound)"
        );

        // All 3 tokens are killed on shutdown.
        owner.kill_all(&mut runner).expect("kill_all");
        assert_eq!(owner.len(), 0);
        // 3 kills: chat1, media1, game2 (game1 was replaced, never killed here).
        assert_eq!(
            runner.killed.len(),
            3,
            "kill_all sends exactly 3 kills (one per live token)"
        );
    }
}
