//! meters.rs — Real signal-peak level metering via `pw-record` PCM capture.
//!
//! # What this measures
//!
//! For each metered node this module spawns a long-lived `pw-record` child
//! process that writes raw s16le PCM to its stdout.  A per-node reader thread
//! reads the stream, computes the peak (max |sample|) over ~40 ms windows
//! (1920 samples @ 48 kHz), normalises to [0.0, 1.0], and sends it on a
//! crossbeam channel.  A coordinator task gathers all channels at ~25 Hz and
//! emits the Tauri `levels` event.
//!
//! **This IS real-time audio signal peak**, not configured volume.  A silent
//! channel at full volume → 0.0.  A clipping signal at low volume → 1.0.
//!
//! # Node targeting
//!
//! * **Sinks** (`Arctis_Game`, `Arctis_Chat`, `Arctis_Media`): pw-record
//!   targets `<node.name>.monitor` (every PipeWire sink has a monitor source).
//! * **Mic source** (`arctis_clean_mic`): pw-record targets the source
//!   directly.
//!
//! # Lifecycle
//!
//! Capture workers only run while at least one LevelMeter is subscribed: the
//! emit loop in lib.rs starts the [`MeterTask`] on the 0→1 subscriber edge and
//! drops it on 1→0 (Drop kills the pw-record children), so a hidden window or
//! a meterless page costs zero capture processes. See [`lifecycle`].
//!
//! # Resilience
//!
//! Each node has a supervisor thread that respawns its pw-record child when it
//! exits (node absent yet, pw-record missing, PipeWire restart). Quick failures
//! back off exponentially from ~1 s to ~5 s; the level holds at 0.0 while no
//! child runs (honest: no data = silence). Errors never crash the app; all
//! code is off the audio hot path.
//!
//! # CPU
//!
//! s16 peak is a single `abs()` + compare per sample — trivial.  Four
//! pw-record processes reading s16 mono/stereo at 48 kHz each consume < 0.5 %
//! CPU on modern hardware.  Emit rate 25 Hz keeps Tauri IPC quiet.

use std::{
    collections::HashMap,
    io::Read,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

use tokio::sync::watch;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A flat map of `node.name` → peak level [0.0, 1.0].
/// Serialised directly as the `levels` Tauri event payload.
pub type LevelsPayload = HashMap<String, f32>;

/// Emit-on-change epsilon. ~0.5/255 — below this the change is imperceptible on
/// any meter bar, so we skip the `levels` event entirely (mirrors the
/// `state-changed` emit-on-change guard in lib.rs).
pub(crate) const EMIT_EPSILON: f32 = 0.002;

/// Returns true if two payloads are equal within [`EMIT_EPSILON`] for every key
/// (same key set, every peak within epsilon). Used to suppress redundant
/// `levels` emits when nothing audible changed since the last tick.
pub(crate) fn levels_unchanged(a: &LevelsPayload, b: &LevelsPayload) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().all(|(k, va)| match b.get(k) {
        Some(vb) => (va - vb).abs() < EMIT_EPSILON,
        None => false,
    })
}

// ---------------------------------------------------------------------------
// Node configuration
// ---------------------------------------------------------------------------

/// Description of one metered node.
#[derive(Clone)]
struct NodeCfg {
    /// The `node.name` key used in the payload.
    name: &'static str,
    /// PipeWire capture target passed to `--target`.
    /// For sinks this is `<name>.monitor`; for the mic it is `<name>`.
    pw_target: &'static str,
    /// Number of capture channels (1 = mono, 2 = stereo).
    channels: u8,
}

const NODES: &[NodeCfg] = &[
    NodeCfg {
        name: "Arctis_Game",
        pw_target: "Arctis_Game.monitor",
        channels: 2,
    },
    NodeCfg {
        name: "Arctis_Chat",
        pw_target: "Arctis_Chat.monitor",
        channels: 2,
    },
    NodeCfg {
        name: "Arctis_Media",
        pw_target: "Arctis_Media.monitor",
        channels: 2,
    },
    NodeCfg {
        name: "arctis_clean_mic",
        pw_target: "arctis_clean_mic",
        channels: 1,
    },
];

