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
///
/// Parses the JSON array produced by `pw-dump` with `serde_json` and filters
/// for objects that are:
///   - `"type": "PipeWire:Interface:Node"`
///   - `info.props.media.class` starts with `"Stream/Output/Audio"`
///
/// A `Client` object (which also carries `application.process.binary`) is
/// intentionally excluded because targeting it with `pw-metadata` does not
/// move the audio stream.
///
/// Matching rules:
///   - `AppMatch::Name(v)` → `props["application.name"] == v`
///   - `AppMatch::Binary(v)` → `props["node.name"] == v` OR
///     `props["application.process.binary"] == v`
pub fn parse_stream_id(pw_dump_json: &str, app_match: &AppMatch) -> Result<String, AudioError> {
    let array: serde_json::Value =
        serde_json::from_str(pw_dump_json).map_err(|e| AudioError::Parse {
            what: "pw-dump JSON".to_string(),
            detail: e.to_string(),
        })?;

    let objects = array.as_array().ok_or_else(|| AudioError::Parse {
        what: "pw-dump JSON".to_string(),
        detail: "expected a top-level JSON array".to_string(),
    })?;

    for obj in objects {
        // Must be a Node.
        if obj.get("type").and_then(|v| v.as_str()) != Some("PipeWire:Interface:Node") {
            continue;
        }

        let props = match obj.get("info").and_then(|i| i.get("props")) {
            Some(p) => p,
            None => continue,
        };

        // Must be a playback stream.
        let media_class = match props.get("media.class").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => continue,
        };
        if !media_class.starts_with("Stream/Output/Audio") {
            continue;
        }

        // Match against the requested app identifier.
        let matched = match app_match {
            AppMatch::Name(v) => {
                props.get("application.name").and_then(|s| s.as_str()) == Some(v.as_str())
            }
            AppMatch::Binary(v) => {
                let node_name = props.get("node.name").and_then(|s| s.as_str());
                let bin = props
                    .get("application.process.binary")
                    .and_then(|s| s.as_str());
                node_name == Some(v.as_str()) || bin == Some(v.as_str())
            }
        };

        if matched {
            let id = obj
                .get("id")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| AudioError::Parse {
                    what: "stream id".to_string(),
                    detail: "matched node has no numeric top-level `id`".to_string(),
                })?;
            return Ok(id.to_string());
        }
    }

    Err(AudioError::Parse {
        what: "stream id".to_string(),
        detail: format!(
            "no Stream/Output/Audio Node matching {} = {}",
            app_match.key(),
            app_match.value()
        ),
    })
}

use crate::runner::CommandRunner;
use std::fs;

/// Orchestrates per-app routing: a LIVE move via `pw-metadata` and a
/// PERSISTENT WirePlumber `node.rules` fragment. Subprocess-only (G1/G3).
pub struct Router<R: CommandRunner> {
    runner: R,
    rules: Vec<RouteRule>,
}

