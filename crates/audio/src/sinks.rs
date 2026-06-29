//! Pure parsing of `pw-dump` JSON into real output sinks (excluding virtual sinks
//! and filter-chain infrastructure). Subprocess-driven discovery lives in the engine;
//! this file is pure (string in, data out) so it is unit-testable without PipeWire.

use serde::{Deserialize, Serialize};

use crate::error::AudioError;

/// One audio output sink from PipeWire, as parsed from `pw-dump`.
/// Excludes virtual sinks (e.g., `Arctis_Game`) and filter-chain infrastructure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSink {
    pub node_name: String,
    pub description: String,
    pub is_default: bool,
}

/// Extract the default audio sink name from `pw-metadata 0` stdout.
///
/// Scans for a line of the form:
/// `update: id:0 key:'default.audio.sink' value:'{"name":"..."}' type:...`
/// and returns the inner `name` string.  Returns `None` if the key is absent,
/// the line is malformed, or the embedded JSON cannot be parsed.
pub fn parse_default_sink_name(pw_metadata_stdout: &str) -> Option<String> {
    for line in pw_metadata_stdout.lines() {
        if !line.contains("key:'default.audio.sink'") {
            continue;
        }
        // Locate value:'<json>' in the line.
        let value_start = line.find("value:'")?;
        let rest = &line[value_start + 7..]; // skip `value:'`
        let value_end = rest.find('\'')?;
        let value_json = &rest[..value_end];
        // The embedded value is a JSON object like {"name":"<node.name>"}.
        let v: serde_json::Value = serde_json::from_str(value_json).ok()?;
        return v.get("name").and_then(|n| n.as_str()).map(|s| s.to_string());
    }
    None
}

/// Parse `pw-dump` JSON into the list of real output sinks, excluding virtual
/// sinks and filter-chain nodes.
///
/// Selection rules:
/// - Include only `media.class == "Audio/Sink"`
/// - Exclude if `node.name` starts with `"Arctis_"`
/// - Exclude if `node.link-group` (fallback `node.group`) starts with `"filter-chain-"`
/// - `description` = `node.description` else `device.description` else `node.name`
/// - `is_default` = `node_name == default_sink_name`
pub fn parse_output_sinks(
    pw_dump_json: &str,
    default_sink_name: Option<&str>,
) -> Result<Vec<OutputSink>, AudioError> {
    let array: serde_json::Value =
        serde_json::from_str(pw_dump_json).map_err(|e| AudioError::Parse {
            what: "pw-dump JSON".to_string(),
            detail: e.to_string(),
        })?;
    let objects = array.as_array().ok_or_else(|| AudioError::Parse {
        what: "pw-dump JSON".to_string(),
        detail: "expected a top-level JSON array".to_string(),
    })?;

    let mut sinks: Vec<OutputSink> = Vec::new();

    for obj in objects {
        let ty = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if ty != "PipeWire:Interface:Node" {
            continue;
        }

        let props = match obj.get("info").and_then(|i| i.get("props")) {
            Some(p) => p,
            None => continue,
        };

        let media_class = props.get("media.class").and_then(|v| v.as_str()).unwrap_or("");
        if media_class != "Audio/Sink" {
            continue;
        }

        let node_name = match props.get("node.name").and_then(|v| v.as_str()) {
            Some(n) => n,
            None => continue,
        };

        // Exclude virtual sinks starting with "Arctis_"
        if node_name.starts_with("Arctis_") {
            continue;
        }

        // Exclude filter-chain infrastructure nodes.
        let link_group = props
            .get("node.link-group")
            .and_then(|v| v.as_str())
            .or_else(|| props.get("node.group").and_then(|v| v.as_str()))
            .unwrap_or("");
        if link_group.starts_with("filter-chain-") {
            continue;
        }

        // Build description: prefer node.description, fallback to device.description, then node.name
        let description = props
            .get("node.description")
            .and_then(|v| v.as_str())
            .or_else(|| props.get("device.description").and_then(|v| v.as_str()))
            .unwrap_or(node_name)
            .to_string();

        let is_default = default_sink_name.map(|d| d == node_name).unwrap_or(false);

        sinks.push(OutputSink {
            node_name: node_name.to_string(),
            description,
            is_default,
        });
    }

    Ok(sinks)
}