/// Sample rate used for all captures.
const RATE: u32 = 48_000;
/// ~40 ms window for peak computation (samples per channel).
const WINDOW_SAMPLES: usize = 1_920;
/// Base delay before respawning an exited pw-record child.
const RESPAWN_DELAY_MS: u64 = 1_000;
/// Cap for the respawn backoff when the child keeps failing quickly.
const RESPAWN_DELAY_MAX_MS: u64 = 5_000;
/// A capture run shorter than this counts as a failed start (node absent,
/// pw-record erroring out) and doubles the backoff; longer runs reset it.
const HEALTHY_RUN_MS: u64 = 2_000;

// ---------------------------------------------------------------------------
// Pure helpers (unit-testable, no subprocess)
// ---------------------------------------------------------------------------

/// Compute the peak absolute sample value from a buffer of raw s16le bytes,
/// normalised to [0.0, 1.0].  Returns 0.0 for empty input.
///
/// Bytes are interpreted as little-endian signed 16-bit samples regardless of
/// the number of channels — peak across all channels is what we want for a
/// level meter.
pub(crate) fn peak_from_s16_bytes(bytes: &[u8]) -> f32 {
    if bytes.len() < 2 {
        return 0.0;
    }
    let mut peak: i16 = 0;
    for chunk in bytes.chunks_exact(2) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
        // abs of i16::MIN overflows i16 — handle explicitly
        let abs = if sample == i16::MIN {
            i16::MAX
        } else {
            sample.unsigned_abs() as i16
        };
        if abs > peak {
            peak = abs;
        }
    }
    peak as f32 / i16::MAX as f32
}

/// Next respawn delay: reset to the base after a healthy run, otherwise double
/// (bounded) so a permanently-absent node doesn't spin a subprocess every second.
pub(crate) fn next_respawn_delay_ms(prev_ms: u64, ran_healthy: bool) -> u64 {
    if ran_healthy {
        RESPAWN_DELAY_MS
    } else {
        (prev_ms * 2).min(RESPAWN_DELAY_MAX_MS)
    }
}

/// What the emit loop (lib.rs) should do this tick, given the UI subscriber
/// count and whether capture workers are currently running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    /// Subscribers gone, workers running → drop the task (kills pw-record children).
    Stop,
    /// No subscribers, nothing running → do nothing.
    Idle,
    /// First subscriber arrived → start capture workers.
    Start,
    /// Subscribers present, workers running → collect + emit.
    Run,
}

/// Map (subscriber count, workers running) → the lifecycle action for this tick.
pub fn lifecycle(subscribers: usize, running: bool) -> Lifecycle {
    match (subscribers, running) {
        (0, true) => Lifecycle::Stop,
        (0, false) => Lifecycle::Idle,
        (_, false) => Lifecycle::Start,
        (_, true) => Lifecycle::Run,
    }
}

/// Build the `pw-record` command-line arguments for a given node config.
///
/// Returns `(program, args)` so the caller can pass them to `Command::new`.
fn pw_record_args(node: &NodeCfg) -> (&'static str, Vec<String>) {
    let args = vec![
        "--target".into(),
        node.pw_target.to_string(),
        "--rate".into(),
        RATE.to_string(),
        "--channels".into(),
        node.channels.to_string(),
        "--format".into(),
        "s16".into(),
        "--raw".into(),
        "-".into(), // write PCM to stdout
    ];
    ("pw-record", args)
}

// ---------------------------------------------------------------------------
// Per-node capture worker
// ---------------------------------------------------------------------------

