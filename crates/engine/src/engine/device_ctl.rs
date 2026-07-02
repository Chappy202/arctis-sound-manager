//! Device-worker interaction: gated hardware writes and ChatMix validation.
use super::*;

/// How long `device_set` waits for the DeviceWorker's reply before failing with a
/// typed error. Bounded so a wedged/absent worker can never block the caller (and
/// with it the daemon-wide engine mutex) forever.
const DEVICE_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Reply timeout for ChatMix validation: the validation itself takes ~6 s of reads
/// and may additionally queue behind a status-read cycle.
const CHATMIX_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Bounded wait for a device-worker reply. Timeout and a disconnected worker both
/// surface as typed `EngineError`s (G7) instead of blocking indefinitely.
fn recv_device_reply<T>(
    rx: &std::sync::mpsc::Receiver<Result<T, String>>,
    timeout: std::time::Duration,
) -> Result<Result<T, String>, EngineError> {
    use std::sync::mpsc::RecvTimeoutError;
    match rx.recv_timeout(timeout) {
        Ok(r) => Ok(r),
        Err(RecvTimeoutError::Timeout) => Err(EngineError::Device(format!(
            "no reply from device worker within {}s (worker busy or wedged)",
            timeout.as_secs()
        ))),
        Err(RecvTimeoutError::Disconnected) => {
            Err(EngineError::BadRequest("no reply from device worker".into()))
        }
    }
}

impl<R: CommandRunner> Engine<R> {
    /// Return a clone of the Arc holding the shared device state.
    /// The DeviceWorker (spawned externally) writes to this; engine::state() reads it.
    pub fn device_shared(&self) -> std::sync::Arc<std::sync::Mutex<crate::state::DeviceShared>> {
        Arc::clone(&self.device)
    }

    /// Wire up the DeviceWorker command channel so `device_set` can route writes
    /// to the single-owner worker thread. Called after the worker is spawned.
    pub fn set_device_tx(&mut self, tx: std::sync::mpsc::Sender<crate::device::DeviceCommand>) {
        self.device_tx = Some(tx);
    }

    /// Send a validated device write through the worker thread.
    ///
    /// Returns `Err` if:
    /// - the worker is not running (`device_tx` is `None`),
    /// - the channel is broken (worker thread died), or
    /// - the write is rejected by the `enabled_writes` gate (control not yet owner-validated).
    ///
    /// Surfaces all failures — never swallows errors.
    ///
    /// Waits at most [`DEVICE_REPLY_TIMEOUT`] for the worker's reply: if the worker
    /// is wedged (or the device detached mid-flight) the caller gets a typed error
    /// instead of blocking forever while holding the daemon-wide engine mutex.
    pub fn device_set(&self, name: &str, value: i64) -> Result<(), EngineError> {
        self.device_set_with_timeout(name, value, DEVICE_REPLY_TIMEOUT)
    }

    /// `device_set` with an explicit reply timeout (tests inject a short one).
    fn device_set_with_timeout(
        &self,
        name: &str,
        value: i64,
        timeout: std::time::Duration,
    ) -> Result<(), EngineError> {
        let tx = self
            .device_tx
            .as_ref()
            .ok_or_else(|| EngineError::BadRequest("device worker not running".into()))?;
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        tx.send(crate::device::DeviceCommand::Set {
            name: name.to_string(),
            value,
            reply: reply_tx,
        })
        .map_err(|_| EngineError::BadRequest("device worker gone".into()))?;
        recv_device_reply(&reply_rx, timeout)?.map_err(EngineError::Device)
    }

