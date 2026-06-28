use crate::descriptor::{DeviceDescriptor, Parser, StatusField, ValueEncoding};
use crate::error::DeviceError;
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
        Parser::Percentage { min, max, invert } => {
            let span = max.saturating_sub(*min).max(1) as u32;
            let clamped = raw.clamp(*min, *max);
            let mut pct = (clamped.saturating_sub(*min) as u32 * 100 / span) as u8;
            if *invert {
                pct = 100 - pct;
            }
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

/// Per-frame read timeout in milliseconds for `read_status`.
///
/// The `[0x06,0xb0]` status reply and any queued `[0x07,0x45]` dial frames
/// arrive fast (firmware queues them immediately after the request). 80 ms is
/// sufficient to drain all queued frames; 500 ms was pure dead wait. Keeping a
/// generous-but-not-excessive value avoids dropping the battery reply under
/// heavier system load.
///
/// NOTE: verify during live testing that battery_charge is still reported after
/// this reduction. If battery frames arrive late on real hardware, bump back to
/// 150–200 ms.
const STATUS_READ_TIMEOUT_MS: i32 = 80;

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
        match transport.read_report(&mut buf, STATUS_READ_TIMEOUT_MS) {
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

/// Encode a single write command into a fully-formed, zero-padded report.
///
/// `value` is the wire value: for `IntRange` it is the user integer (clamped to
/// `[min, max]`); for `Enum` it is the numeric wire byte (must match an entry).
///
/// SAFETY: builds exactly ONE report — `report_id + opcode + one value byte`,
/// padded to `report_length`. No init bytes, no extra opcodes.
pub fn encode_command(
    desc: &DeviceDescriptor,
    name: &str,
    value: i64,
) -> Result<Vec<u8>, DeviceError> {
    let spec = desc
        .commands
        .get(name)
        .ok_or_else(|| DeviceError::Unsupported(name.to_string()))?;

    let wire_value: u8 = match &spec.encoding {
        ValueEncoding::IntRange { min, max } => {
            let min = i64::from(*min);
            let max = i64::from(*max);
            if value < min || value > max {
                return Err(DeviceError::InvalidValue {
                    cmd: name.to_string(),
                    detail: format!("{value} out of range [{min}, {max}]"),
                });
            }
            value as u8
        }
        ValueEncoding::Enum { entries } => {
            let v = u8::try_from(value).map_err(|_| DeviceError::InvalidValue {
                cmd: name.to_string(),
                detail: format!("{value} out of byte range"),
            })?;
            if !entries.iter().any(|e| e.value == v) {
                return Err(DeviceError::InvalidValue {
                    cmd: name.to_string(),
                    detail: format!("{v} is not a valid choice"),
                });
            }
            v
        }
    };

    let mut report = Vec::with_capacity(desc.report_length);
    report.push(desc.report_id);
    report.extend_from_slice(&spec.opcode);
    report.push(wire_value);
    if report.len() > desc.report_length {
        return Err(DeviceError::InvalidValue {
            cmd: name.to_string(),
            detail: "opcode longer than report_length".into(),
        });
    }
    report.resize(desc.report_length, 0);
    Ok(report)
}

/// Build the save/commit report from `desc.save_command` (padded to report_length).
fn encode_save(desc: &DeviceDescriptor) -> Option<Vec<u8>> {
    let save = desc.save_command.as_ref()?;
    let mut report = Vec::with_capacity(desc.report_length);
    report.push(desc.report_id);
    report.extend_from_slice(save);
    report.resize(desc.report_length, 0);
    Some(report)
}

/// Send exactly one command report (and at most one save report when `spec.save` is true).
///
/// SAFETY: only the encoded command report and the optional save report are written —
/// never a burst. Every transport error is surfaced as [`DeviceError`].
pub fn write_command<T: Transport>(
    transport: &mut T,
    desc: &DeviceDescriptor,
    name: &str,
    value: i64,
) -> Result<(), DeviceError> {
    // encode_command already validates the command name; the lookup below is a
    // defensive fallback that returns a typed error rather than panicking.
    let report = encode_command(desc, name, value)?;
    transport.write_report(&report)?;

    let spec = desc
        .commands
        .get(name)
        .ok_or_else(|| DeviceError::Unsupported(name.to_string()))?;
    if spec.save {
        if let Some(save_report) = encode_save(desc) {
            transport.write_report(&save_report)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::parse_descriptor;
    use crate::error::DeviceError;
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

    /// An inverted Percentage parser maps raw max→0%, raw min→100%, midpoint→~50%.
    #[test]
    fn percentage_invert_maps_high_raw_to_low_pct() {
        let d = parse_descriptor(
            r#"
            name = "Test"
            vendor_id = 0x1038
            product_ids = [0x0004]
            interface = 4
            report_id = 0x06
            report_length = 64
            capabilities = []

            [status]
            request = [0xb0]

            [[status.fields]]
            name = "knob"
            match_prefix = [0x07, 0x25]
            offset = 2
            parser = { type = "percentage", min = 0, max = 56, invert = true }
        "#,
        )
        .expect("parse");

        // raw 0 (min) -> 100%, raw 56 (max) -> 0%, raw 28 (mid) -> ~50%.
        let at = |raw: u8| {
            let f = frame(&[0x07, 0x25], &[(2, raw)]);
            decode_frame(&d, &f).fields.get("knob").cloned()
        };
        assert_eq!(at(0), Some(StatusValue::Percentage(100)), "raw 0 -> 100%");
        assert_eq!(at(56), Some(StatusValue::Percentage(0)), "raw 56 -> 0%");
        match at(28) {
            Some(StatusValue::Percentage(p)) => {
                assert!((49..=51).contains(&p), "raw 28 -> ~50%, got {p}");
            }
            other => panic!("expected Percentage, got {other:?}"),
        }
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

    // ── encoder tests (Task 3) ────────────────────────────────────────────────

    #[test]
    fn encode_command_builds_padded_report_with_report_id_opcode_value() {
        let d = nova();
        // sidetone level 2 -> [0x06, 0x39, 0x02, 0,0,...] len 64
        let report = encode_command(&d, "sidetone", 2).expect("encode");
        assert_eq!(report.len(), d.report_length);
        assert_eq!(report[0], 0x06, "report_id first");
        assert_eq!(report[1], 0x39, "opcode");
        assert_eq!(report[2], 0x02, "encoded value");
        assert!(report[3..].iter().all(|&b| b == 0), "rest zero-padded");
    }

    #[test]
    fn encode_command_int_range_rejects_out_of_range() {
        let d = nova();
        // mic_volume range 1..10; request 11 (above max) -> error, no bytes sent
        let err = encode_command(&d, "mic_volume", 11).unwrap_err();
        assert!(
            matches!(err, DeviceError::InvalidValue { .. }),
            "above-max must return InvalidValue, got {err:?}"
        );

        // mic_volume range 1..10; request 0 (below min) -> error
        let err = encode_command(&d, "mic_volume", 0).unwrap_err();
        assert!(
            matches!(err, DeviceError::InvalidValue { .. }),
            "below-min must return InvalidValue, got {err:?}"
        );
    }

    #[test]
    fn write_command_out_of_range_writes_no_bytes() {
        let d = nova();
        let mut t = MockTransport::new();
        // mic_volume range 1..10; request 11 -> must error and write nothing
        let err = write_command(&mut t, &d, "mic_volume", 11).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidValue { .. }));
        assert!(
            t.written.is_empty(),
            "no bytes must be written for out-of-range value"
        );
    }

    #[test]
    fn encode_command_int_range_in_range_succeeds() {
        let d = nova();
        // sidetone max is 3; value 3 (at boundary) -> succeeds
        let report = encode_command(&d, "sidetone", 3).expect("encode");
        assert_eq!(report[2], 3, "max boundary value must be encoded as-is");

        // sidetone min is 0; value 0 -> succeeds
        let report = encode_command(&d, "sidetone", 0).expect("encode");
        assert_eq!(report[2], 0, "min boundary value must be encoded as-is");
    }

    #[test]
    fn encode_command_enum_maps_wire_value() {
        let d = nova();
        // anc enum: value 1 == transparency. encode_command takes the wire value (i64).
        let report = encode_command(&d, "anc", 1).expect("encode");
        assert_eq!(report[1], 0xbd);
        assert_eq!(report[2], 1);
    }

    #[test]
    fn encode_command_enum_unknown_wire_value_returns_error() {
        let d = nova();
        // anc only has values 0/1/2; 99 is unknown
        let err = encode_command(&d, "anc", 99).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidValue { .. }));
    }

    #[test]
    fn encode_command_unknown_command_returns_unsupported_error() {
        let d = nova();
        let err = encode_command(&d, "oled", 0).unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported(_)));
    }

    #[test]
    fn encode_command_mic_volume_int_range() {
        let d = nova();
        // mic_volume range 1..10; value 5 -> [0x06, 0x37, 0x05, 0,...]
        let report = encode_command(&d, "mic_volume", 5).expect("encode");
        assert_eq!(report.len(), 64);
        assert_eq!(report[0], 0x06);
        assert_eq!(report[1], 0x37, "mic_volume opcode");
        assert_eq!(report[2], 5);
        assert!(report[3..].iter().all(|&b| b == 0));
    }

    #[test]
    fn encode_command_inactive_time_enum_10min() {
        let d = nova();
        // inactive_time enum: value 3 == "10min" -> opcode 0xc1, wire byte 3
        let report = encode_command(&d, "inactive_time", 3).expect("encode");
        assert_eq!(report.len(), 64);
        assert_eq!(report[0], 0x06);
        assert_eq!(report[1], 0xc1, "inactive_time opcode");
        assert_eq!(report[2], 3, "10min wire value");
        assert!(report[3..].iter().all(|&b| b == 0));
    }

    #[test]
    fn write_command_sends_exactly_one_report_then_save() {
        let d = nova();
        let mut t = MockTransport::new();
        // sidetone has save = true -> expect 2 writes: the command, then save.
        write_command(&mut t, &d, "sidetone", 1).expect("write");
        assert_eq!(t.written.len(), 2, "command + save = exactly two writes");
        assert_eq!(t.written[0][0], 0x06);
        assert_eq!(t.written[0][1], 0x39);
        assert_eq!(t.written[0][2], 1);
        // save report = [0x06, 0x09, 0, ...]
        assert_eq!(t.written[1][0], 0x06);
        assert_eq!(t.written[1][1], 0x09);
        assert!(t.written[1][2..].iter().all(|&b| b == 0));
        assert_eq!(t.written[1].len(), d.report_length);
    }

    #[test]
    fn write_command_no_save_sends_one_report() {
        // Build a descriptor with save=false (no save_command) to prove no extra write.
        let d = parse_descriptor(
            r#"
            name = "T"
            vendor_id = 0x1038
            product_ids = [0x12e5]
            interface = 4
            report_id = 0x06
            report_length = 64
            capabilities = ["sidetone"]

            [status]
            request = [0xb0]

            [commands.sidetone]
            opcode = [0x39]
            capability = "sidetone"
            encoding = { type = "int_range", min = 0, max = 3 }
        "#,
        )
        .unwrap();
        let mut t = MockTransport::new();
        write_command(&mut t, &d, "sidetone", 2).unwrap();
        assert_eq!(t.written.len(), 1, "no save -> exactly one write");
    }

    #[test]
    fn write_command_surfaces_transport_error() {
        // A transport that errors on write must propagate (never swallow).
        struct FailWrite;
        impl Transport for FailWrite {
            fn write_report(&mut self, _d: &[u8]) -> Result<(), TransportError> {
                Err(TransportError::Io("boom".into()))
            }
            fn read_report(&mut self, _b: &mut [u8], _t: i32) -> Result<usize, TransportError> {
                Err(TransportError::Timeout)
            }
        }
        let d = nova();
        let err = write_command(&mut FailWrite, &d, "sidetone", 1).unwrap_err();
        assert!(matches!(err, DeviceError::Transport(_)));
    }
}
