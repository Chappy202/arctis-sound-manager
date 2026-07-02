use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::AudioError;

/// One persistent routing rule: send the app's streams to a channel sink.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
";

/// Escape a string for embedding inside a double-quoted SPA-JSON string:
/// backslashes and double quotes are backslash-escaped so an app name like
/// `my "app"` cannot break the generated fragment.
fn escape_spa_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

/// Render one `<section> = [ { matches … actions … } … ]` SPA-JSON rules body.
/// Shared by the client (`stream.rules`) and pulse (`pulse.rules`) renderers —
/// both mechanisms use the identical match-rule layout (see pipewire(1)).
fn rules_section(section: &str, comment: &str, rules: &[RouteRule]) -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push_str(comment);
    out.push_str(&format!("{section} = [\n"));
    for r in rules {
        out.push_str("  {\n");
        out.push_str("    matches = [\n");
        out.push_str("      {\n");
        out.push_str(&format!(
            "        application.process.binary = \"{}\"\n",
            escape_spa_string(&r.app_binary)
        ));
        out.push_str("      }\n");
        out.push_str("    ]\n");
        out.push_str("    actions = {\n");
        out.push_str("      update-props = {\n");
        out.push_str(&format!(
            "        target.object = \"{}\"\n",
            escape_spa_string(&r.target_sink)
        ));
        out.push_str("      }\n");
        out.push_str("    }\n");
        out.push_str("  }\n");
    }
    out.push_str("]\n");
    out
}

/// Render the `stream.rules` fragment body for `~/.config/pipewire/client.conf.d/`.
/// Read by NATIVE PipeWire clients when they start up (pipewire-client.conf(5)),
/// so it applies to apps launched after the file is written — no daemon restart
/// (G3). Running apps are moved live via `pw-metadata` separately.
pub fn stream_rules_fragment(rules: &[RouteRule]) -> String {
    rules_section(
        "stream.rules",
        "# Per-app routing for NATIVE PipeWire clients (client.conf stream.rules);\n\
         # read by each client at ITS startup — applies to newly launched apps.\n",
        rules,
    )
}

/// Render the `pulse.rules` fragment body for `~/.config/pipewire/pipewire-pulse.conf.d/`.
/// Applied by the pipewire-pulse server to PulseAudio clients when they connect
/// (pipewire-pulse.conf(5)); the file itself is read at pipewire-pulse startup.
pub fn pulse_rules_fragment(rules: &[RouteRule]) -> String {
    rules_section(
        "pulse.rules",
        "# Per-app routing for PulseAudio clients (pipewire-pulse.conf pulse.rules);\n\
         # applied when a pulse client connects.\n",
        rules,
    )
}

/// Resolve `$HOME` (or `/tmp` as a last resort) for the fragment/state paths.
fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()))
}

/// Path of the managed native-client fragment:
/// `$HOME/.config/pipewire/client.conf.d/90-asm-routing.conf`.
pub fn client_fragment_path() -> PathBuf {
    client_fragment_path_in(&home_dir())
}

/// Compute the client fragment path rooted at a given home dir.
fn client_fragment_path_in(home: &std::path::Path) -> PathBuf {
    home.join(".config/pipewire/client.conf.d/90-asm-routing.conf")
}

/// Path of the managed pulse-client fragment:
/// `$HOME/.config/pipewire/pipewire-pulse.conf.d/90-asm-routing.conf`.
pub fn pulse_fragment_path() -> PathBuf {
    pulse_fragment_path_in(&home_dir())
}

/// Compute the pulse fragment path rooted at a given home dir.
fn pulse_fragment_path_in(home: &std::path::Path) -> PathBuf {
    home.join(".config/pipewire/pipewire-pulse.conf.d/90-asm-routing.conf")
}

/// Legacy fragment location written by older releases. The `node.rules` section
/// it carried is NOT a WirePlumber 0.5 section — nothing ever read it — so
/// `save_persistent` deletes it (stale-path cleanup).
fn legacy_wireplumber_fragment_path_in(home: &std::path::Path) -> PathBuf {
    home.join(".config/wireplumber/wireplumber.conf.d/90-asm-routing.conf")
}

