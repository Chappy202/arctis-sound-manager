use crate::descriptor::{DeviceDescriptor, Parser, StatusField};
use crate::transport::{Transport, TransportError};
use arctis_domain::{DeviceState, StatusValue};

/// Decode a single status frame into device state using a descriptor.
pub fn decode_frame(desc: &DeviceDescriptor, frame: &[u8]) -> DeviceState {
    let mut state = DeviceState::default();
    for field in &desc.status.fields {
        if frame_matches(frame, &field.match_prefix) {
            if let Some(value) = parse_field(field, frame) {
                state.fields.insert(field.name.clone(), value);
            }
        }
    }
    state
}

fn frame_matches(frame: &[u8], prefix: &[u8]) -> bool {
    frame.len() >= prefix.len() && frame[..prefix.len()] == *prefix
}

fn parse_field(field: &StatusField, frame: &[u8]) -> Option<StatusValue> {
    let raw = *frame.get(field.offset)?;
    Some(match &field.parser {
        Parser::Percentage { min, max } => {
            let span = max.saturating_sub(*min).max(1) as u32;
            let clamped = raw.clamp(*min, *max);
            let pct = (clamped.saturating_sub(*min) as u32 * 100 / span) as u8;
            StatusValue::Percentage(pct)
        }
        Parser::Bool { true_value } => StatusValue::Bool(raw == *true_value),
        Parser::Enum { entries } => entries
            .iter()
            .find(|e| e.value == raw)
            .map(|e| StatusValue::Enum(e.label.clone()))
            .unwrap_or(StatusValue::Int(raw as i64)),
        Parser::Int => StatusValue::Int(raw as i64),
    })
}

/// Build the status-request report, send it, read one frame, and decode it.
pub fn read_status<T: Transport>(
    transport: &mut T,
    desc: &DeviceDescriptor,
) -> Result<DeviceState, TransportError> {
    let mut report = Vec::with_capacity(desc.report_length);
    report.push(desc.report_id);
    report.extend_from_slice(&desc.status.request);
    report.resize(desc.report_length, 0);
    transport.write_report(&report)?;

    let mut buf = vec![0u8; desc.report_length];
    let n = transport.read_report(&mut buf, 500)?;
    Ok(decode_frame(desc, &buf[..n]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockTransport;
    use crate::registry::Registry;
    use arctis_domain::StatusValue;

    fn nova() -> DeviceDescriptor {
        Registry::builtin()
            .unwrap()
            .find(arctis_domain::DeviceId::new(0x1038, 0x12e5))
            .unwrap()
            .clone()
    }

    fn frame(prefix: &[u8], pairs: &[(usize, u8)]) -> Vec<u8> {
        let mut f = vec![0u8; 64];
        f[..prefix.len()].copy_from_slice(prefix);
        for (i, v) in pairs {
            f[*i] = *v;
        }
        f
    }

    #[test]
    fn decodes_battery_percentage_from_0to8_scale() {
        let d = nova();
        // raw battery 4 of 0..8 == 50%
        let f = frame(&[0x06, 0xb0], &[(6, 4)]);
        let state = decode_frame(&d, &f);
        assert_eq!(
            state.fields.get("battery_charge"),
            Some(&StatusValue::Percentage(50))
        );
    }

    #[test]
    fn decodes_anc_enum_from_separate_frame_header() {
        let d = nova();
        let f = frame(&[0x07, 0xbd], &[(2, 1)]);
        let state = decode_frame(&d, &f);
        assert_eq!(
            state.fields.get("anc_mode"),
            Some(&StatusValue::Enum("transparency".into()))
        );
    }

    #[test]
    fn read_status_sends_request_then_decodes_response() {
        let d = nova();
        let response = frame(&[0x06, 0xb0], &[(6, 8), (9, 1)]);
        let mut t = MockTransport::new().with_response(response);

        let state = read_status(&mut t, &d).expect("should read");

        // request was padded to report_length and starts with report_id + request bytes
        assert_eq!(t.written.len(), 1);
        assert_eq!(t.written[0].len(), 64);
        assert_eq!(&t.written[0][..2], &[0x06, 0xb0]);
        // battery 8/8 == 100%, mic muted
        assert_eq!(
            state.fields.get("battery_charge"),
            Some(&StatusValue::Percentage(100))
        );
        assert_eq!(
            state.fields.get("mic_muted"),
            Some(&StatusValue::Bool(true))
        );
    }
}
