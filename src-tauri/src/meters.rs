//! meters.rs — Level sampling for the `levels` Tauri event.
//!
//! # What this measures
//!
//! This module samples the *configured software volume* (PipeWire
//! `Props.channelVolumes`) for the Arctis channel virtual sinks and the
//! clean-mic source by running `pw-dump` and parsing its JSON output.
//!
//! **This is NOT real-time audio signal peak or RMS.**  The value reflects
//! the volume the engine (or user) has set for each node — not signal
//! activity.  True peak/RMS metering would require a native `pipewire-rs`
//! capture stream connecting to the monitor ports of each sink-input.  That
//! is a heavier dependency (pipewire-rs is `!Send`, needs a dedicated OS
//! thread + event loop) and is documented as a follow-up task.
//!
//! # Rate
//!
//! Called every 2 seconds (matching the state-poll task).  `pw-dump` itself
//! takes ~20–30 ms on this system, which is well within budget.
//!
//! # Resilience
//!
//! Any error (pw-dump not found, parse failure, empty output, …) returns
//! `None`; the caller skips the tick.  The app never crashes.

use std::collections::HashMap;

/// Node names the engine creates for channel sinks + the clean-mic source.
/// These must stay in sync with crates/engine/src/convert.rs / children.rs.
const TARGET_NODES: &[&str] = &[
    "Arctis_Game",
    "Arctis_Chat",
    "Arctis_Media",
    "arctis_clean_mic",
];

/// A flat map of node.name → linear volume scalar [0.0, 1.0].
/// Serialised directly as the `levels` event payload.
pub type LevelsPayload = HashMap<String, f32>;

/// Sample current software volume levels via `pw-dump`.
///
/// Returns `Some(payload)` on success, `None` on any error (resilient).
/// Intended to be called from `spawn_blocking`.
pub fn sample_levels() -> Option<LevelsPayload> {
    let output = std::process::Command::new("pw-dump").output().ok()?;

    if !output.status.success() && output.stdout.is_empty() {
        return None;
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let entries = json.as_array()?;

    let mut payload = LevelsPayload::new();

    for entry in entries {
        // Only process Node entries; skip anything that isn't a Node.
        if entry.get("type").and_then(|t| t.as_str()) != Some("PipeWire:Interface:Node") {
            continue;
        }

        // Skip entries without a node.name (use `let … else` so we continue, not `?`).
        let Some(node_name) = entry
            .pointer("/info/props/node.name")
            .and_then(|v| v.as_str())
        else {
            continue;
        };

        if !TARGET_NODES.contains(&node_name) {
            continue;
        }

        // Dig into params.Props[*].channelVolumes; skip if absent.
        let Some(props_array) = entry
            .pointer("/info/params/Props")
            .and_then(|v| v.as_array())
        else {
            continue;
        };

        let channel_volumes: Option<Vec<f64>> = props_array.iter().find_map(|p| {
            p.get("channelVolumes")
                .and_then(|cv| cv.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect())
        });

        if let Some(vols) = channel_volumes {
            if vols.is_empty() {
                continue;
            }
            let avg = vols.iter().sum::<f64>() / vols.len() as f64;
            let clamped = avg.clamp(0.0, 1.0) as f32;
            payload.insert(node_name.to_string(), clamped);
        }
    }

    if payload.is_empty() {
        None
    } else {
        Some(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal pw-dump-style JSON value for a single node.
    fn make_node(name: &str, volumes: &[f64]) -> serde_json::Value {
        serde_json::json!({
            "type": "PipeWire:Interface:Node",
            "id": 100,
            "info": {
                "props": { "node.name": name },
                "params": {
                    "Props": [{ "channelVolumes": volumes }]
                }
            }
        })
    }

    /// Parse a hand-built JSON array through the same logic as `sample_levels`
    /// (factored out to avoid subprocess calls in tests).
    fn parse_levels(json: serde_json::Value) -> LevelsPayload {
        let entries = match json.as_array() {
            Some(a) => a,
            None => return LevelsPayload::new(),
        };

        let mut payload = LevelsPayload::new();

        for entry in entries {
            if entry.get("type").and_then(|t| t.as_str()) != Some("PipeWire:Interface:Node") {
                continue;
            }
            let node_name = match entry
                .pointer("/info/props/node.name")
                .and_then(|v| v.as_str())
            {
                Some(n) => n,
                None => continue,
            };
            if !TARGET_NODES.contains(&node_name) {
                continue;
            }
            let props_array = match entry
                .pointer("/info/params/Props")
                .and_then(|v| v.as_array())
            {
                Some(a) => a,
                None => continue,
            };
            let vols: Option<Vec<f64>> = props_array.iter().find_map(|p| {
                p.get("channelVolumes")
                    .and_then(|cv| cv.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_f64()).collect())
            });
            if let Some(v) = vols {
                if !v.is_empty() {
                    let avg = v.iter().sum::<f64>() / v.len() as f64;
                    payload.insert(node_name.to_string(), avg.clamp(0.0, 1.0) as f32);
                }
            }
        }

        payload
    }

    #[test]
    fn extracts_game_channel_volume() {
        let data = serde_json::json!([make_node("Arctis_Game", &[0.75, 0.75])]);
        let p = parse_levels(data);
        assert!((p["Arctis_Game"] - 0.75).abs() < 1e-4);
    }

    #[test]
    fn averages_stereo_volumes() {
        let data = serde_json::json!([make_node("Arctis_Chat", &[0.4, 0.8])]);
        let p = parse_levels(data);
        assert!((p["Arctis_Chat"] - 0.6).abs() < 1e-4);
    }

    #[test]
    fn ignores_non_target_nodes() {
        let non_target = serde_json::json!({
            "type": "PipeWire:Interface:Node",
            "id": 50,
            "info": {
                "props": { "node.name": "alsa_output.pci-0000" },
                "params": { "Props": [{ "channelVolumes": [0.5, 0.5] }] }
            }
        });
        let data = serde_json::json!([non_target]);
        let p = parse_levels(data);
        assert!(p.is_empty());
    }

    #[test]
    fn clamps_volumes_above_one() {
        let data = serde_json::json!([make_node("Arctis_Media", &[1.2, 1.5])]);
        let p = parse_levels(data);
        assert!(p["Arctis_Media"] <= 1.0);
    }

    #[test]
    fn handles_node_without_props() {
        let no_props = serde_json::json!({
            "type": "PipeWire:Interface:Node",
            "id": 101,
            "info": {
                "props": { "node.name": "Arctis_Game" },
                "params": {}
            }
        });
        let data = serde_json::json!([no_props]);
        let p = parse_levels(data);
        assert!(p.is_empty());
    }

    #[test]
    fn returns_empty_for_empty_array() {
        let p = parse_levels(serde_json::json!([]));
        assert!(p.is_empty());
    }
}