/// Path of the canonical routes JSON state file:
/// `$HOME/.config/arctis-sound-manager/routes.json`.
fn routes_state_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".config/arctis-sound-manager/routes.json")
}

/// Compute the routes state path rooted at a given home dir.
fn routes_state_path_in(home: &std::path::Path) -> PathBuf {
    home.join(".config/arctis-sound-manager/routes.json")
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
    parse_stream_ids(pw_dump_json, app_match).map(|ids| ids[0].clone())
}

/// Find ALL stream node ids whose props match `app_match` in `pw-dump` JSON
/// (same selection rules as [`parse_stream_id`]). Multi-node apps (browsers
/// hold one output node per tab/stream) must be moved node-by-node; acting on
/// only the first leaves the siblings on the old sink. Errors when no node
/// matches, so the returned Vec is never empty.
pub fn parse_stream_ids(
    pw_dump_json: &str,
    app_match: &AppMatch,
) -> Result<Vec<String>, AudioError> {
    let array: serde_json::Value =
        serde_json::from_str(pw_dump_json).map_err(|e| AudioError::Parse {
            what: "pw-dump JSON".to_string(),
            detail: e.to_string(),
        })?;

    let objects = array.as_array().ok_or_else(|| AudioError::Parse {
        what: "pw-dump JSON".to_string(),
        detail: "expected a top-level JSON array".to_string(),
    })?;

    let mut ids = Vec::new();
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
            ids.push(id.to_string());
        }
    }

    if ids.is_empty() {
        return Err(AudioError::Parse {
            what: "stream id".to_string(),
            detail: format!(
                "no Stream/Output/Audio Node matching {} = {}",
                app_match.key(),
                app_match.value()
            ),
        });
    }
    Ok(ids)
}

use crate::runner::CommandRunner;
use std::fs;

/// Orchestrates per-app routing: a LIVE move via `pw-metadata` and a
/// PERSISTENT WirePlumber `node.rules` fragment. Subprocess-only (G1/G3).
pub struct Router<R: CommandRunner> {
    runner: R,
    rules: Vec<RouteRule>,
    /// Optional home directory override for tests; `None` means read from $HOME.
    home_override: Option<PathBuf>,
}

