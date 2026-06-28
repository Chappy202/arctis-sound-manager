use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;

pub struct DaemonState {
    pub socket: PathBuf,
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
