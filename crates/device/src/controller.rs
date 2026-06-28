use crate::codec::{read_status, write_command};
use crate::descriptor::DeviceDescriptor;
use crate::error::DeviceError;
use crate::transport::{Transport, TransportError};
use arctis_domain::DeviceState;

/// True for an Arctis dial frame: `[0x07, 0x45, …]` (media_mix @2, chat_mix @3).
///
/// Used by [`DeviceController::validate_chatmix`] to identify incoming dial reports
/// after the ChatMix-enable opcode is sent.
pub fn is_dial_frame(frame: &[u8]) -> bool {
    frame.len() >= 2 && frame[0] == 0x07 && frame[1] == 0x45
}

/// Owns the single device transport. The ONLY thing that reads/writes the device.
/// Writes are gated twice: by the descriptor capability AND by the runtime
/// `enabled_writes` allowlist (which the OWNER-RUN validation gates populate).
pub struct DeviceController<T: Transport> {
    transport: T,
    descriptor: DeviceDescriptor,
    enabled_writes: Vec<String>,
}

impl<T: Transport> DeviceController<T> {
    pub fn new(transport: T, descriptor: DeviceDescriptor) -> Self {
        Self {
            transport,
            descriptor,
            enabled_writes: Vec::new(),
        }
    }

    /// Builder: declare which write command names are OWNER-VALIDATED + enabled.
    pub fn with_enabled_writes(mut self, names: &[&str]) -> Self {
        self.enabled_writes = names.iter().map(|s| s.to_string()).collect();
        self
    }

    /// Return a reference to the underlying transport.
    /// Available to integration tests in dependent crates (e.g. arctis-engine) so they
    /// can inspect `MockTransport::written` without a dedicated channel.
    #[cfg(any(test, feature = "testing-utils"))]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Read a full status snapshot. Safe; best-effort merge of frames.
    pub fn read(&mut self) -> Result<DeviceState, DeviceError> {
        Ok(read_status(&mut self.transport, &self.descriptor)?)
    }

    /// OWNER-RUN ChatMix validation. Sends the single `chatmix_enable` opcode
    /// `[0x06,0x49,0x01]` ONCE, then reads up to `max_reads` input reports
    /// (each with `timeout_ms` timeout) and returns `Ok(true)` if any incoming
    /// frame is a dial frame (`[0x07,0x45,…]`), or `Ok(false)` if none are seen.
    ///
    /// # G2 SAFETY — why the allowlist is intentionally not consulted here
    ///
    /// This is the ONE sanctioned path that writes `chatmix_enable` before it is
    /// in the `enabled_writes` allowlist, because VALIDATION MUST PRECEDE ENABLING
    /// (spec §5).  Specifically:
    ///
    /// - **Hardcoded to one opcode.** This method is not a generic gate bypass — it
    ///   calls `write_command("chatmix_enable", 1)` directly and nothing else.
    /// - **Single validated target.** The descriptor still validates the command name
    ///   and value encoding; a typo or bad value is caught before any write.
    /// - **Reachable only via `--validate`.** The CLI guard ensures no automated path
    ///   ever invokes this: without the explicit flag, the opcode is never sent.
    /// - **Recoverable.** If the headset behaves unexpectedly, a replug resets it.
    ///
    /// All automated tests use [`crate::mock::MockTransport`]; the real opcode is
    /// sent only when the owner explicitly runs `device chatmix-enable --validate`
    /// on hardware.
    pub fn validate_chatmix(
        &mut self,
        max_reads: usize,
        timeout_ms: i32,
    ) -> Result<bool, DeviceError> {
        // 1) Send the validation opcode.
        //    The enabled_writes allowlist is intentionally NOT consulted — this IS
        //    the pre-enable validation step.  `write_command` still validates the
        //    command name and value encoding against the descriptor.
        write_command(&mut self.transport, &self.descriptor, "chatmix_enable", 1)?;

        // 2) Watch for dial frames.
        let mut buf = vec![0u8; self.descriptor.report_length];
        for _ in 0..max_reads {
            match self.transport.read_report(&mut buf, timeout_ms) {
                Ok(n) if is_dial_frame(&buf[..n]) => return Ok(true),
                Ok(_) => continue,
                Err(TransportError::Timeout) => continue, // not a dial frame yet — keep trying
                Err(e) => return Err(e.into()),           // surface real IO errors
            }
        }
        Ok(false)
    }

