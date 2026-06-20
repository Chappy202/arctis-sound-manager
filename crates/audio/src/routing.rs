use std::path::PathBuf;

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
}
