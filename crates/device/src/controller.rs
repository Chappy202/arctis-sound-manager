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