impl<R: CommandRunner> Router<R> {
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            rules: Vec::new(),
        }
    }

    pub fn with_rules(runner: R, rules: Vec<RouteRule>) -> Self {
        Self { runner, rules }
    }

    #[cfg(test)]
    pub fn runner(&self) -> &R {
        &self.runner
    }

    pub fn list(&self) -> &[RouteRule] {
        &self.rules
    }

    fn check(
        out: crate::runner::CmdOutput,
        program: &str,
    ) -> Result<crate::runner::CmdOutput, AudioError> {
        if out.status == 0 {
            Ok(out)
        } else {
            Err(AudioError::NonZeroExit {
                program: program.to_string(),
                status: out.status,
                stderr: out.stderr,
            })
        }
    }

    /// Move a running app's stream to `target_sink` LIVE. Returns the id moved.
    pub fn apply_live(&mut self, app: &AppMatch, target_sink: &str) -> Result<String, AudioError> {
        let dump = self.runner.run("pw-dump", &[])?;
        let dump = Self::check(dump, "pw-dump")?;
        let id = parse_stream_id(&dump.stdout, app)?;
        let argv = move_stream_argv(&id, target_sink)?;
        let args: Vec<&str> = argv.iter().map(String::as_str).collect();
        let out = self.runner.run("pw-metadata", &args)?;
        Self::check(out, "pw-metadata")?;
        Ok(id)
    }

    /// Upsert a persistent rule by app binary (no duplicates).
    pub fn set_rule(&mut self, rule: RouteRule) {
        if let Some(existing) = self
            .rules
            .iter_mut()
            .find(|r| r.app_binary == rule.app_binary)
        {
            existing.target_sink = rule.target_sink;
        } else {
            self.rules.push(rule);
        }
    }

    /// Write the persistent WirePlumber fragment to disk (creates dirs).
    pub fn write_persistent(&mut self) -> Result<PathBuf, AudioError> {
        let path = wireplumber_fragment_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| AudioError::Spawn {
                program: "mkdir wireplumber.conf.d".to_string(),
                source_msg: e.to_string(),
            })?;
        }
        let body = node_rules_fragment(&self.rules);
        fs::write(&path, body).map_err(|e| AudioError::Spawn {
            program: "write wireplumber fragment".to_string(),
            source_msg: e.to_string(),
        })?;
        Ok(path)
    }
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

    /// Regression: the fixture contains BOTH a Client object (id 81, has
    /// `application.process.binary: "spotify"`) AND a Node object (id 86,
    /// `media.class: "Stream/Output/Audio"`, `node.name: "spotify"`).
    /// The old text-scan returned 81 (the Client); the JSON parser must
    /// return 86 (the Node) because only the Node can be targeted by
    /// `pw-metadata`.
    #[test]
    fn parse_stream_id_returns_node_not_client_for_spotify_binary() {
        let dump = include_str!("../tests/fixtures/pw_dump_streams.json");
        let id = parse_stream_id(dump, &AppMatch::Binary("spotify".into())).unwrap();
        // Must be 86 (the Node), NOT 81 (the Client).
        assert_eq!(
            id, "86",
            "expected Node id 86, got {id} — did we accidentally return the Client?"
        );
    }

    #[test]
    fn parse_stream_id_spotify_by_name_returns_node() {
        let dump = include_str!("../tests/fixtures/pw_dump_streams.json");
        let id = parse_stream_id(dump, &AppMatch::Name("Spotify".into())).unwrap();
        assert_eq!(id, "86");
    }

    #[test]
    fn parse_stream_id_malformed_json_is_parse_error() {
        let err = parse_stream_id("not json at all", &AppMatch::Binary("x".into())).unwrap_err();
        assert!(matches!(err, AudioError::Parse { .. }));
    }

    #[test]
    fn parse_stream_id_empty_dump_is_parse_error() {
        let err = parse_stream_id("", &AppMatch::Binary("x".into())).unwrap_err();
        assert!(matches!(err, AudioError::Parse { .. }));
    }

    // --- Router tests ---

    use crate::runner::MockRunner;

    #[test]
    fn apply_live_dumps_parses_then_moves_with_exact_argv() {
        let dump = include_str!("../tests/fixtures/pw_dump_streams.json");
        let runner = MockRunner::new()
            .with_output(0, dump, "") // pw-dump
            .with_output(0, "", ""); // pw-metadata move
        let mut router = Router::new(runner);
        let id = router
            .apply_live(&AppMatch::Binary("firefox".into()), "Arctis_Media")
            .unwrap();
        assert_eq!(id, "73");
        let calls = &router.runner().calls;
        assert_eq!(calls[0], vec!["pw-dump"]);
        assert_eq!(
            calls[1],
            vec![
                "pw-metadata",
                "-n",
                "default",
                "73",
                "target.object",
                "Arctis_Media"
            ]
        );
    }

    #[test]
    fn apply_live_errors_when_app_absent() {
        let dump = include_str!("../tests/fixtures/pw_dump_streams.json");
        let runner = MockRunner::new().with_output(0, dump, "");
        let mut router = Router::new(runner);
        let err = router
            .apply_live(&AppMatch::Binary("nope".into()), "Arctis_Media")
            .unwrap_err();
        assert!(matches!(err, AudioError::Parse { .. }));
        // Only pw-dump ran; no move attempted.
        assert_eq!(router.runner().calls.len(), 1);
    }

    #[test]
    fn set_rule_upserts_without_duplicating() {
        let mut router = Router::new(MockRunner::new());
        router.set_rule(RouteRule::new("firefox", "Arctis_Media"));
        router.set_rule(RouteRule::new("firefox", "Arctis_Game")); // re-route same app
        router.set_rule(RouteRule::new("Discord", "Arctis_Chat"));
        assert_eq!(router.list().len(), 2);
        assert_eq!(router.list()[0].target_sink, "Arctis_Game");
    }

    #[test]
    fn write_persistent_writes_fragment_to_temp_home() {
        // Point HOME at a temp dir so the test writes nowhere real.
        let tmp = std::env::temp_dir().join(format!("asm_wp_test_{}", std::process::id()));
        std::env::set_var("HOME", &tmp);
        let mut router = Router::with_rules(
            MockRunner::new(),
            vec![RouteRule::new("firefox", "Arctis_Media")],
        );
        let path = router.write_persistent().unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("application.process.binary = \"firefox\""));
        assert!(body.contains("target.object = \"Arctis_Media\""));
        assert!(path.to_string_lossy().ends_with("90-asm-routing.conf"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