/// Parse the negotiated channel count for a playback stream from `pw-dump` JSON.
///
/// Iterates `PipeWire:Interface:Node` entries with `media.class == "Stream/Output/Audio"`.
/// A node matches when `matcher` (case-insensitive) is a substring of either
/// `info.props["application.name"]` or `info.props["node.name"]`.
///
/// For the first matching node the channel count is read from:
/// 1. `info.props["audio.channels"]` — as a JSON number or a numeric string.
/// 2. Fallback: `info.params.Format[0].channels` — as a JSON number.
///
/// Returns `None` if no node matches, no readable channel count is found, or the
/// JSON is malformed.  Never panics.
pub fn parse_stream_channels(dump: &str, matcher: &str) -> Option<u8> {
    let array: serde_json::Value = serde_json::from_str(dump).ok()?;
    let objects = array.as_array()?;
    let matcher_lower = matcher.to_lowercase();

    for obj in objects {
        let ty = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if ty != "PipeWire:Interface:Node" {
            continue;
        }

        let info = match obj.get("info") {
            Some(i) => i,
            None => continue,
        };
        let props = match info.get("props") {
            Some(p) => p,
            None => continue,
        };

        let media_class = props.get("media.class").and_then(|v| v.as_str()).unwrap_or("");
        if media_class != "Stream/Output/Audio" {
            continue;
        }

        // Match on application.name or node.name (case-insensitive substring).
        let app_name = props
            .get("application.name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let node_name_v = props
            .get("node.name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !app_name.to_lowercase().contains(&matcher_lower)
            && !node_name_v.to_lowercase().contains(&matcher_lower)
        {
            continue;
        }

        // 1. Try info.props["audio.channels"]: JSON number or numeric string.
        if let Some(ch) = props.get("audio.channels").and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        }) {
            return Some(ch as u8);
        }

        // 2. Fallback: info.params.Format[0].channels.
        return info
            .get("params")
            .and_then(|p| p.get("Format"))
            .and_then(|f| f.as_array())
            .and_then(|a| a.first())
            .and_then(|entry| entry.get("channels"))
            .and_then(|c| c.as_u64())
            .map(|ch| ch as u8);
    }
    None
}

