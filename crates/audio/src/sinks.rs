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

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = include_str!("../tests/fixtures/pw_dump_sinks.json");

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
}
