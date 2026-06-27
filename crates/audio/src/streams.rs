//! Pure parsing of `pw-dump` JSON into application output streams and their
//! current sink. Subprocess-driven discovery lives in the engine; this file is
//! pure (string in, data out) so it is unit-testable without PipeWire.

use serde::{Deserialize, Serialize};

use crate::error::AudioError;

/// One running application output stream, as parsed from `pw-dump`.
/// `sink_node_name` is the `node.name` of the sink it is currently linked to,
/// or `None` if it is not linked to any sink yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedStream {
    pub id: u32,
    pub binary: String,
    pub app_name: String,
    pub pid: Option<u32>,
    pub icon_name: Option<String>,
    pub media_name: Option<String>,
    pub sink_node_name: Option<String>,
}

/// Parse `pw-dump` JSON into the list of application output streams, each with
/// its currently-linked sink `node.name` (resolved via Link objects).
///
/// Includes pulse-compat streams (`client.api == "pipewire-pulse"`) — skipping
/// them would hide most real apps (Chrome, Spotify, Discord).
pub fn parse_app_streams(pw_dump_json: &str) -> Result<Vec<ParsedStream>, AudioError> {
    let array: serde_json::Value =
        serde_json::from_str(pw_dump_json).map_err(|e| AudioError::Parse {
            what: "pw-dump JSON".to_string(),
            detail: e.to_string(),
        })?;
    let objects = array.as_array().ok_or_else(|| AudioError::Parse {
        what: "pw-dump JSON".to_string(),
        detail: "expected a top-level JSON array".to_string(),
    })?;

    // node id -> node.name for every sink (media.class == "Audio/Sink").
    let mut sink_names: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    // stream node id -> sink node id (first link wins; dedupe is implicit).
    let mut stream_to_sink: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut streams: Vec<ParsedStream> = Vec::new();

    for obj in objects {
        let ty = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if ty == "PipeWire:Interface:Link" {
            let info = match obj.get("info") {
                Some(i) => i,
                None => continue,
            };
            let out = info.get("output-node-id").and_then(|v| v.as_u64());
            let inp = info.get("input-node-id").and_then(|v| v.as_u64());
            if let (Some(o), Some(i)) = (out, inp) {
                stream_to_sink.entry(o as u32).or_insert(i as u32);
            }
            continue;
        }
        if ty != "PipeWire:Interface:Node" {
            continue;
        }
        let props = match obj.get("info").and_then(|i| i.get("props")) {
            Some(p) => p,
            None => continue,
        };
        let media_class = props.get("media.class").and_then(|v| v.as_str()).unwrap_or("");
        let id = match obj.get("id").and_then(|v| v.as_u64()) {
            Some(v) => v as u32,
            None => continue,
        };
        if media_class == "Audio/Sink" {
            if let Some(name) = props.get("node.name").and_then(|v| v.as_str()) {
                sink_names.insert(id, name.to_string());
            }
            continue;
        }
        if !media_class.starts_with("Stream/Output/Audio") {
            continue;
        }
        // Identify the app. Require a binary (fallback to node.name) so anonymous
        // streams without any identity are skipped.
        let binary = props
            .get("application.process.binary")
            .and_then(|v| v.as_str())
            .or_else(|| props.get("node.name").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        if binary.is_empty() {
            continue;
        }
        let app_name = props
            .get("application.name")
            .and_then(|v| v.as_str())
            .unwrap_or(&binary)
            .to_string();
        let pid = props
            .get("application.process.id")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<u32>().ok());
        let icon_name = props
            .get("application.icon-name")
            .and_then(|v| v.as_str())
            .map(String::from);
        let media_name = props
            .get("media.name")
            .and_then(|v| v.as_str())
            .map(String::from);
        streams.push(ParsedStream {
            id,
            binary,
            app_name,
            pid,
            icon_name,
            media_name,
            sink_node_name: None,
        });
    }

    // Second pass: attach the resolved sink name.
    for s in &mut streams {
        if let Some(sink_id) = stream_to_sink.get(&s.id) {
            s.sink_node_name = sink_names.get(sink_id).cloned();
        }
    }
    Ok(streams)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = include_str!("../tests/fixtures/pw_dump_app_streams.json");

    #[test]
    fn parses_native_and_pulse_streams_only() {
        let streams = parse_app_streams(DUMP).unwrap();
        // firefox (native) + spotify (pulse-compat). OBS is Stream/Input → excluded.
        let bins: Vec<&str> = streams.iter().map(|s| s.binary.as_str()).collect();
        assert!(bins.contains(&"firefox"), "native stream missing: {bins:?}");
        assert!(
            bins.contains(&"spotify"),
            "pulse-compat stream MUST be included (Chrome/Spotify/Discord use it): {bins:?}"
        );
        assert!(!bins.contains(&"obs"), "input stream must be excluded");
        assert_eq!(streams.len(), 2);
    }

    #[test]
    fn resolves_current_sink_via_links_deduped() {
        let streams = parse_app_streams(DUMP).unwrap();
        let ff = streams.iter().find(|s| s.binary == "firefox").unwrap();
        // Two links to the same sink → resolves once to Arctis_Game.
        assert_eq!(ff.sink_node_name.as_deref(), Some("Arctis_Game"));
    }

    #[test]
    fn unlinked_stream_has_no_sink() {
        let streams = parse_app_streams(DUMP).unwrap();
        let sp = streams.iter().find(|s| s.binary == "spotify").unwrap();
        assert_eq!(sp.sink_node_name, None);
    }

    #[test]
    fn fields_are_populated() {
        let streams = parse_app_streams(DUMP).unwrap();
        let ff = streams.iter().find(|s| s.binary == "firefox").unwrap();
        assert_eq!(ff.id, 70);
        assert_eq!(ff.app_name, "Firefox");
        assert_eq!(ff.pid, Some(1234));
        assert_eq!(ff.icon_name.as_deref(), Some("firefox"));
        assert_eq!(ff.media_name.as_deref(), Some("YouTube"));
    }

    #[test]
    fn malformed_json_is_parse_error() {
        let err = parse_app_streams("not json").unwrap_err();
        assert!(matches!(err, AudioError::Parse { .. }));
    }
}