    /// Send a single write command. Refuses unless (a) it is in enabled_writes
    /// AND (b) its capability is present in the descriptor.
    pub fn set(&mut self, name: &str, value: i64) -> Result<(), DeviceError> {
        if !self.enabled_writes.iter().any(|n| n == name) {
            return Err(DeviceError::Unsupported(format!(
                "{name} is not enabled (no validated OWNER-RUN gate)"
            )));
        }
        let spec = self
            .descriptor
            .commands
            .get(name)
            .ok_or_else(|| DeviceError::Unsupported(name.to_string()))?;
        if !self.descriptor.capabilities.contains(&spec.capability) {
            return Err(DeviceError::Unsupported(format!(
                "{name} capability not advertised by device"
            )));
        }
        write_command(&mut self.transport, &self.descriptor, name, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockTransport;
    use crate::registry::Registry;
    use arctis_domain::DeviceId;

    fn nova() -> crate::DeviceDescriptor {
        Registry::builtin()
            .unwrap()
            .find(DeviceId::new(0x1038, 0x12e5))
            .unwrap()
            .clone()
    }

    #[test]
    fn set_refuses_command_not_in_enabled_writes() {
        let d = nova();
        let mut c = DeviceController::new(MockTransport::new(), d).with_enabled_writes(&[]); // nothing enabled yet
        let err = c.set("sidetone", 1).unwrap_err();
        assert!(
            matches!(err, DeviceError::Unsupported(_)),
            "a write must be refused until its OWNER-RUN gate enables it"
        );
        // ...and NOTHING was written.
        assert!(c.transport().written.is_empty());
    }

    #[test]
    fn set_writes_when_enabled_and_capability_present() {
        let d = nova();
        let mut c =
            DeviceController::new(MockTransport::new(), d).with_enabled_writes(&["sidetone"]);
        c.set("sidetone", 2).expect("enabled write succeeds");
        assert_eq!(c.transport().written[0][1], 0x39);
        assert_eq!(c.transport().written[0][2], 2);
    }

    #[test]
    fn set_refuses_when_capability_absent_even_if_enabled() {
        // Descriptor without `mic_led` capability but command present + enabled.
        let mut d = nova();
        d.capabilities
            .retain(|c| *c != arctis_domain::Capability::MicLed);
        let mut c =
            DeviceController::new(MockTransport::new(), d).with_enabled_writes(&["mic_led"]);
        let err = c.set("mic_led", 5).unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported(_)));
    }

    // ── Task B2: is_dial_frame + validate_chatmix tests ─────────────────────

    #[test]
    fn is_dial_frame_true_for_0x07_0x45_prefix() {
        let mut frame = vec![0u8; 64];
        frame[0] = 0x07;
        frame[1] = 0x45;
        assert!(super::is_dial_frame(&frame), "[0x07,0x45,…] must be a dial frame");
    }

    #[test]
    fn is_dial_frame_false_for_other_prefix() {
        let mut frame = vec![0u8; 64];
        frame[0] = 0x06;
        frame[1] = 0xb0;
        assert!(!super::is_dial_frame(&frame), "[0x06,0xb0,…] is not a dial frame");
    }

    #[test]
    fn is_dial_frame_false_for_too_short() {
        assert!(!super::is_dial_frame(&[0x07]), "single-byte slice must return false");
    }

    #[test]
    fn is_dial_frame_false_for_empty() {
        assert!(!super::is_dial_frame(&[]), "empty slice must return false");
    }

    /// validate_chatmix returns Ok(true) when a dial frame arrives, AND the opcode
    /// [0x06,0x49,0x01] was written to the mock transport exactly once.
    #[test]
    fn validate_chatmix_detects_dial_frame_and_records_opcode() {
        let d = nova();
        let report_len = d.report_length;

        // Two non-dial frames then a dial frame.
        let non_dial = {
            let mut f = vec![0u8; report_len];
            f[0] = 0x06;
            f[1] = 0xb0;
            f
        };
        let dial = {
            let mut f = vec![0u8; report_len];
            f[0] = 0x07;
            f[1] = 0x45;
            f[2] = 5; // media_mix sample value
            f[3] = 4; // chat_mix sample value
            f
        };

        let transport = MockTransport::new()
            .with_response(non_dial.clone())
            .with_response(non_dial)
            .with_response(dial);

        let mut c = DeviceController::new(transport, d);
        let result = c.validate_chatmix(8, 50).expect("validate_chatmix must not error");
        assert!(result, "should detect the dial frame and return true");

        // G2 core assertion: opcode [0x06,0x49,0x01] written exactly once to the mock.
        let written = &c.transport().written;
        assert_eq!(written.len(), 1, "exactly one write (no save frame, save=false)");
        assert_eq!(written[0][0], 0x06, "report_id must be 0x06");
        assert_eq!(written[0][1], 0x49, "opcode must be 0x49");
        assert_eq!(written[0][2], 0x01, "value must be 0x01 (enabled)");
        assert!(
            written[0][3..].iter().all(|&b| b == 0),
            "remainder must be zero-padded"
        );
    }

