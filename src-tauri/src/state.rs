use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize};

pub struct DaemonState {
    pub socket: PathBuf,
}

/// Whether the main window is (or should be) visible.
///
/// Drives two things:
/// * the deferred first show — `show_when_ready` (commands.rs) only shows the
///   window when this is true, so a `--hidden` tray launch stays hidden;
/// * the poll-cadence gate — hidden-to-tray drops the state/streams polls to a
///   slow cadence (see [`should_poll`]).
///
/// Updated at every Rust-side show/hide site (tray toggle, single-instance
/// activation, close-to-tray) plus the `Focused(true)` window event as a
/// catch-all for any other show path.
pub struct WindowVisibility(pub AtomicBool);

impl WindowVisibility {
    pub fn new(visible: bool) -> Self {
        Self(AtomicBool::new(visible))
    }
}

/// Hidden-to-tray poll gate: while the window is visible, poll on every ticker
/// tick (full cadence); while hidden, only when at least `hidden_ms` elapsed
/// since the last poll. `elapsed_ms = None` (never polled) always polls.
///
/// The underlying ticker stays fast, so the cadence snaps back to full on the
/// first tick after the window is shown again.
pub fn should_poll(elapsed_ms: Option<u128>, visible: bool, hidden_ms: u128) -> bool {
    match (visible, elapsed_ms) {
        (true, _) | (_, None) => true,
        (false, Some(e)) => e >= hidden_ms,
    }
}

/// Shared count of mounted level meters (UI subscribers to the `levels` event).
///
/// Managed Tauri state, incremented by `meter_subscribe` and decremented by
/// `meter_unsubscribe`. The meter task reads it each tick and skips emitting
/// the `levels` event entirely when the count is 0 — so no meter dispatch work
/// competes with scroll compositing on pages that show no meters.
#[derive(Default)]
pub struct MeterSubscribers(pub AtomicUsize);

impl DaemonState {
    pub fn new() -> Self {
        Self {
            socket: arctis_client::socket_path(),
        }
    }
}

impl Default for DaemonState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_window_polls_every_tick() {
        assert!(should_poll(Some(0), true, 3_000));
        assert!(should_poll(Some(250), true, 3_000));
        assert!(should_poll(None, true, 3_000));
    }

    #[test]
    fn first_poll_always_runs_even_hidden() {
        assert!(should_poll(None, false, 3_000));
    }

    #[test]
    fn hidden_window_waits_for_hidden_cadence() {
        assert!(!should_poll(Some(250), false, 3_000));
        assert!(!should_poll(Some(2_999), false, 3_000));
        assert!(should_poll(Some(3_000), false, 3_000));
        assert!(should_poll(Some(10_000), false, 3_000));
    }
}
