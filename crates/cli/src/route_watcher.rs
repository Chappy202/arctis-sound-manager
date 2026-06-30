//! Route stream-watcher: re-applies remembered per-app routes when an
//! application output stream (re)appears on the PipeWire graph.
//!
//! ## Why
//! A remembered route is applied once (on the user's explicit move) as a
//! `pw-metadata -n default <NODE_ID> target.object <sink>` entry keyed on the
//! stream's TRANSIENT node id. When an app (e.g. Vivaldi) goes idle, PipeWire
//! destroys that node and drops the metadata entry; on resume the app gets a
//! NEW node id with no route and falls back to the default sink. Nothing
//! re-applies it. This watcher closes that gap.
//!
//! ## Shape (SAFE, DECOUPLED)
//! A pipewire-rs `MainLoop` + `Context` + `Core` + `Registry` runs on its own
//! thread. On each newly-appearing `Stream/Output/Audio` node it:
//!   1. extracts the app binary (`application.process.binary`, falling back to
//!      `application.name`) and the node id,
//!   2. reads the PERSISTED routes source of truth directly — the active
//!      profile's `routes` in the unified config (`arctis_config::store::load`)
//!      — building a `binary -> target_sink` snapshot. It NEVER calls into the
//!      single-threaded `Engine`,
//!   3. if (and only if) that binary has a remembered sink, re-applies the
//!      route via the EXISTING `move_stream_argv` builder run through
//!      `pw-metadata` (no new mechanism, G1).
//!
//! It ONLY re-applies EXISTING remembered routes — never creates a route, never
//! overrides a manual move, never touches HID/device state (G2).
//!
//! ## Testability
//! The pure pieces (snapshot lookup, props -> (binary, id) extraction, and the
//! "build this exact argv" decision) are unit-tested below. The live pipewire-rs
//! event loop can only be exercised against a real PipeWire daemon, so it is
//! OWNER-VERIFIED manually (route an app, let it idle, resume, confirm it
//! returns) and is compiled only under the `pw-watcher` feature.

// Without the `pw-watcher` feature the pure helpers below are exercised only by
// the unit tests (the live module that consumes them is cfg'd out), so silence
// dead-code warnings for the non-feature, non-test build.
#![cfg_attr(not(feature = "pw-watcher"), allow(dead_code))]

use arctis_audio::move_stream_argv;
use arctis_config::Config;
use std::collections::HashMap;

/// `media.class` prefix that marks an application playback stream node.
pub const STREAM_OUTPUT_AUDIO: &str = "Stream/Output/Audio";

/// A decoupled snapshot of `app_binary -> target_sink`, read fresh from the
/// persisted config (NOT from the live `Engine`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteSnapshot {
    map: HashMap<String, String>,
}

impl RouteSnapshot {
    /// Build a snapshot from an already-loaded config's active profile routes.
    pub fn from_config(cfg: &Config) -> Self {
        let mut map = HashMap::new();
        if let Ok(profile) = cfg.active() {
            for r in &profile.routes {
                map.insert(r.app_binary.clone(), r.target_sink.clone());
            }
        }
        Self { map }
    }

    /// Load a snapshot fresh from the on-disk config file (the same source of
    /// truth `persist_route` writes). Any load/parse error yields an EMPTY
    /// snapshot and a log line — the watcher must never panic or crash the
    /// daemon over a transient config read (G7).
    pub fn load() -> Self {
        match arctis_config::store::load() {
            Ok(cfg) => Self::from_config(&cfg),
            Err(e) => {
                eprintln!("route-watcher: config load failed ({e}); treating as no routes");
                Self::default()
            }
        }
    }

    /// The remembered target sink for `binary`, if any.
    pub fn target_for(&self, binary: &str) -> Option<&str> {
        self.map.get(binary).map(String::as_str)
    }

