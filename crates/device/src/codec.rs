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

/// Maximum number of response frames drained in a single [] call.
const MAX_STATUS_FRAMES: usize = 8;

/// Build the status-request report, send it, and drain up to [`MAX_STATUS_FRAMES`]
/// response frames, merging decoded fields into a single [`DeviceState`].
///
/// # Partial-snapshot contract
///
/// The Nova Pro (and similar devices) scatter status fields across multiple
/// frame headers (`[0x06,0xb0]`, `[0x07,0xbd]`, `[0x07,0x45]`, …).  A single
/// request causes the device to push several frames; each frame carries only the
/// subset of fields whose `match_prefix` matches that frame's header.
///
/// `read_status` reads frames until one of:
/// - every field declared in `desc.status.fields` is present in the accumulator,
/// - a [`TransportError::Timeout`] is returned (no more frames available), or
/// - [`MAX_STATUS_FRAMES`] frames have been read.
///
/// If a non-timeout error occurs *before* any frame has been successfully
/// decoded, that error is propagated.  If at least one frame was already merged
/// into the accumulator, the partial state is returned instead of erring.
pub fn read_status<T: Transport>(
    transport: &mut T,
    desc: &DeviceDescriptor,
) -> Result<DeviceState, TransportError> {
    let mut report = Vec::with_capacity(desc.report_length);
    report.push(desc.report_id);
    report.extend_from_slice(&desc.status.request);
    report.resize(desc.report_length, 0);
    transport.write_report(&report)?;

    let all_field_names: Vec<&str> = desc.status.fields.iter().map(|f| f.name.as_str()).collect();
    let mut state = DeviceState::default();

    for _ in 0..MAX_STATUS_FRAMES {
        // Check if every declared field has been accumulated.
        if all_field_names
            .iter()
            .all(|n| state.fields.contains_key(*n))
        {
            break;
        }

        let mut buf = vec![0u8; desc.report_length];
        match transport.read_report(&mut buf, 500) {
            Ok(n) => {
                let frame_state = decode_frame(desc, &buf[..n]);
                state.fields.extend(frame_state.fields);
            }
            Err(TransportError::Timeout) => break,
            Err(e) => {
                if state.fields.is_empty() {
                    return Err(e);
                }
                break;
            }
        }
    }

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::parse_descriptor;
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

        // report_id must be the very first byte; request bytes follow immediately after
        assert_eq!(t.written.len(), 1);
        assert_eq!(t.written[0].len(), 64);
        assert_eq!(
            t.written[0][0], d.report_id,
            "first byte must be report_id, not a request byte"
        );
        assert_eq!(
            &t.written[0][1..1 + d.status.request.len()],
            &d.status.request[..],
            "request bytes must follow report_id in the correct order"
        );
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

    /// Three queued frames (one per header) must all be merged into a single DeviceState.
    #[test]
    fn read_status_merges_three_frames_for_nova_pro() {
        let d = nova();
        // Frame 1: [0x06,0xb0] -> battery_charge (8/8=100%) + mic_muted (1=true)
        let frame1 = frame(&[0x06, 0xb0], &[(6, 8), (9, 1)]);
        // Frame 2: [0x07,0xbd] -> anc_mode (1=transparency)
        let frame2 = frame(&[0x07, 0xbd], &[(2, 1)]);
        // Frame 3: [0x07,0x45] -> media_mix=42, chat_mix=7
        let frame3 = frame(&[0x07, 0x45], &[(2, 42), (3, 7)]);

        let mut t = MockTransport::new()
            .with_response(frame1)
            .with_response(frame2)
            .with_response(frame3);

        let state = read_status(&mut t, &d).expect("should read all three frames");

        assert_eq!(
            state.fields.get("battery_charge"),
            Some(&StatusValue::Percentage(100)),
            "battery_charge must be decoded from frame1"
        );
        assert_eq!(
            state.fields.get("mic_muted"),
            Some(&StatusValue::Bool(true)),
            "mic_muted must be decoded from frame1"
        );
        assert_eq!(
            state.fields.get("anc_mode"),
            Some(&StatusValue::Enum("transparency".into())),
            "anc_mode must be decoded from frame2"
        );
        assert_eq!(
            state.fields.get("media_mix"),
            Some(&StatusValue::Int(42)),
            "media_mix must be decoded from frame3"
        );
        assert_eq!(
            state.fields.get("chat_mix"),
            Some(&StatusValue::Int(7)),
            "chat_mix must be decoded from frame3"
        );
    }

    /// A single queued frame followed by Timeout must return that frame's fields without error.
    #[test]
    fn read_status_returns_partial_state_on_timeout_after_first_frame() {
        let d = nova();
        // Only queue one frame; the next read will time out -> loop breaks cleanly.
        let response = frame(&[0x06, 0xb0], &[(6, 4)]);
        let mut t = MockTransport::new().with_response(response);

        let state = read_status(&mut t, &d).expect("partial state must not error");

        assert_eq!(
            state.fields.get("battery_charge"),
            Some(&StatusValue::Percentage(50)),
            "battery_charge must be present from the single frame"
        );
        // Fields from other headers are absent -- that's the partial-snapshot contract.
        assert!(!state.fields.contains_key("anc_mode"));
        assert!(!state.fields.contains_key("media_mix"));
    }

    /// A raw value below the Percentage min must clamp to 0%.
    #[test]
    fn percentage_below_min_clamps_to_zero() {
        let d = parse_descriptor(
            r#"
            name = "Test"
            vendor_id = 0x1038
            product_ids = [0x0001]
            interface = 4
            report_id = 0x06
            report_length = 64
            capabilities = []

            [status]
            request = [0xb0]

            [[status.fields]]
            name = "level"
            match_prefix = [0x06, 0xb0]
            offset = 3
            parser = { type = "percentage", min = 2, max = 8 }
        "#,
        )
        .expect("parse");

        // raw value 1 is strictly below min=2 -> must clamp to 0%
        let f = frame(&[0x06, 0xb0], &[(3, 1)]);
        let state = decode_frame(&d, &f);
        assert_eq!(
            state.fields.get("level"),
            Some(&StatusValue::Percentage(0)),
            "raw value below min should clamp to 0%"
        );
    }

    /// An Enum field whose raw value is not in the entries list falls back to Int.
    #[test]
    fn enum_unknown_value_falls_back_to_int() {
        let d = parse_descriptor(
            r#"
            name = "Test"
            vendor_id = 0x1038
            product_ids = [0x0002]
            interface = 4
            report_id = 0x06
            report_length = 64
            capabilities = []

            [status]
            request = [0xb0]

            [[status.fields]]
            name = "mode"
            match_prefix = [0x06, 0xb0]
            offset = 2
            parser = { type = "enum", entries = [
                { value = 0, label = "off" },
                { value = 1, label = "on" },
            ] }
        "#,
        )
        .expect("parse");

        // raw value 99 is not in entries -> falls back to Int(99)
        let f = frame(&[0x06, 0xb0], &[(2, 99)]);
        let state = decode_frame(&d, &f);
        assert_eq!(
            state.fields.get("mode"),
            Some(&StatusValue::Int(99)),
            "unknown enum value should fall back to StatusValue::Int"
        );
    }

    /// A frame shorter than a field's offset must not panic; that field is simply absent.
    #[test]
    fn short_frame_skips_out_of_bounds_field_without_panic() {
        let d = parse_descriptor(
            r#"
            name = "Test"
            vendor_id = 0x1038
            product_ids = [0x0003]
            interface = 4
            report_id = 0x06
            report_length = 64
            capabilities = []

            [status]
            request = [0xb0]

            [[status.fields]]
            name = "far_field"
            match_prefix = []
            offset = 6
            parser = { type = "int" }
        "#,
        )
        .expect("parse");

        // Frame is only 2 bytes; field offset is 6 -> should be silently skipped
        let short = vec![0xAA, 0xBB];
        let state = decode_frame(&d, &short);
        assert!(
            !state.fields.contains_key("far_field"),
            "field beyond frame length must be absent, not panicked"
        );
    }
}
