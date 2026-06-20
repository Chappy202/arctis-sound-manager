use std::path::PathBuf;

use crate::error::AudioError;

/// One persistent routing rule: send the app's streams to a channel sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRule {
    /// Matches `application.process.binary` (e.g. "firefox", "Discord").
    pub app_binary: String,
    /// Channel sink `node.name` to route to (e.g. "Arctis_Media").
    pub target_sink: String,
}

impl RouteRule {
    pub fn new(app_binary: &str, target_sink: &str) -> Self {
        Self {
            app_binary: app_binary.to_string(),
            target_sink: target_sink.to_string(),
        }
    }
}

const HEADER: &str = "\
# Managed by Arctis Sound Manager — do not edit by hand.
# Persistent per-application routing (WirePlumber 0.5 SPA-JSON node.rules).
";

/// Render the full WirePlumber 0.5 `node.rules` SPA-JSON fragment body.
/// Emits both `node.target` and `target.object` so the rule is honoured
/// regardless of which key the running WirePlumber prefers.
pub fn node_rules_fragment(rules: &[RouteRule]) -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push_str("node.rules = [\n");
    for r in rules {
        out.push_str("  {\n");
        out.push_str("    matches = [\n");
        out.push_str("      {\n");
        out.push_str(&format!(
            "        application.process.binary = \"{}\"\n",
            r.app_binary
        ));
        out.push_str("      }\n");
        out.push_str("    ]\n");
        out.push_str("    actions = {\n");
        out.push_str("      update-props = {\n");
        out.push_str(&format!("        node.target = \"{}\"\n", r.target_sink));
        out.push_str(&format!("        target.object = \"{}\"\n", r.target_sink));
        out.push_str("      }\n");
        out.push_str("    }\n");
        out.push_str("  }\n");
    }
    out.push_str("]\n");
    out
}

/// Path of the managed fragment: `$HOME/.config/wireplumber/wireplumber.conf.d/90-asm-routing.conf`.
pub fn wireplumber_fragment_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let mut p = PathBuf::from(home);
    p.push(".config");
    p.push("wireplumber");
    p.push("wireplumber.conf.d");
    p.push("90-asm-routing.conf");
    p
}

/// Which application property to match a running stream by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMatch {
    /// Match `application.process.binary` (preferred — stable across windows).
    Binary(String),
    /// Match `application.name`.
    Name(String),
}

impl AppMatch {
    fn key(&self) -> &'static str {
        match self {
            AppMatch::Binary(_) => "application.process.binary",
            AppMatch::Name(_) => "application.name",
        }
    }
    fn value(&self) -> &str {
        match self {
            AppMatch::Binary(v) | AppMatch::Name(v) => v,
        }
    }
}

/// Argv (after `pw-metadata`) to move a running stream LIVE to a target sink
/// by setting `target.object` on the stream node in the `default` metadata.
/// The value is the target sink `node.name` (Research basis default; an
/// OWNER-RUN correction to object.serial is a one-line change here).
pub fn move_stream_argv(stream_id: &str, target_sink: &str) -> Result<Vec<String>, AudioError> {
    if stream_id.trim().is_empty() {
        return Err(AudioError::Invalid("empty stream id".into()));
    }
    if target_sink.trim().is_empty() {
        return Err(AudioError::Invalid("empty target sink".into()));
    }
    Ok(vec![
        "-n".to_string(),
        "default".to_string(),
        stream_id.to_string(),
        "target.object".to_string(),
        target_sink.to_string(),
    ])
}

/// Argv (after `pw-metadata`) to clear a live move (release the stream back
/// to policy): delete the `target.object` key for the stream node id.
pub fn clear_stream_target_argv(stream_id: &str) -> Result<Vec<String>, AudioError> {
    if stream_id.trim().is_empty() {
        return Err(AudioError::Invalid("empty stream id".into()));
    }
    Ok(vec![
        "-d".to_string(),
        stream_id.to_string(),
        "target.object".to_string(),
    ])
}