    // Test-only inspectors: in a binary crate `pub` does not suppress dead_code,
    // and the runtime watcher never queries size — only the unit tests do.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// Decide the app-binary identity of a freshly-appeared node, mirroring
/// `arctis_audio::parse_stream_id`: the node must be a `Stream/Output/Audio`,
/// and we prefer `application.process.binary` (stable across windows), falling
/// back to `application.name`. Returns `None` for non-stream nodes or streams
/// with no usable identifier.
pub fn stream_app_binary(
    media_class: Option<&str>,
    binary: Option<&str>,
    app_name: Option<&str>,
) -> Option<String> {
    let class = media_class?;
    if !class.starts_with(STREAM_OUTPUT_AUDIO) {
        return None;
    }
    if let Some(b) = binary {
        let b = b.trim();
        if !b.is_empty() {
            return Some(b.to_string());
        }
    }
    if let Some(n) = app_name {
        let n = n.trim();
        if !n.is_empty() {
            return Some(n.to_string());
        }
    }
    None
}

/// The decision + argv builder: if `binary` has a remembered route in
/// `snapshot`, return the exact `pw-metadata` argv (AFTER the program name) to
/// re-apply it for `node_id`, reusing `move_stream_argv` (G1, no duplication).
/// Returns `None` when there is no remembered route (the watcher then does
/// nothing — it never creates routes).
pub fn reapply_argv(snapshot: &RouteSnapshot, binary: &str, node_id: u32) -> Option<Vec<String>> {
    let sink = snapshot.target_for(binary)?;
    // move_stream_argv only errors on empty inputs; node_id is always non-empty
    // and a real remembered sink is non-empty, so this is effectively infallible
    // here — but we still propagate None rather than unwrap (G7).
    move_stream_argv(&node_id.to_string(), sink).ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Live pipewire-rs watcher (feature-gated; OWNER-COMPILED + OWNER-VERIFIED).
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "pw-watcher")]
mod live {
    use super::{reapply_argv, stream_app_binary, RouteSnapshot};
    use arctis_audio::{CommandRunner, RealRunner};
    use pipewire as pw;
    use pw::types::ObjectType;
    use std::thread::JoinHandle;

    /// Internal control message sent into the loop thread to quit cleanly.
    struct Terminate;

    /// Handle to the running watcher thread. Dropping without `stop()` still
    /// detaches cleanly, but prefer `stop()` for a deterministic join.
    pub struct RouteWatcher {
        sender: pw::channel::Sender<Terminate>,
        handle: Option<JoinHandle<()>>,
    }

    impl RouteWatcher {
        /// Start the watcher on its own thread. Never panics; if the thread
        /// cannot be spawned it logs and returns a handle whose `stop()` is a
        /// no-op so the daemon is unaffected.
        pub fn start() -> Self {
            let (sender, receiver) = pw::channel::channel::<Terminate>();
            let handle = std::thread::Builder::new()
                .name("route-watcher".into())
                .spawn(move || {
                    if let Err(e) = run_loop(receiver) {
                        eprintln!("daemon: route watcher exited with error (continuing): {e}");
                    }
                })
                .map_err(|e| {
                    eprintln!("daemon: failed to spawn route watcher thread (continuing): {e}");
                })
                .ok();
            eprintln!("daemon: route watcher started (pw-watcher feature enabled)");
            Self { sender, handle }
        }

        /// Ask the loop to quit and join the thread. Idempotent and panic-safe.
        pub fn stop(mut self) {
            // Wake the loop with a Terminate; ignore send error (loop already gone).
            let _ = self.sender.send(Terminate);
            if let Some(h) = self.handle.take() {
                if let Err(e) = h.join() {
                    eprintln!("daemon: route watcher thread panicked on shutdown: {e:?}");
                }
            }
        }
    }

