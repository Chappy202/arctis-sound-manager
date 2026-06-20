use crate::registry::Registry;
use crate::transport::{Transport, TransportError};
use arctis_domain::DeviceId;
use hidapi::HidApi;

pub struct HidrawTransport {
    device: hidapi::HidDevice,
}

impl HidrawTransport {
    /// Open the matching HID interface for `id` (read/write capable, but this
    /// plan only ever writes status-request reports).
    pub fn open(id: DeviceId, interface: u8) -> Result<Self, TransportError> {
        let api = HidApi::new().map_err(|e| TransportError::Io(e.to_string()))?;
        let info = api
            .device_list()
            .find(|d| {
                d.vendor_id() == id.vendor_id
                    && d.product_id() == id.product_id
                    && d.interface_number() == i32::from(interface)
            })
            .ok_or_else(|| TransportError::NotFound(format!("{id} iface {interface}")))?;
        let device = info
            .open_device(&api)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(Self { device })
    }
}

impl Transport for HidrawTransport {
    fn write_report(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.device
            .write(data)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(())
    }

    fn read_report(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, TransportError> {
        let n = self
            .device
            .read_timeout(buf, timeout_ms)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        if n == 0 {
            return Err(TransportError::Timeout);
        }
        Ok(n)
    }
}

/// Scan connected HID devices for the first one a registry descriptor matches.
/// Returns its `DeviceId` and the descriptor's declared control interface.
pub fn discover(registry: &Registry) -> Result<Option<(DeviceId, u8)>, TransportError> {
    let api = HidApi::new().map_err(|e| TransportError::Io(e.to_string()))?;
    for info in api.device_list() {
        let id = DeviceId::new(info.vendor_id(), info.product_id());
        if let Some(desc) = registry.find(id) {
            if info.interface_number() == i32::from(desc.interface) {
                return Ok(Some((id, desc.interface)));
            }
        }
    }
    Ok(None)
}