    /// validate_chatmix returns Ok(false) when no dial frames arrive within max_reads,
    /// and the opcode is still written once.
    #[test]
    fn validate_chatmix_times_out_when_no_dial_frames() {
        let d = nova();
        // No queued responses → every read returns Timeout.
        let mut c = DeviceController::new(MockTransport::new(), d);
        let result = c
            .validate_chatmix(4, 10)
            .expect("validate_chatmix must not error on timeout");
        assert!(!result, "no dial frames → should return false");

        // Opcode still written once.
        assert_eq!(c.transport().written.len(), 1, "opcode must be written once");
        assert_eq!(c.transport().written[0][1], 0x49, "opcode byte must be 0x49");
        assert_eq!(c.transport().written[0][2], 0x01, "value must be 0x01");
    }

    // ── chatmix_enable gate tests (Task B1) ──────────────────────────────────

    #[test]
    fn chatmix_enable_refused_while_allowlist_empty_and_nothing_written() {
        // Production default: enabled_writes is empty.  The command must be
        // refused AND the mock transport must record zero bytes written — this is
        // the core G2 safety assertion: the opcode is never sent while the gate is
        // closed.
        let mut c = DeviceController::new(MockTransport::new(), nova()).with_enabled_writes(&[]);
        let err = c.set("chatmix_enable", 1).unwrap_err();
        assert!(
            matches!(err, DeviceError::Unsupported(_)),
            "chatmix_enable must be refused with Unsupported when allowlist is empty"
        );
        // G2 proof: zero bytes on the wire.
        assert!(
            c.transport().written.is_empty(),
            "no bytes must reach the transport while the allowlist gate is closed"
        );
    }

    #[test]
    fn chatmix_enable_sends_exactly_one_report_with_correct_bytes_when_enabled() {
        // When the owner-validated gate is open (name in enabled_writes), exactly
        // ONE report must be written (save=false → no save frame) with the wire
        // bytes [0x06, 0x49, 0x01, 0, …].
        let mut c = DeviceController::new(MockTransport::new(), nova())
            .with_enabled_writes(&["chatmix_enable"]);
        c.set("chatmix_enable", 1).expect("enabled write succeeds");
        assert_eq!(
            c.transport().written.len(),
            1,
            "save=false → exactly one write (no save frame)"
        );
        let report = &c.transport().written[0];
        assert_eq!(report[0], 0x06, "report_id");
        assert_eq!(report[1], 0x49, "opcode");
        assert_eq!(report[2], 0x01, "value=1 (enabled)");
        assert!(
            report[3..].iter().all(|&b| b == 0),
            "remainder must be zero-padded"
        );
    }

    #[test]
    fn chatmix_enable_value_0_encodes_disabled_byte() {
        // Confirms that enum value 0 (\"disabled\") encodes correctly as wire byte 0x00.
        let mut c = DeviceController::new(MockTransport::new(), nova())
            .with_enabled_writes(&["chatmix_enable"]);
        c.set("chatmix_enable", 0).expect("value 0 is a valid enum entry");
        let report = &c.transport().written[0];
        assert_eq!(report[1], 0x49, "opcode");
        assert_eq!(report[2], 0x00, "value=0 (disabled)");
    }

    #[test]
    fn read_delegates_to_read_status() {
        let d = nova();
        let frame = {
            let mut f = vec![0u8; 64];
            f[0] = 0x06;
            f[1] = 0xb0;
            f[6] = 8;
            f[9] = 0;
            f
        };
        let mut c = DeviceController::new(MockTransport::new().with_response(frame), d);
        let state = c.read().expect("read ok");
        assert_eq!(
            state.fields.get("battery_charge"),
            Some(&arctis_domain::StatusValue::Percentage(100))
        );
    }
}