    /// Body of the watcher thread. Builds the PipeWire connection, attaches the
    /// registry + terminate listeners, and runs the loop until a `Terminate`
    /// message arrives. All callbacks are written to never panic (G7).
    fn run_loop(receiver: pw::channel::Receiver<Terminate>) -> Result<(), pw::Error> {
        pw::init();
        let mainloop = pw::main_loop::MainLoop::new(None)?;
        let context = pw::context::Context::new(&mainloop)?;
        let core = context.connect(None)?;
        let registry = core.get_registry()?;

        // Terminate hook: a message on the channel quits the loop cleanly from
        // the daemon's shutdown path (no leaked thread, no hang).
        let _terminate = {
            // Borrow the function-level `mainloop` for `.loop_()` (it outlives the
            // attached receiver), and move a SEPARATE clone into the quit closure —
            // one binding can't be borrowed and moved in the same expression.
            let ml = mainloop.clone();
            receiver.attach(mainloop.loop_(), move |_| ml.quit())
        };

        // Resilience: log Core errors instead of letting them bubble up and end
        // the thread. A transient PipeWire error must not take down the daemon.
        let _core_listener = core
            .add_listener_local()
            .error(|id, seq, res, message| {
                eprintln!("route-watcher: core error id={id} seq={seq} res={res}: {message}");
            })
            .register();

        let _registry_listener = registry
            .add_listener_local()
            .global(move |global| {
                // Only Node globals can carry a Stream/Output/Audio class.
                if global.type_ != ObjectType::Node {
                    return;
                }
                let props = match global.props {
                    Some(p) => p,
                    None => return,
                };
                let binary = match stream_app_binary(
                    props.get("media.class"),
                    props.get("application.process.binary"),
                    props.get("application.name"),
                ) {
                    Some(b) => b,
                    None => return,
                };

                // Decoupled lookup: read the persisted routes fresh each time so
                // we always honour the latest config without touching the Engine.
                let snapshot = RouteSnapshot::load();
                let argv = match reapply_argv(&snapshot, &binary, global.id) {
                    Some(a) => a,
                    None => return, // no remembered route — never create one
                };

                let args: Vec<&str> = argv.iter().map(String::as_str).collect();
                let mut runner = RealRunner;
                match runner.run("pw-metadata", &args) {
                    Ok(out) if out.status == 0 => {
                        eprintln!(
                            "route-watcher: re-applied route {binary} -> {} (node {})",
                            snapshot.target_for(&binary).unwrap_or(""),
                            global.id
                        );
                    }
                    Ok(out) => eprintln!(
                        "route-watcher: pw-metadata exit {} for {binary} (node {}): {}",
                        out.status, global.id, out.stderr
                    ),
                    Err(e) => {
                        eprintln!("route-watcher: pw-metadata spawn failed for {binary}: {e}")
                    }
                }
            })
            .register();

        // Blocks until a Terminate message triggers mainloop.quit().
        mainloop.run();
        Ok(())
    }
}

#[cfg(feature = "pw-watcher")]
pub use live::RouteWatcher;

// ─────────────────────────────────────────────────────────────────────────────
// No-op stub when the live watcher is not compiled in. Keeps the daemon's
// lifecycle wiring identical regardless of feature selection.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(not(feature = "pw-watcher"))]
pub struct RouteWatcher;

#[cfg(not(feature = "pw-watcher"))]
impl RouteWatcher {
    /// No-op when built without `pw-watcher`. Logs once so it is obvious in the
    /// journal that remembered-route re-application is NOT active.
    pub fn start() -> Self {
        eprintln!(
            "daemon: route watcher DISABLED (built without the `pw-watcher` feature); \
             remembered routes will not be re-applied on stream resume. Rebuild the \
             daemon with `--features pw-watcher` (needs pipewire-devel + clang) to enable."
        );
        RouteWatcher
    }

    /// No-op stop.
    pub fn stop(self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use arctis_config::{Config, Profile, RouteConfig};

    fn cfg_with_routes(routes: Vec<(&str, &str)>) -> Config {
        let mut cfg = Config::default_config();
        let active = cfg.active_profile.clone();
        let profile: &mut Profile = cfg.profile_mut(&active).expect("active profile exists");
        profile.routes = routes
            .into_iter()
            .map(|(b, s)| RouteConfig {
                app_binary: b.to_string(),
                target_sink: s.to_string(),
            })
            .collect();
        cfg
    }

    #[test]
    fn snapshot_from_config_indexes_active_profile_routes() {
        let cfg = cfg_with_routes(vec![
            ("vivaldi", "Arctis_Media"),
            ("Discord", "Arctis_Chat"),
        ]);
        let snap = RouteSnapshot::from_config(&cfg);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap.target_for("vivaldi"), Some("Arctis_Media"));
        assert_eq!(snap.target_for("Discord"), Some("Arctis_Chat"));
        assert_eq!(snap.target_for("firefox"), None);
    }