/// Parse the live volume (0–100 percent) for a named node from `pw-dump` JSON.
///
/// Looks for a `PipeWire:Interface:Node` whose `info.props.node.name` matches
/// `node_name`, then reads the first entry of `info.params.Props[0].channelVolumes`
/// (falling back to `info.params.Props[0].volume`) and converts the raw linear
/// value to a 0–100 PERCEPTUAL percent integer via the inverse of the cubic write:
/// pct = cbrt(channelVolumes) * 100 (matches wpctl/PipeWire/pavucontrol).
///
/// Returns `None` if the node is absent, the JSON is malformed, or no volume
/// field is present. Callers should fall back to the config's `volume_pct`.
pub fn parse_node_volume(pw_dump_json: &str, node_name: &str) -> Option<u8> {
    let array: serde_json::Value = serde_json::from_str(pw_dump_json).ok()?;
    let objects = array.as_array()?;
    for obj in objects {
        let ty = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if ty != "PipeWire:Interface:Node" {
            continue;
        }
        let info = obj.get("info")?;
        let props = info.get("props")?;
        let name = props
            .get("node.name")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if name != node_name {
            continue;
        }
        let params = info.get("params")?;
        let props_arr = params.get("Props")?.as_array()?;
        let first = props_arr.first()?;
        let linear = first
            .get("channelVolumes")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_f64())
            .or_else(|| first.get("volume").and_then(|v| v.as_f64()))?;
        // Inverse of the cubic write: perceptual pct = cbrt(channelVolumes) * 100.
        let pct = ((linear as f32).cbrt() * 100.0).round().clamp(0.0, 100.0) as u8;
        return Some(pct);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = include_str!("../tests/fixtures/pw_dump_sinks.json");

    // ── TDD: parse_default_sink_name ──────────────────────────────────────────

    #[test]
    fn parse_default_sink_name_extracts_name() {
        let input = concat!(
            "update: id:0 key:'default.audio.sink' ",
            r#"value:'{"name":"alsa_output.pci-0000_00_1f.3.analog-stereo"}' type:Spa:String"#
        );
        let name = parse_default_sink_name(input);
        assert_eq!(
            name.as_deref(),
            Some("alsa_output.pci-0000_00_1f.3.analog-stereo")
        );
    }

    #[test]
    fn parse_default_sink_name_returns_none_when_key_absent() {
        let input = concat!(
            "update: id:0 key:'some.other.key' ",
            r#"value:'{"name":"something"}' type:Spa:String"#
        );
        assert!(parse_default_sink_name(input).is_none());
    }

    #[test]
    fn lists_real_sinks_excludes_virtual() {
        let s =
            parse_output_sinks(DUMP, Some("alsa_output.pci-0000_00_1f.3.analog-stereo")).unwrap();
        let names: Vec<&str> = s.iter().map(|x| x.node_name.as_str()).collect();
        assert!(
            names.iter().any(|n| n.contains("SteelSeries_Arctis")),
            "headset sink missing: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.starts_with("Arctis_")),
            "virtual sinks excluded: {names:?}"
        );
        assert!(
            s.iter()
                .find(|x| x.node_name.contains("analog-stereo"))
                .unwrap()
                .is_default,
            "onboard analog-stereo should be marked default"
        );
    }

    // ── TDD: parse_node_volume ────────────────────────────────────────────────

    #[test]
    fn parse_node_volume_extracts_from_channel_volumes() {
        let dump = include_str!("../tests/fixtures/pw_dump_volumes.json");
        assert_eq!(parse_node_volume(dump, "Arctis_Game"), Some(50));
        assert_eq!(parse_node_volume(dump, "Arctis_Chat"), Some(100));
        assert_eq!(parse_node_volume(dump, "Arctis_Missing"), None);
    }

    #[test]
    fn parse_node_volume_returns_none_on_invalid_json() {
        assert_eq!(parse_node_volume("not json", "any"), None);
        assert_eq!(parse_node_volume("", "any"), None);
    }

    // ── TDD: parse_stream_channels ────────────────────────────────────────────

    /// Helper: wrap a single node object in a top-level array string.
    fn stream_node(props_extra: &str, params_extra: &str) -> String {
        format!(
            r#"[{{"type":"PipeWire:Interface:Node","info":{{"props":{{"media.class":"Stream/Output/Audio"{props_extra}}},"params":{{{params_extra}}}}}}}]"#
        )
    }

    #[test]
    fn parse_stream_channels_from_props_audio_channels() {
        // audio.channels as a JSON number (8 = 7.1)
        let dump = stream_node(
            r#","application.name":"DayZ","audio.channels":8"#,
            "",
        );
        assert_eq!(parse_stream_channels(&dump, "dayz"), Some(8));
    }

    #[test]
    fn parse_stream_channels_from_format_param_fallback() {
        // No audio.channels prop; channel count lives in params.Format[0].channels
        let dump = stream_node(
            r#","application.name":"DayZ""#,
            r#""Format":[{"channels":2,"rate":48000}]"#,
        );
        assert_eq!(parse_stream_channels(&dump, "dayz"), Some(2));
    }

    #[test]
    fn parse_stream_channels_numeric_string_prop() {
        // audio.channels serialised as a JSON string "6" (5.1)
        let dump = stream_node(
            r#","application.name":"Surround","audio.channels":"6""#,
            "",
        );
        assert_eq!(parse_stream_channels(&dump, "surround"), Some(6));
    }

    #[test]
    fn parse_stream_channels_no_match_returns_none() {
        let dump = stream_node(
            r#","application.name":"Spotify","audio.channels":2"#,
            "",
        );
        assert_eq!(parse_stream_channels(&dump, "dayz"), None);
    }

    #[test]
    fn parse_stream_channels_malformed_json_returns_none() {
        assert_eq!(parse_stream_channels("not json", "x"), None);
        assert_eq!(parse_stream_channels("", "x"), None);
    }

    #[test]
    fn parse_stream_channels_matches_on_node_name_too() {
        // application.name absent; match via node.name
        let dump = stream_node(
            r#","node.name":"DayZ_proton_stream","audio.channels":8"#,
            "",
        );
        assert_eq!(parse_stream_channels(&dump, "dayz"), Some(8));
        assert_eq!(parse_stream_channels(&dump, "spotify"), None);
    }
}