    /// OWNER-RUN ChatMix validation: sends `[0x06,0x49,0x01]` once via the device
    /// worker and watches for dial frames `[0x07,0x45]` for ~6 s.
    ///
    /// Returns `Ok(true)` if any dial frame was seen, `Ok(false)` if none arrived
    /// within the timeout, or `Err` on a transport fault or if the worker is absent.
    ///
    /// Reachable only via the explicit `--validate` CLI flag — never from normal daemon
    /// request handling.  Mirrors [`device_set`] in structure but sends
    /// `DeviceCommand::ValidateChatmix` and receives a `bool` reply.
    pub fn validate_chatmix(&self) -> Result<bool, EngineError> {
        let tx = self
            .device_tx
            .as_ref()
            .ok_or_else(|| EngineError::BadRequest("device worker not running".into()))?;
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        tx.send(crate::device::DeviceCommand::ValidateChatmix { reply: reply_tx })
            .map_err(|_| EngineError::BadRequest("device worker gone".into()))?;
        // Validation legitimately runs ~6 s of reads and may queue behind a read
        // cycle — allow generous headroom, but never wait forever.
        recv_device_reply(&reply_rx, CHATMIX_REPLY_TIMEOUT)?.map_err(EngineError::Device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;

    // ─────────────────────────────────────────────
    // TDD: Task 6 — engine.device_set
    // ─────────────────────────────────────────────

    #[test]
    fn device_set_errors_when_worker_not_wired() {
        let cfg = make_config_no_eq_no_routes();
        let engine = Engine::new(MockRunner::new(), cfg);
        // device_tx is None — must return BadRequest
        let result = engine.device_set("sidetone", 2);
        assert!(
            matches!(result, Err(EngineError::BadRequest(_))),
            "must error with BadRequest when worker not running: {result:?}"
        );
    }

    #[test]
    fn device_set_returns_gated_error_when_control_not_enabled() {
        // Wire a fake worker channel backed by a receiver that always replies Err (gate refused).
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<crate::device::DeviceCommand>();
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        engine.set_device_tx(cmd_tx);

        // Spawn a fake worker that drains commands and sends back a gate-refused error.
        let worker = std::thread::spawn(move || {
            while let Ok(crate::device::DeviceCommand::Set { reply, .. }) = cmd_rx.recv() {
                let _ = reply.send(Err(
                    "sidetone is not enabled (no validated OWNER-RUN gate)".into()
                ));
            }
        });

        let result = engine.device_set("sidetone", 2);
        assert!(
            matches!(result, Err(EngineError::Device(_))),
            "gate-refused reply must surface as EngineError::Device: {result:?}"
        );
        if let Err(EngineError::Device(msg)) = result {
            assert!(
                msg.contains("not enabled") || msg.contains("OWNER-RUN"),
                "error message must mention the gate: {msg}"
            );
        }

        // Drop engine (which drops the cmd_tx) to let the worker finish.
        drop(engine);
        worker.join().expect("fake worker must not panic");
    }

    #[test]
    fn device_set_times_out_with_typed_error_when_worker_never_replies() {
        // Wire a command channel whose receiver stays alive but NEVER replies —
        // models a wedged worker (or a command queued while the reader is stuck).
        let (cmd_tx, _cmd_rx) = std::sync::mpsc::channel::<crate::device::DeviceCommand>();
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        engine.set_device_tx(cmd_tx);

        let start = std::time::Instant::now();
        let result = engine.device_set_with_timeout(
            "sidetone",
            2,
            std::time::Duration::from_millis(50),
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(3),
            "device_set must return promptly, not hang"
        );
        match result {
            Err(EngineError::Device(msg)) => assert!(
                msg.contains("no reply"),
                "timeout error must mention the missing reply: {msg}"
            ),
            other => panic!("expected typed Device timeout error, got: {other:?}"),
        }
    }

    #[test]
    fn device_set_returns_ok_when_worker_accepts() {
        // Wire a fake worker channel that always replies Ok(()).
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<crate::device::DeviceCommand>();
        let cfg = make_config_no_eq_no_routes();
        let mut engine = Engine::new(MockRunner::new(), cfg);
        engine.set_device_tx(cmd_tx);

        let worker = std::thread::spawn(move || {
            while let Ok(crate::device::DeviceCommand::Set { name, value, reply }) = cmd_rx.recv() {
                assert_eq!(name, "sidetone", "worker received correct control name");
                assert_eq!(value, 2, "worker received correct value");
                let _ = reply.send(Ok(()));
            }
        });

        let result = engine.device_set("sidetone", 2);
        assert!(
            result.is_ok(),
            "worker-accepted write must return Ok: {result:?}"
        );

        drop(engine);
        worker.join().expect("fake worker must not panic");
    }
}