/// Reads from a `pw-record` child's stdout, computes peaks over windows, and
/// sends them on `tx`.  Returns when `stop` is set or the child exits (EOF /
/// read error); the supervisor decides whether to respawn.
fn capture_loop(mut child: Child, channels: u8, tx: &watch::Sender<f32>, stop: &AtomicBool) {
    let buf_bytes = WINDOW_SAMPLES * channels as usize * 2; // 2 bytes/sample
    let mut buf = vec![0u8; buf_bytes];
    let mut stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            let _ = child.kill();
            return;
        }
    };

    while !stop.load(Ordering::Relaxed) {
        let mut offset = 0;
        // Fill the whole window buffer
        while offset < buf_bytes && !stop.load(Ordering::Relaxed) {
            match stdout.read(&mut buf[offset..]) {
                Ok(0) => {
                    // EOF — child exited
                    let _ = child.wait();
                    return;
                }
                Ok(n) => offset += n,
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let peak = peak_from_s16_bytes(&buf[..offset]);
        let _ = tx.send(peak);
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Supervisor: (re)spawns the pw-record child for one node until `stop` is
/// set.  When the child exits (node not created yet, pw-record missing,
/// PipeWire restart) the level is reset to 0.0 and the child is respawned
/// after a bounded backoff — so meters recover when the daemon/sinks come up
/// after the GUI, instead of staying dead until relaunch.
fn supervise_capture(node: NodeCfg, tx: watch::Sender<f32>, stop: Arc<AtomicBool>) {
    let mut delay_ms = RESPAWN_DELAY_MS;
    while !stop.load(Ordering::Relaxed) {
        let started = std::time::Instant::now();
        let (prog, args) = pw_record_args(&node);
        let child_result = Command::new(prog)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null()) // suppress pw-record status lines
            .spawn();
        if let Ok(child) = child_result {
            capture_loop(child, node.channels, &tx, &stop);
        }
        // Spawn failure (pw-record not found) falls through to the same
        // backoff path — it may appear later (e.g. PATH/env fixed by reinstall).
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let _ = tx.send(0.0); // no child → honest silence
        let healthy = started.elapsed().as_millis() as u64 >= HEALTHY_RUN_MS;
        delay_ms = next_respawn_delay_ms(delay_ms, healthy);
        sleep_unless_stopped(delay_ms, &stop);
    }
}

/// Sleep `total_ms` in small slices, returning early once `stop` is set so a
/// dropped MeterTask never leaves a supervisor respawning a child.
fn sleep_unless_stopped(total_ms: u64, stop: &AtomicBool) {
    let mut remaining = total_ms;
    while remaining > 0 && !stop.load(Ordering::Relaxed) {
        let step = remaining.min(100);
        thread::sleep(std::time::Duration::from_millis(step));
        remaining -= step;
    }
}

/// Spawn the supervised capture worker for one node.
///
/// Returns `(rx, stop_flag)`.  Set `stop_flag` to true to request teardown;
/// the supervisor exits at the next read/sleep boundary and kills its child.
fn spawn_capture(node: &NodeCfg) -> (watch::Receiver<f32>, Arc<AtomicBool>) {
    let (tx, rx) = watch::channel(0.0f32);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);
    let node = node.clone();
    thread::spawn(move || supervise_capture(node, tx, stop_clone));
    (rx, stop)
}

// ---------------------------------------------------------------------------
// Public API — metering task
// ---------------------------------------------------------------------------

/// Handle returned by [`start_meter_task`].  Dropping it stops all capture
/// workers cleanly (no leaked pw-record processes).
pub struct MeterTask {
    stop_flags: Vec<Arc<AtomicBool>>,
    receivers: Vec<(String, watch::Receiver<f32>)>,
}

impl MeterTask {
    /// Collect the current peak levels from all capture workers.
    pub fn current_levels(&mut self) -> LevelsPayload {
        self.receivers
            .iter_mut()
            .map(|(name, rx)| (name.clone(), *rx.borrow_and_update()))
            .collect()
    }
}

impl Drop for MeterTask {
    fn drop(&mut self) {
        for flag in &self.stop_flags {
            flag.store(true, Ordering::Relaxed);
        }
        // Supervisor threads notice the flag at the next read/sleep boundary
        // and kill their children; we don't block waiting for them.
    }
}

/// Start capture workers for all configured nodes.
///
/// Call [`MeterTask::current_levels`] at the desired emit rate to collect
/// peaks.  Drop the `MeterTask` to tear down all workers.
pub fn start_meter_task() -> MeterTask {
    let mut stop_flags = Vec::with_capacity(NODES.len());
    let mut receivers = Vec::with_capacity(NODES.len());

    for node in NODES {
        let (rx, stop) = spawn_capture(node);
        stop_flags.push(stop);
        receivers.push((node.name.to_string(), rx));
    }

    MeterTask {
        stop_flags,
        receivers,
    }
}

// ---------------------------------------------------------------------------
// Unit tests (no subprocess)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- peak_from_s16_bytes ---

    #[test]
    fn peak_of_empty_is_zero() {
        assert_eq!(peak_from_s16_bytes(&[]), 0.0);
    }

    #[test]
    fn peak_of_single_odd_byte_is_zero() {
        // Less than 2 bytes → no complete sample
        assert_eq!(peak_from_s16_bytes(&[0xFF]), 0.0);
    }

    #[test]
    fn peak_of_silence_is_zero() {
        let silence = vec![0u8; 3840]; // 1920 stereo samples all zero
        assert_eq!(peak_from_s16_bytes(&silence), 0.0);
    }

    #[test]
    fn peak_of_full_scale_positive_is_one() {
        // i16::MAX = 32767 in little-endian = [0xFF, 0x7F]
        let full_pos: Vec<u8> = vec![0xFF, 0x7F]; // one sample at i16::MAX
        let level = peak_from_s16_bytes(&full_pos);
        assert!((level - 1.0).abs() < 1e-5, "got {level}");
    }

    #[test]
    fn peak_of_full_scale_negative_is_one() {
        // i16::MIN = -32768 in LE = [0x00, 0x80] — abs saturates to i16::MAX
        let full_neg: Vec<u8> = vec![0x00, 0x80];
        let level = peak_from_s16_bytes(&full_neg);
        assert!((level - 1.0).abs() < 1e-5, "got {level}");
    }

    #[test]
    fn peak_of_half_scale() {
        // 16384 = 0x4000, LE = [0x00, 0x40]
        let half: Vec<u8> = vec![0x00, 0x40];
        let level = peak_from_s16_bytes(&half);
        // 16384 / 32767 ≈ 0.5000
        assert!((level - 0.5).abs() < 0.001, "got {level}");
    }

    #[test]
    fn peak_picks_max_across_channels() {
        // Two stereo samples: (0, 32767) → peak = 32767
        let mut buf = Vec::new();
        buf.extend_from_slice(&0i16.to_le_bytes()); // L channel: 0
        buf.extend_from_slice(&i16::MAX.to_le_bytes()); // R channel: 32767
        let level = peak_from_s16_bytes(&buf);
        assert!((level - 1.0).abs() < 1e-5, "got {level}");
    }

    #[test]
    fn peak_picks_max_across_multiple_windows() {
        // Mix quiet samples then a loud one
        let mut buf = vec![0u8; 100];
        // Append one loud sample at end: i16::MAX
        buf.extend_from_slice(&i16::MAX.to_le_bytes());
        let level = peak_from_s16_bytes(&buf);
        assert!((level - 1.0).abs() < 1e-5, "got {level}");
    }

    // --- pw_record_args ---

    #[test]
    fn pw_record_args_for_game_sink_targets_monitor() {
        let node = &NODES[0]; // Arctis_Game
        let (prog, args) = pw_record_args(node);
        assert_eq!(prog, "pw-record");
        // --target must be the .monitor suffix
        let target_idx = args.iter().position(|a| a == "--target").unwrap();
        assert_eq!(args[target_idx + 1], "Arctis_Game.monitor");
    }

    #[test]
    fn pw_record_args_for_mic_targets_source_directly() {
        let node = &NODES[3]; // arctis_clean_mic
        let (_prog, args) = pw_record_args(node);
        let target_idx = args.iter().position(|a| a == "--target").unwrap();
        assert_eq!(args[target_idx + 1], "arctis_clean_mic");
    }

    #[test]
    fn pw_record_args_uses_s16_format() {
        let node = &NODES[0];
        let (_prog, args) = pw_record_args(node);
        let fmt_idx = args.iter().position(|a| a == "--format").unwrap();
        assert_eq!(args[fmt_idx + 1], "s16");
    }

    #[test]
    fn pw_record_args_uses_48000_rate() {
        let node = &NODES[0];
        let (_prog, args) = pw_record_args(node);
        let rate_idx = args.iter().position(|a| a == "--rate").unwrap();
        assert_eq!(args[rate_idx + 1], "48000");
    }

    #[test]
    fn pw_record_args_ends_with_dash_for_stdout() {
        let node = &NODES[0];
        let (_prog, args) = pw_record_args(node);
        assert_eq!(args.last().unwrap(), "-");
    }

    #[test]
    fn pw_record_args_mic_uses_mono() {
        let node = &NODES[3]; // arctis_clean_mic
        let (_prog, args) = pw_record_args(node);
        let ch_idx = args.iter().position(|a| a == "--channels").unwrap();
        assert_eq!(args[ch_idx + 1], "1");
    }

    #[test]
    fn pw_record_args_sink_uses_stereo() {
        let node = &NODES[0]; // Arctis_Game sink
        let (_prog, args) = pw_record_args(node);
        let ch_idx = args.iter().position(|a| a == "--channels").unwrap();
        assert_eq!(args[ch_idx + 1], "2");
    }

    // --- Node config sanity ---

    #[test]
    fn all_sink_nodes_target_monitor_port() {
        for node in NODES.iter().filter(|n| n.name != "arctis_clean_mic") {
            assert!(
                node.pw_target.ends_with(".monitor"),
                "sink {name} should target .monitor, got {target}",
                name = node.name,
                target = node.pw_target
            );
        }
    }

    #[test]
    fn mic_node_targets_source_directly() {
        let mic = NODES.iter().find(|n| n.name == "arctis_clean_mic").unwrap();
        assert_eq!(mic.pw_target, "arctis_clean_mic");
    }

    // --- levels_unchanged (emit-on-change guard) ---

    fn payload(pairs: &[(&str, f32)]) -> LevelsPayload {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn identical_payloads_are_unchanged() {
        let a = payload(&[("Arctis_Game", 0.5), ("Arctis_Chat", 0.1)]);
        let b = payload(&[("Arctis_Game", 0.5), ("Arctis_Chat", 0.1)]);
        assert!(levels_unchanged(&a, &b));
    }

    #[test]
    fn sub_epsilon_change_is_unchanged() {
        let a = payload(&[("Arctis_Game", 0.5000)]);
        let b = payload(&[("Arctis_Game", 0.5000 + EMIT_EPSILON / 2.0)]);
        assert!(levels_unchanged(&a, &b));
    }

    #[test]
    fn supra_epsilon_change_is_changed() {
        let a = payload(&[("Arctis_Game", 0.50)]);
        let b = payload(&[("Arctis_Game", 0.51)]);
        assert!(!levels_unchanged(&a, &b));
    }

    #[test]
    fn different_key_sets_are_changed() {
        let a = payload(&[("Arctis_Game", 0.5)]);
        let b = payload(&[("Arctis_Chat", 0.5)]);
        assert!(!levels_unchanged(&a, &b));
        let c = payload(&[("Arctis_Game", 0.5), ("Arctis_Chat", 0.5)]);
        assert!(!levels_unchanged(&a, &c));
    }

    // --- lifecycle (subscriber count → task lifecycle) ---

    #[test]
    fn first_subscriber_starts_workers() {
        assert_eq!(lifecycle(1, false), Lifecycle::Start);
        assert_eq!(lifecycle(3, false), Lifecycle::Start);
    }

    #[test]
    fn last_unsubscribe_stops_workers() {
        assert_eq!(lifecycle(0, true), Lifecycle::Stop);
    }

    #[test]
    fn steady_states_keep_or_idle() {
        assert_eq!(lifecycle(0, false), Lifecycle::Idle);
        assert_eq!(lifecycle(2, true), Lifecycle::Run);
    }

    // --- next_respawn_delay_ms (bounded backoff) ---

    #[test]
    fn healthy_run_resets_backoff_to_base() {
        assert_eq!(next_respawn_delay_ms(RESPAWN_DELAY_MAX_MS, true), RESPAWN_DELAY_MS);
        assert_eq!(next_respawn_delay_ms(RESPAWN_DELAY_MS, true), RESPAWN_DELAY_MS);
    }

    #[test]
    fn quick_failure_doubles_backoff() {
        assert_eq!(next_respawn_delay_ms(RESPAWN_DELAY_MS, false), RESPAWN_DELAY_MS * 2);
    }

    #[test]
    fn backoff_is_capped() {
        assert_eq!(next_respawn_delay_ms(RESPAWN_DELAY_MAX_MS, false), RESPAWN_DELAY_MAX_MS);
        assert_eq!(next_respawn_delay_ms(4_000, false), RESPAWN_DELAY_MAX_MS);
    }
}
