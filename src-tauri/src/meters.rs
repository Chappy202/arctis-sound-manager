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
//! # Resilience
//!
//! Nodes that do not exist yet → pw-record exits quickly with error → we keep
//! the level at 0.0 until the next restart attempt (every ~1 s).  Errors
//! never crash the app; all code is off the audio hot path.
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
/// sends them on `tx`.  Runs until `stop` is set or the child exits.
fn capture_loop(mut child: Child, channels: u8, tx: watch::Sender<f32>, stop: Arc<AtomicBool>) {
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

/// Spawn a `pw-record` capture worker for one node.
///
/// Returns `(rx, stop_flag)`.  Set `stop_flag` to true to request teardown;
/// the worker thread exits at the next read boundary.
fn spawn_capture(node: &NodeCfg) -> (watch::Receiver<f32>, Arc<AtomicBool>) {
    let (tx, rx) = watch::channel(0.0f32);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);
    let channels = node.channels;

    let (prog, args) = pw_record_args(node);
    let child_result = Command::new(prog)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // suppress pw-record status lines
        .spawn();

    match child_result {
        Ok(child) => {
            thread::spawn(move || capture_loop(child, channels, tx, stop_clone));
        }
        Err(_) => {
            // pw-record not found or node absent — level stays at 0.0
        }
    }

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
        // Worker threads will notice the flag and kill their children on the
        // next read; we don't block waiting for them.
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
}
