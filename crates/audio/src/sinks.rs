//! Pure parsing of `pw-dump` JSON into real output sinks (excluding virtual sinks
//! and filter-chain infrastructure). Subprocess-driven discovery lives in the engine;
//! this file is pure (string in, data out) so it is unit-testable without PipeWire.

use serde::{Deserialize, Serialize};

use crate::error::AudioError;

/// One audio output sink from PipeWire, as parsed from `pw-dump`.
/// Excludes virtual sinks (e.g., `Arctis_Game`) and filter-chain infrastructure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSink {
    /// PipeWire object id (ephemeral, but valid for immediate `wpctl <id>` use).
    pub id: u32,
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

        // Object id — needed for `wpctl set-volume/set-mute <id>` (wpctl does
        // not accept node names). Nodes without an id are unusable; skip them.
        let id = match obj.get("id").and_then(|v| v.as_u64()) {
            Some(i) => i as u32,
            None => continue,
        };

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
            id,
            node_name: node_name.to_string(),
            description,
            is_default,
        });
    }

    Ok(sinks)
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
        // Object ids are surfaced (needed for wpctl targeting).
        assert_eq!(
            s.iter()
                .find(|x| x.node_name.contains("SteelSeries_Arctis"))
                .unwrap()
                .id,
            10,
            "headset sink id must come from the pw-dump object id"
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

}