    #[test]
    fn snapshot_empty_when_no_routes() {
        let cfg = cfg_with_routes(vec![]);
        let snap = RouteSnapshot::from_config(&cfg);
        assert!(snap.is_empty());
        assert_eq!(snap.target_for("anything"), None);
    }

    // ── stream_app_binary: props -> identifier extraction ────────────────────

    #[test]
    fn stream_app_binary_prefers_process_binary() {
        let got = stream_app_binary(
            Some("Stream/Output/Audio"),
            Some("vivaldi"),
            Some("Vivaldi"),
        );
        assert_eq!(got.as_deref(), Some("vivaldi"));
    }

    #[test]
    fn stream_app_binary_falls_back_to_app_name() {
        let got = stream_app_binary(Some("Stream/Output/Audio"), None, Some("Discord"));
        assert_eq!(got.as_deref(), Some("Discord"));
    }

    #[test]
    fn stream_app_binary_blank_binary_falls_back_to_name() {
        let got = stream_app_binary(Some("Stream/Output/Audio"), Some("   "), Some("Spotify"));
        assert_eq!(got.as_deref(), Some("Spotify"));
    }

    #[test]
    fn stream_app_binary_accepts_class_with_suffix() {
        // PipeWire sometimes appends suffixes (e.g. "Stream/Output/Audio/...").
        let got = stream_app_binary(Some("Stream/Output/Audio/Music"), Some("foobar"), None);
        assert_eq!(got.as_deref(), Some("foobar"));
    }

    #[test]
    fn stream_app_binary_rejects_non_stream_class() {
        assert_eq!(
            stream_app_binary(Some("Audio/Sink"), Some("vivaldi"), None),
            None
        );
        assert_eq!(
            stream_app_binary(Some("Stream/Input/Audio"), Some("vivaldi"), None),
            None
        );
    }

    #[test]
    fn stream_app_binary_none_when_no_class_or_identifier() {
        assert_eq!(stream_app_binary(None, Some("vivaldi"), None), None);
        assert_eq!(
            stream_app_binary(Some("Stream/Output/Audio"), None, None),
            None
        );
        assert_eq!(
            stream_app_binary(Some("Stream/Output/Audio"), Some(""), Some("  ")),
            None
        );
    }

    // ── reapply_argv: the "remembered route -> exact pw-metadata argv" decision

    #[test]
    fn reapply_argv_builds_exact_metadata_argv_for_remembered_route() {
        let snap = RouteSnapshot::from_config(&cfg_with_routes(vec![("vivaldi", "Arctis_Media")]));
        let argv = reapply_argv(&snap, "vivaldi", 73).expect("remembered route -> argv");
        // Must equal the existing move_stream_argv shape (G1 reuse).
        assert_eq!(
            argv,
            vec!["-n", "default", "73", "target.object", "Arctis_Media"]
        );
    }

    #[test]
    fn reapply_argv_none_for_unremembered_binary() {
        let snap = RouteSnapshot::from_config(&cfg_with_routes(vec![("vivaldi", "Arctis_Media")]));
        assert!(reapply_argv(&snap, "firefox", 73).is_none());
    }

    #[test]
    fn reapply_argv_matches_move_stream_argv_exactly() {
        // Belt-and-braces: the watcher's argv is byte-identical to the manual
        // move path's argv, so the watcher cannot diverge from `routing.rs`.
        let snap = RouteSnapshot::from_config(&cfg_with_routes(vec![("Discord", "Arctis_Chat")]));
        let got = reapply_argv(&snap, "Discord", 88).unwrap();
        let want = move_stream_argv("88", "Arctis_Chat").unwrap();
        assert_eq!(got, want);
    }
}
