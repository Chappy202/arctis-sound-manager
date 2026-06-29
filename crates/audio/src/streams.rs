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

/// Always-on system/infrastructure streams that should never appear in the app
/// list (matched case-insensitively against the binary OR the application name).
/// Extend this as more system noise turns up.
const HIDDEN_SYSTEM_STREAMS: &[&str] = &["speech-dispatcher-dummy", "speech-dispatcher"];

/// Generic shell names that Electron/Chromium apps report in `application.name`
/// (e.g. Discord, Slack, VS Code). For these the process binary is the better
/// display name.
const GENERIC_APP_NAMES: &[&str] = &["chromium", "chrome", "electron"];

/// True if a stream is system noise that should be hidden from the mixer.
fn is_hidden_system_stream(binary: &str, app_name: &str) -> bool {
    HIDDEN_SYSTEM_STREAMS
        .iter()
        .any(|d| binary.eq_ignore_ascii_case(d) || app_name.eq_ignore_ascii_case(d))
}

/// Capitalize the first character of a single word ("discord" → "Discord");
/// leaves an already-capitalized word unchanged.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Resolve a human-friendly app name. When `application.name` is a generic
/// Electron/Chromium shell value, prefer the process binary (title-cased), then
/// the media name; otherwise use `application.name` as-is.
fn resolve_app_name(app_name: &str, binary: &str, media_name: Option<&str>) -> String {
    let is_generic =
        |s: &str| GENERIC_APP_NAMES.contains(&s.trim().to_ascii_lowercase().as_str());
    if is_generic(app_name) {
        if !binary.trim().is_empty() && !is_generic(binary) {
            return capitalize_first(binary.trim());
        }
        if let Some(m) = media_name {
            if !m.trim().is_empty() {
                return m.trim().to_string();
            }
        }
    }
    app_name.to_string()
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
        // Exclude filter-chain infrastructure nodes.
        let link_group = props
            .get("node.link-group")
            .and_then(|v| v.as_str())
            .or_else(|| props.get("node.group").and_then(|v| v.as_str()))
            .unwrap_or("");
        if link_group.starts_with("filter-chain-") {
            continue;
        }
        // Exclude nodes with no application identity (neither binary nor app name).
        let has_app_binary = props
            .get("application.process.binary")
            .and_then(|v| v.as_str())
            .is_some();
        let has_app_name = props.get("application.name").and_then(|v| v.as_str()).is_some();
        if !has_app_binary && !has_app_name {
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
        let raw_app_name = props
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
        // Hide always-on system/infrastructure streams (e.g. speech-dispatcher).
        if is_hidden_system_stream(&binary, &raw_app_name) {
            continue;
        }
        // Electron/Chromium apps report a generic application.name ("Chromium");
        // prefer the process binary so they show their real name (Discord, …).
        let app_name = resolve_app_name(&raw_app_name, &binary, media_name.as_deref());
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

    // --- system-stream filtering + Electron app naming --------------------

    /// Minimal one-node Stream/Output/Audio pw-dump with the given props.
    fn one_stream_dump(props_json: &str) -> String {
        format!(
            r#"[{{"id":50,"type":"PipeWire:Interface:Node","info":{{"props":{{"media.class":"Stream/Output/Audio",{props_json}}}}}}}]"#
        )
    }

    #[test]
    fn hides_speech_dispatcher_dummy_stream() {
        let dump = one_stream_dump(
            r#""application.name":"speech-dispatcher-dummy","application.process.binary":"speech-dispatcher-dummy""#,
        );
        let streams = parse_app_streams(&dump).unwrap();
        assert!(streams.is_empty(), "speech-dispatcher must be hidden: {streams:?}");
    }

    #[test]
    fn electron_chromium_app_uses_binary_name() {
        // Discord reports application.name="Chromium" but binary="Discord".
        let dump = one_stream_dump(
            r#""application.name":"Chromium","application.process.binary":"Discord""#,
        );
        let streams = parse_app_streams(&dump).unwrap();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].app_name, "Discord");
        assert_eq!(streams[0].binary, "Discord");
    }

    #[test]
    fn lowercase_binary_is_capitalized_for_generic_name() {
        let dump = one_stream_dump(
            r#""application.name":"Chromium","application.process.binary":"slack""#,
        );
        let streams = parse_app_streams(&dump).unwrap();
        assert_eq!(streams[0].app_name, "Slack");
    }

    #[test]
    fn generic_name_falls_back_to_media_name_when_binary_also_generic() {
        let dump = one_stream_dump(
            r#""application.name":"Chromium","application.process.binary":"chromium","media.name":"Some Tab""#,
        );
        let streams = parse_app_streams(&dump).unwrap();
        assert_eq!(streams[0].app_name, "Some Tab");
    }

    #[test]
    fn normal_app_name_is_left_untouched() {
        let dump = one_stream_dump(
            r#""application.name":"Spotify","application.process.binary":"spotify""#,
        );
        let streams = parse_app_streams(&dump).unwrap();
        assert_eq!(streams[0].app_name, "Spotify");
    }

    #[test]
    fn excludes_our_filter_chain_outputs() {
        let streams = parse_app_streams(DUMP).unwrap();
        let bins: Vec<&str> = streams.iter().map(|s| s.binary.as_str()).collect();
        assert!(!bins.iter().any(|b| b.contains(".output")),
            "filter-chain infra must be excluded: {bins:?}");
        assert!(bins.contains(&"firefox"), "real apps must remain: {bins:?}");
    }
}