/// Find the stream node id whose props match `app_match` in `pw-dump` JSON.
/// Lightweight scan (no serde): locates the matched key/value, then reads the
/// nearest preceding `"id":` within the same object. Verified against a canned
/// fixture; OWNER-RUN confirms the real layout.
pub fn parse_stream_id(pw_dump_json: &str, app_match: &AppMatch) -> Result<String, AudioError> {
    let needle = format!("\"{}\": \"{}\"", app_match.key(), app_match.value());
    let Some(match_pos) = pw_dump_json.find(&needle) else {
        return Err(AudioError::Parse {
            what: "stream id".to_string(),
            detail: format!("no stream with {} = {}", app_match.key(), app_match.value()),
        });
    };
    // Scan backwards from the match for the most recent `"id":` field.
    let head = &pw_dump_json[..match_pos];
    let Some(id_kw) = head.rfind("\"id\":") else {
        return Err(AudioError::Parse {
            what: "stream id".to_string(),
            detail: "matched app but no preceding \"id\" field".to_string(),
        });
    };
    let after = &head[id_kw + "\"id\":".len()..];
    let digits: String = after
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return Err(AudioError::Parse {
            what: "stream id".to_string(),
            detail: "could not read numeric id".to_string(),
        });
    }
    Ok(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_exact_two_rule_fixture() {
        let rules = vec![
            RouteRule::new("firefox", "Arctis_Media"),
            RouteRule::new("Discord", "Arctis_Chat"),
        ];
        let got = node_rules_fragment(&rules);
        let want = include_str!("../tests/fixtures/wp_node_rules.conf");
        if got != want {
            eprintln!("=== GOT ===\n{got}\n=== WANT ===\n{want}");
        }
        assert_eq!(got, want);
    }

    #[test]
    fn empty_rules_emit_empty_array() {
        let got = node_rules_fragment(&[]);
        assert!(got.contains("node.rules = [\n]\n"));
        assert!(got.starts_with("# Managed by Arctis Sound Manager"));
    }

    #[test]
    fn fragment_path_is_under_wireplumber_conf_d() {
        let p = wireplumber_fragment_path();
        let s = p.to_string_lossy();
        assert!(s.ends_with("wireplumber/wireplumber.conf.d/90-asm-routing.conf"));
    }

    #[test]
    fn move_argv_sets_target_object_in_default_metadata() {
        let argv = move_stream_argv("73", "Arctis_Media").unwrap();
        assert_eq!(
            argv,
            vec!["-n", "default", "73", "target.object", "Arctis_Media"]
        );
    }

    #[test]
    fn clear_argv_deletes_target_object_key() {
        let argv = clear_stream_target_argv("73").unwrap();
        assert_eq!(argv, vec!["-d", "73", "target.object"]);
    }

    #[test]
    fn move_argv_rejects_empty_inputs() {
        assert!(move_stream_argv("", "Arctis_Media").is_err());
        assert!(move_stream_argv("73", "  ").is_err());
    }

    #[test]
    fn parse_stream_id_by_binary() {
        let dump = include_str!("../tests/fixtures/pw_dump_streams.json");
        let id = parse_stream_id(dump, &AppMatch::Binary("firefox".into())).unwrap();
        assert_eq!(id, "73");
    }

    #[test]
    fn parse_stream_id_by_name() {
        let dump = include_str!("../tests/fixtures/pw_dump_streams.json");
        let id = parse_stream_id(dump, &AppMatch::Name("Discord".into())).unwrap();
        assert_eq!(id, "88");
    }

    #[test]
    fn parse_stream_id_absent_is_typed_error() {
        let dump = include_str!("../tests/fixtures/pw_dump_streams.json");
        let err = parse_stream_id(dump, &AppMatch::Binary("nope".into())).unwrap_err();
        assert!(matches!(err, AudioError::Parse { .. }));
    }
}
