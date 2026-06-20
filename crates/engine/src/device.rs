use arctis_device::{DeviceController, DeviceError, Transport};

/// Convenience alias for the value returned by [`DeviceOpener::open`].
pub type OpenResult<T> = Result<Option<(DeviceController<T>, Vec<String>)>, DeviceError>;

/// Opens the device transport on demand. Real impl uses HidrawTransport +
/// discover(); tests inject a closure returning a MockTransport.
pub trait DeviceOpener: Send + 'static {
    type T: Transport;
    /// Returns Ok(None) when no device is connected (graceful), Err on real IO fault.
    fn open(&self) -> OpenResult<Self::T>;
}

/// A write command sent to the device worker thread through its command channel.
///
/// The reply channel carries `Ok(())` on success or a stringified error.
/// SAFETY: writes and reads happen on the same worker thread → serialized (Global Constraint).
pub enum DeviceCommand {
    Set {
        name: String,
        value: i64,
        reply: std::sync::mpsc::Sender<Result<(), String>>,
    },
}

/// Drain all pending [`DeviceCommand`]s from `cmd_rx`, executing each against `controller`.
///
/// Called inside the inner read loop so writes are interleaved between status reads
/// on the same thread — no separate mutex needed.
fn drain_commands<T: Transport>(
    controller: &mut DeviceController<T>,
    cmd_rx: &std::sync::mpsc::Receiver<DeviceCommand>,
) {
    loop {
        match cmd_rx.try_recv() {
            Ok(DeviceCommand::Set { name, value, reply }) => {
                let result = controller.set(&name, value).map_err(|e| e.to_string());
                let _ = reply.send(result);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }
    }
}

/// The read-loop: owns a controller and loops until `stop` is set.
///
/// `cmd_rx` is the write-command channel; when `None`, the loop is read-only
/// (existing behaviour for tests and callers that don't need write support).
pub fn run_read_loop<O: DeviceOpener>(
    opener: O,
    shared: std::sync::Arc<std::sync::Mutex<crate::DeviceShared>>,
    events: Option<std::sync::mpsc::Sender<crate::state::Event>>,
    poll: std::time::Duration,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    cmd_rx: Option<std::sync::mpsc::Receiver<DeviceCommand>>,
) {
    use std::sync::atomic::Ordering;
    while !stop.load(Ordering::Relaxed) {
        match opener.open() {
            Ok(Some((mut controller, _enabled))) => {
                // Read until error/disconnect, then fall back to re-open.
                while !stop.load(Ordering::Relaxed) {
                    // Drain pending write commands before the next read.
                    if let Some(rx) = &cmd_rx {
                        drain_commands(&mut controller, rx);
                    }
                    match controller.read() {
                        Ok(state) => {
                            let fields = crate::state::render_device_fields(&state);
                            if let Ok(mut g) = shared.lock() {
                                g.present = true;
                                g.fields = fields.clone();
                            }
                            if let Some(tx) = &events {
                                let _ = tx.send(crate::state::Event::DeviceState { fields });
                            }
                        }
                        Err(_) => {
                            // disconnect / transient: mark absent, break to re-open.
                            if let Ok(mut g) = shared.lock() {
                                g.present = false;
                            }
                            break;
                        }
                    }
                    std::thread::sleep(poll);
                }
            }
            Ok(None) => {
                if let Ok(mut g) = shared.lock() {
                    g.present = false;
                }
            }
            Err(_) => {
                if let Ok(mut g) = shared.lock() {
                    g.present = false;
                }
            }
        }
        std::thread::sleep(poll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arctis_device::{MockTransport, Registry};
    use arctis_domain::DeviceId;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn nova_desc() -> arctis_device::DeviceDescriptor {
        Registry::builtin()
            .unwrap()
            .find(DeviceId::new(0x1038, 0x12e5))
            .unwrap()
            .clone()
    }

    /// A battery status frame that decodes to battery_charge=100% and mic_muted=true.
    fn battery_frame() -> Vec<u8> {
        let mut f = vec![0u8; 64];
        f[0] = 0x06;
        f[1] = 0xb0;
        f[6] = 8; // raw 8/8 == 100%
        f[9] = 1; // mic_muted = true
        f
    }

    /// MockOpener that returns a DeviceController backed by a MockTransport
    /// with one queued battery frame.
    struct MockOpener {
        desc: arctis_device::DeviceDescriptor,
        frame: Vec<u8>,
    }

    impl DeviceOpener for MockOpener {
        type T = MockTransport;
        fn open(&self) -> Result<Option<(DeviceController<Self::T>, Vec<String>)>, DeviceError> {
            let transport = MockTransport::new().with_response(self.frame.clone());
            let controller = DeviceController::new(transport, self.desc.clone());
            Ok(Some((controller, vec![])))
        }
    }

    /// MockOpener that always returns Ok(None) — device absent.
    /// Signals `tx` on every `open()` call so tests can wait deterministically.
    struct AbsentOpener {
        tx: std::sync::mpsc::Sender<()>,
    }

    impl DeviceOpener for AbsentOpener {
        type T = MockTransport;
        fn open(&self) -> Result<Option<(DeviceController<Self::T>, Vec<String>)>, DeviceError> {
            let _ = self.tx.send(());
            Ok(None)
        }
    }

    /// MockOpener that always returns Err(NotConnected).
    /// Signals `tx` on every `open()` call so tests can wait deterministically.
    struct ErrorOpener {
        tx: std::sync::mpsc::Sender<()>,
    }

    impl DeviceOpener for ErrorOpener {
        type T = MockTransport;
        fn open(&self) -> Result<Option<(DeviceController<Self::T>, Vec<String>)>, DeviceError> {
            let _ = self.tx.send(());
            Err(DeviceError::NotConnected)
        }
    }

    #[test]
    fn read_loop_populates_shared_state_then_stops() {
        let shared = Arc::new(Mutex::new(crate::DeviceShared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();

        let opener = MockOpener {
            desc: nova_desc(),
            frame: battery_frame(),
        };

        let shared_clone = Arc::clone(&shared);
        let stop_clone = Arc::clone(&stop);

        let handle = std::thread::spawn(move || {
            run_read_loop(
                opener,
                shared_clone,
                Some(tx),
                Duration::from_millis(1),
                stop_clone,
                None,
            );
        });

        // Wait for a DeviceState event — this proves the worker did a successful read.
        // Using a 3-second timeout so the test fails clearly if the loop never emits.
        let event = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("DeviceState event must be received within 3s");

        // Verify the event carries the battery value
        match &event {
            crate::state::Event::DeviceState { fields } => {
                assert_eq!(
                    fields.get("battery_charge"),
                    Some(&"100".to_string()),
                    "DeviceState event must carry battery_charge='100'"
                );
            }
            other => panic!("expected DeviceState event, got: {:?}", other),
        }

        // The shared state was also set (event is sent after the shared update)
        {
            let g = shared.lock().unwrap();
            // Fields must contain battery_charge even if `present` may have already
            // toggled back to false (mock transport runs out of queued frames).
            assert_eq!(
                g.fields.get("battery_charge"),
                Some(&"100".to_string()),
                "shared fields must have battery_charge='100'"
            );
        }

        // Stop the loop
        stop.store(true, Ordering::Relaxed);
        handle.join().expect("worker thread must not panic");
    }

    #[test]
    fn read_loop_marks_absent_when_opener_returns_none() {
        let shared = Arc::new(Mutex::new(crate::DeviceShared {
            present: true, // start as present to confirm it gets set to false
            fields: Default::default(),
        }));
        let stop = Arc::new(AtomicBool::new(false));

        let (open_tx, open_rx) = std::sync::mpsc::channel::<()>();

        let shared_clone = Arc::clone(&shared);
        let stop_clone = Arc::clone(&stop);

        let handle = std::thread::spawn(move || {
            run_read_loop(
                AbsentOpener { tx: open_tx },
                shared_clone,
                None,
                Duration::from_millis(1),
                stop_clone,
                None,
            );
        });

        // Wait until open() has been called at least once — proves the loop ran
        // and set present=false. 3-second bound catches genuine hangs.
        open_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("AbsentOpener::open() must be called within 3s");

        stop.store(true, Ordering::Relaxed);
        handle.join().expect("worker thread must not panic");

        let g = shared.lock().unwrap();
        assert!(
            !g.present,
            "device_present must be false when opener returns None"
        );
    }

    #[test]
    fn read_loop_survives_opener_error() {
        let shared = Arc::new(Mutex::new(crate::DeviceShared::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let (open_tx, open_rx) = std::sync::mpsc::channel::<()>();

        let shared_clone = Arc::clone(&shared);
        let stop_clone = Arc::clone(&stop);

        let handle = std::thread::spawn(move || {
            run_read_loop(
                ErrorOpener { tx: open_tx },
                shared_clone,
                None,
                Duration::from_millis(1),
                stop_clone,
                None,
            );
        });

        // Wait until open() has been called at least once — proves the loop ran
        // and set present=false. 3-second bound catches genuine hangs.
        open_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("ErrorOpener::open() must be called within 3s");

        stop.store(true, Ordering::Relaxed);
        handle
            .join()
            .expect("worker thread must not panic on opener error");

        let g = shared.lock().unwrap();
        assert!(
            !g.present,
            "device_present must be false when opener errors"
        );
    }

    // ─────────────────────────────────────────────
    // Task 6: DeviceCommand channel tests
    // ─────────────────────────────────────────────

    /// Helper: opener that provides a controller with `sidetone` enabled so we can
    /// test both the gated path and the success path via the command channel.
    struct EnabledOpener {
        desc: arctis_device::DeviceDescriptor,
        frame: Vec<u8>,
        enabled_writes: Vec<String>,
    }

    impl DeviceOpener for EnabledOpener {
        type T = MockTransport;
        fn open(&self) -> Result<Option<(DeviceController<Self::T>, Vec<String>)>, DeviceError> {
            let transport = MockTransport::new().with_response(self.frame.clone());
            let names: Vec<&str> = self.enabled_writes.iter().map(|s| s.as_str()).collect();
            let controller =
                DeviceController::new(transport, self.desc.clone()).with_enabled_writes(&names);
            Ok(Some((controller, self.enabled_writes.clone())))
        }
    }

    /// send device-set for a NON-enabled control → reply must be Err (gate refused).
    #[test]
    fn device_command_gated_when_not_enabled() {
        let shared = Arc::new(Mutex::new(crate::DeviceShared::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<DeviceCommand>();

        let opener = EnabledOpener {
            desc: nova_desc(),
            frame: battery_frame(),
            enabled_writes: vec![], // nothing enabled
        };

        let shared_clone = Arc::clone(&shared);
        let stop_clone = Arc::clone(&stop);

        let handle = std::thread::spawn(move || {
            run_read_loop(
                opener,
                shared_clone,
                None,
                Duration::from_millis(5),
                stop_clone,
                Some(cmd_rx),
            );
        });

        // Send a write command for a control that is NOT enabled.
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        cmd_tx
            .send(DeviceCommand::Set {
                name: "sidetone".into(),
                value: 2,
                reply: reply_tx,
            })
            .expect("send must succeed while worker is alive");

        // The reply must arrive and be an error (gate refused).
        let result = reply_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("reply must arrive within 3s");
        assert!(
            result.is_err(),
            "non-enabled control must return Err from the worker"
        );
        let msg = result.unwrap_err();
        assert!(
            msg.contains("not enabled") || msg.contains("Unsupported"),
            "error message must mention gating: {msg}"
        );

        stop.store(true, Ordering::Relaxed);
        handle.join().expect("worker must not panic");
    }

    /// send device-set for an ENABLED control → reply must be Ok(()).
    #[test]
    fn device_command_succeeds_when_enabled() {
        let shared = Arc::new(Mutex::new(crate::DeviceShared::default()));
        let stop = Arc::new(AtomicBool::new(false));

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<DeviceCommand>();

        let opener = EnabledOpener {
            desc: nova_desc(),
            frame: battery_frame(),
            enabled_writes: vec!["sidetone".into()], // sidetone enabled
        };

        let shared_clone = Arc::clone(&shared);
        let stop_clone = Arc::clone(&stop);

        let handle = std::thread::spawn(move || {
            run_read_loop(
                opener,
                shared_clone,
                None,
                Duration::from_millis(5),
                stop_clone,
                Some(cmd_rx),
            );
        });

        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        cmd_tx
            .send(DeviceCommand::Set {
                name: "sidetone".into(),
                value: 2,
                reply: reply_tx,
            })
            .expect("send must succeed while worker is alive");

        let result = reply_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("reply must arrive within 3s");
        assert!(
            result.is_ok(),
            "enabled control must return Ok from the worker, got: {result:?}"
        );

        stop.store(true, Ordering::Relaxed);
        handle.join().expect("worker must not panic");
    }
}