impl<R: CommandRunner> Router<R> {
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            rules: Vec::new(),
            home_override: None,
        }
    }

    pub fn with_rules(runner: R, rules: Vec<RouteRule>) -> Self {
        Self {
            runner,
            rules,
            home_override: None,
        }
    }

    /// Create a Router that reads/writes state under `home` instead of $HOME.
    /// Intended for tests only.
    #[cfg(test)]
    pub fn with_home(runner: R, home: PathBuf) -> Self {
        Self {
            runner,
            rules: Vec::new(),
            home_override: Some(home),
        }
    }

    #[cfg(test)]
    pub fn runner(&self) -> &R {
        &self.runner
    }

    pub fn list(&self) -> &[RouteRule] {
        &self.rules
    }

    fn effective_state_path(&self) -> PathBuf {
        match &self.home_override {
            Some(h) => routes_state_path_in(h),
            None => routes_state_path(),
        }
    }

    fn effective_client_fragment_path(&self) -> PathBuf {
        match &self.home_override {
            Some(h) => client_fragment_path_in(h),
            None => client_fragment_path(),
        }
    }

    fn effective_pulse_fragment_path(&self) -> PathBuf {
        match &self.home_override {
            Some(h) => pulse_fragment_path_in(h),
            None => pulse_fragment_path(),
        }
    }

    fn effective_legacy_fragment_path(&self) -> PathBuf {
        let home = match &self.home_override {
            Some(h) => h.clone(),
            None => home_dir(),
        };
        legacy_wireplumber_fragment_path_in(&home)
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

    /// Move a running app's streams to `target_sink` LIVE. Moves EVERY matching
    /// output node (browsers hold several; moving only the first left siblings
    /// on the old sink — same rationale as the engine's `move_stream`).
    /// Returns the ids moved (never empty on Ok).
    pub fn apply_live(
        &mut self,
        app: &AppMatch,
        target_sink: &str,
    ) -> Result<Vec<String>, AudioError> {
        let dump = self.runner.run("pw-dump", &[])?;
        let dump = Self::check(dump, "pw-dump")?;
        let ids = parse_stream_ids(&dump.stdout, app)?;
        for id in &ids {
            let argv = move_stream_argv(id, target_sink)?;
            let args: Vec<&str> = argv.iter().map(String::as_str).collect();
            let out = self.runner.run("pw-metadata", &args)?;
            Self::check(out, "pw-metadata")?;
        }
        Ok(ids)
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

    /// Remove the persistent rule for `app_binary`. No-op if not found.
    /// Does NOT persist — caller must call `save_persistent` after.
    pub fn remove_rule(&mut self, app_binary: &str) {
        self.rules.retain(|r| r.app_binary != app_binary);
    }

    /// Best-effort live clear: move the app's streams back to the default sink
    /// by deleting the `target.object` metadata key on EVERY matching node
    /// (multi-node symmetry with `apply_live`). Errors are silently ignored at
    /// the call site (same pattern as `apply_live` for route-set).
    pub fn clear_live(&mut self, app: &AppMatch) -> Result<(), AudioError> {
        let dump = self.runner.run("pw-dump", &[])?;
        let dump = Self::check(dump, "pw-dump")?;
        for id in parse_stream_ids(&dump.stdout, app)? {
            let argv = clear_stream_target_argv(&id)?;
            let args: Vec<&str> = argv.iter().map(String::as_str).collect();
            let out = self.runner.run("pw-metadata", &args)?;
            Self::check(out, "pw-metadata")?;
        }
        Ok(())
    }

    /// Load persistent routes from `routes.json`. Absent file → empty rules (no error).
    /// Parse failure → typed AudioError.
    pub fn load_persistent(&mut self) -> Result<(), AudioError> {
        let path = self.effective_state_path();
        let content = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.rules = Vec::new();
                return Ok(());
            }
            Err(e) => {
                return Err(AudioError::Spawn {
                    program: format!("read {}", path.display()),
                    source_msg: e.to_string(),
                });
            }
        };
        self.rules = serde_json::from_str(&content).map_err(|e| AudioError::Parse {
            what: "routes.json".to_string(),
            detail: e.to_string(),
        })?;
        Ok(())
    }

    /// Create parent dirs and atomically write `body` to `path` (tmp + rename
    /// in the same directory, so the rename never crosses filesystems).
    fn atomic_write(path: &std::path::Path, body: &str) -> Result<(), AudioError> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| AudioError::Spawn {
                program: format!("mkdir {}", dir.display()),
                source_msg: e.to_string(),
            })?;
        }
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "asm".to_string());
        let tmp = path.with_file_name(format!(".{file_name}.tmp"));
        fs::write(&tmp, body).map_err(|e| AudioError::Spawn {
            program: format!("write {}", tmp.display()),
            source_msg: e.to_string(),
        })?;
        fs::rename(&tmp, path).map_err(|e| AudioError::Spawn {
            program: format!("rename {} -> {}", tmp.display(), path.display()),
            source_msg: e.to_string(),
        })?;
        Ok(())
    }

    /// Save persistent routes to `routes.json` (atomic write), then regenerate
    /// and atomically write the client (`stream.rules`) and pulse (`pulse.rules`)
    /// fragments from the current rules. Also removes the legacy
    /// `wireplumber.conf.d/90-asm-routing.conf` (a dead `node.rules` file that
    /// WirePlumber 0.5 never read). `routes.json` is a pure PROJECTION of the
    /// rules the caller passed in — callers own the source of truth (G4).
    pub fn save_persistent(&self) -> Result<(), AudioError> {
        let json = serde_json::to_string_pretty(&self.rules).map_err(|e| AudioError::Parse {
            what: "routes.json serialization".to_string(),
            detail: e.to_string(),
        })?;
        Self::atomic_write(&self.effective_state_path(), &json)?;
        Self::atomic_write(
            &self.effective_client_fragment_path(),
            &stream_rules_fragment(&self.rules),
        )?;
        Self::atomic_write(
            &self.effective_pulse_fragment_path(),
            &pulse_rules_fragment(&self.rules),
        )?;
        // Best-effort cleanup of the stale legacy fragment (absence is fine).
        let _ = fs::remove_file(self.effective_legacy_fragment_path());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_rules() -> Vec<RouteRule> {
        vec![
            RouteRule::new("firefox", "Arctis_Media"),
            RouteRule::new("Discord", "Arctis_Chat"),
        ]
    }

    #[test]
    fn stream_rules_renders_exact_two_rule_fixture() {
        let got = stream_rules_fragment(&two_rules());
        let want = include_str!("../tests/fixtures/pw_stream_rules.conf");
        if got != want {
            eprintln!("=== GOT ===\n{got}\n=== WANT ===\n{want}");
        }
        assert_eq!(got, want);
    }

    #[test]
    fn pulse_rules_renders_exact_two_rule_fixture() {
        let got = pulse_rules_fragment(&two_rules());
        let want = include_str!("../tests/fixtures/pw_pulse_rules.conf");
        if got != want {
            eprintln!("=== GOT ===\n{got}\n=== WANT ===\n{want}");
        }
        assert_eq!(got, want);
    }

    #[test]
    fn empty_rules_emit_empty_array() {
        let got = stream_rules_fragment(&[]);
        assert!(got.contains("stream.rules = [\n]\n"));
        assert!(got.starts_with("# Managed by Arctis Sound Manager"));
        let got = pulse_rules_fragment(&[]);
        assert!(got.contains("pulse.rules = [\n]\n"));
    }

    #[test]
    fn fragment_values_are_spa_escaped() {
        // A quote or backslash in an app name must not break the SPA-JSON string.
        let rules = vec![RouteRule::new(r#"my "app"\bin"#, "Arctis_Media")];
        for got in [stream_rules_fragment(&rules), pulse_rules_fragment(&rules)] {
            assert!(
                got.contains(r#"application.process.binary = "my \"app\"\\bin""#),
                "unescaped fragment: {got}"
            );
        }
    }

    #[test]
    fn fragment_paths_are_under_pipewire_conf_d() {
        let s = client_fragment_path().to_string_lossy().into_owned();
        assert!(s.ends_with("pipewire/client.conf.d/90-asm-routing.conf"));
        let s = pulse_fragment_path().to_string_lossy().into_owned();
        assert!(s.ends_with("pipewire/pipewire-pulse.conf.d/90-asm-routing.conf"));
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
        let ids = router
            .apply_live(&AppMatch::Binary("firefox".into()), "Arctis_Media")
            .unwrap();
        assert_eq!(ids, vec!["73"]);
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

    /// A multi-node app (browsers) must have EVERY output node moved/cleared,
    /// not just the first — matching the engine's move_stream behaviour.
    #[test]
    fn apply_live_and_clear_live_act_on_every_node_of_a_binary() {
        let dump = r#"[
          {"id":73,"type":"PipeWire:Interface:Node","info":{"props":{
            "media.class":"Stream/Output/Audio","application.process.binary":"vivaldi"}}},
          {"id":91,"type":"PipeWire:Interface:Node","info":{"props":{
            "media.class":"Stream/Output/Audio","application.process.binary":"vivaldi"}}}
        ]"#;
        let runner = MockRunner::new()
            .with_output(0, dump, "") // pw-dump
            .with_output(0, "", "") // move node 73
            .with_output(0, "", ""); // move node 91
        let mut router = Router::new(runner);
        let ids = router
            .apply_live(&AppMatch::Binary("vivaldi".into()), "Arctis_Media")
            .unwrap();
        assert_eq!(ids, vec!["73", "91"]);
        let calls = &router.runner().calls;
        assert_eq!(calls.len(), 3, "pw-dump + one move per node");
        assert_eq!(calls[1][3], "73");
        assert_eq!(calls[2][3], "91");

        let runner = MockRunner::new()
            .with_output(0, dump, "")
            .with_output(0, "", "")
            .with_output(0, "", "");
        let mut router = Router::new(runner);
        router.clear_live(&AppMatch::Binary("vivaldi".into())).unwrap();
        let calls = &router.runner().calls;
        assert_eq!(calls.len(), 3, "pw-dump + one clear per node");
        assert_eq!(calls[1], vec!["pw-metadata", "-d", "73", "target.object"]);
        assert_eq!(calls[2], vec!["pw-metadata", "-d", "91", "target.object"]);
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
    fn save_persistent_writes_both_fragments_and_removes_legacy() {
        let home = unique_tmp("frag_pair");
        // Seed a stale legacy fragment that save_persistent must clean up.
        let legacy = legacy_wireplumber_fragment_path_in(&home);
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "node.rules = []\n").unwrap();

        let mut router = Router::with_home(MockRunner::new(), home.clone());
        router.set_rule(RouteRule::new("firefox", "Arctis_Media"));
        router.save_persistent().unwrap();

        let client = std::fs::read_to_string(client_fragment_path_in(&home)).unwrap();
        assert!(client.contains("stream.rules = [\n"));
        assert!(client.contains("application.process.binary = \"firefox\""));
        assert!(client.contains("target.object = \"Arctis_Media\""));
        let pulse = std::fs::read_to_string(pulse_fragment_path_in(&home)).unwrap();
        assert!(pulse.contains("pulse.rules = [\n"));
        assert!(pulse.contains("target.object = \"Arctis_Media\""));
        assert!(!legacy.exists(), "stale wireplumber node.rules fragment must be removed");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// Helper to create a unique temp dir for a test without touching HOME.
    fn unique_tmp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        std::env::temp_dir().join(format!("asm_{tag}_{pid}_{nanos}", pid = std::process::id()))
    }

    /// Verify that saving and loading preserves ALL rules across two separate Router instances.
    #[test]
    fn test_multi_app_persist_round_trip() {
        let home = unique_tmp("persist");

        // First router: set rule for app A and save.
        {
            let mut router = Router::with_home(MockRunner::new(), home.clone());
            router.load_persistent().unwrap();
            router.set_rule(RouteRule::new("firefox", "Arctis_Game"));
            router.save_persistent().unwrap();
        }

        // Second router: load, add app B, save.
        {
            let mut router = Router::with_home(MockRunner::new(), home.clone());
            router.load_persistent().unwrap();
            router.set_rule(RouteRule::new("discord", "Arctis_Media"));
            router.save_persistent().unwrap();
        }

        // Third router: load and assert BOTH rules are present.
        {
            let mut router = Router::with_home(MockRunner::new(), home.clone());
            router.load_persistent().unwrap();
            let rules = router.list();
            assert_eq!(
                rules.len(),
                2,
                "expected 2 rules, got {}: {:?}",
                rules.len(),
                rules
            );
            let has_firefox = rules
                .iter()
                .any(|r| r.app_binary == "firefox" && r.target_sink == "Arctis_Game");
            let has_discord = rules
                .iter()
                .any(|r| r.app_binary == "discord" && r.target_sink == "Arctis_Media");
            assert!(has_firefox, "firefox rule missing: {:?}", rules);
            assert!(has_discord, "discord rule missing: {:?}", rules);
        }

        let _ = std::fs::remove_dir_all(&home);
    }

    /// load_persistent on an absent file yields empty rules with no error.
    #[test]
    fn test_load_persistent_absent() {
        let home = unique_tmp("absent");
        let mut router = Router::with_home(MockRunner::new(), home.clone());
        router.load_persistent().unwrap();
        assert!(router.list().is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    /// After setting two rules and save_persistent, both fragments contain both app names.
    #[test]
    fn test_fragment_contains_both_apps() {
        let home = unique_tmp("frag");

        let mut router = Router::with_home(MockRunner::new(), home.clone());
        router.set_rule(RouteRule::new("firefox", "Arctis_Game"));
        router.set_rule(RouteRule::new("discord", "Arctis_Chat"));
        router.save_persistent().unwrap();

        for path in [client_fragment_path_in(&home), pulse_fragment_path_in(&home)] {
            let body = std::fs::read_to_string(&path).unwrap();
            assert!(
                body.contains("firefox"),
                "fragment missing 'firefox': {body}"
            );
            assert!(
                body.contains("discord"),
                "fragment missing 'discord': {body}"
            );
        }

        let _ = std::fs::remove_dir_all(&home);
    }
}
