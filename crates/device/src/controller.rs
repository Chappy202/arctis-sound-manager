use crate::codec::{read_status, write_command};
use crate::descriptor::DeviceDescriptor;
use crate::error::DeviceError;
use crate::transport::Transport;
use arctis_domain::DeviceState;

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

    #[cfg(test)]
    pub(crate) fn transport(&self) -> &T {
        &self.transport
    }

    /// Read a full status snapshot. Safe; best-effort merge of frames.
    pub fn read(&mut self) -> Result<DeviceState, DeviceError> {
        Ok(read_status(&mut self.transport, &self.descriptor)?)
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
