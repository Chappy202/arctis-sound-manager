# Sonar-style Mixer Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the manual app-routing form with automatic live discovery of running apps + drag-and-drop routing, in a full SteelSeries-Sonar-style mixer (Master strip, default Aux + Mic strips, CHATMIX slider), while keeping per-channel output-device selection and custom virtual channels.

**Architecture:** Engine-first. New pure stream-parsing logic in `crates/audio`, exposed through new engine methods, new `Request`/`Response`/`Event` protocol variants, daemon handlers, CLI subcommands, Tauri commands, and finally the Svelte GUI. Live freshness comes from a src-tauri poll task emitting a `streams-changed` Tauri event (mirroring the existing `state-changed` and `levels` tasks). Every capability lands in all layers in the same phase (GUI↔CLI↔daemon parity is a hard rule).

**Tech Stack:** Rust (Cargo workspace: `domain`, `device`, `audio`, `config`, `engine`, `client`, `cli`), Tauri v2 (`src-tauri`), Svelte 5 runes + Vite + pnpm (`frontend`). Audio via PipeWire subprocesses (`pw-dump`, `pw-metadata`, `wpctl`) through the `CommandRunner` trait.

## Global Constraints

- **Linux / PipeWire only**, 48 kHz, no resampling. PipeWire 1.4.11 + WirePlumber 0.5.13.
- **No `tauri` dependency outside `src-tauri`**; `engine` and below stay UI-agnostic (ARCHITECTURE §2).
- **Subprocess-first audio** via the `CommandRunner` trait. **No `pipewire-rs` on the hot path.**
- **Safety (ARCHITECTURE G2):** never write the OLED; never replay unverified firmware opcodes; this plan touches **no** device writes — it is audio-graph + UI only.
- **G7:** typed errors, no `unwrap`/`expect` on runtime paths; surface subprocess failures, never swallow them.
- **G1 reuse:** reuse `Router` (`apply_live`/`set_rule`/`save_persistent`/`clear_live`), `move_stream_argv`, `ChannelManager`, `AudioBackend`, `LevelMeter.svelte`, the output-device dropdown, and the add-channel affordance. Do not duplicate routing logic.
- **Parity:** every new `Request` variant gets a daemon handler arm + an engine method + a Tauri command (registered in `src-tauri/src/lib.rs`) + a CLI subcommand + a frontend ipc wrapper + a GUI control.
- **Discovery correctness:** enumerate **all** `Stream/Output/Audio` nodes — do **not** skip `client.api == "pipewire-pulse"` (that would hide Chrome/Spotify/Discord). Resolve current sink via `PipeWire:Interface:Link` objects. Persist by `application.process.binary`. Move target is the sink `node.name` via `target.object`.
- **Commit after every task** with the exact message shown. Run `cargo test --workspace` (Rust tasks) or `pnpm --dir frontend test` (frontend tasks) before committing.

---

## File Structure

**New files:**
- `crates/audio/src/streams.rs` — pure `pw-dump` parsing → `ParsedStream` list + current-sink resolution via links. Unit-tested with fixtures.
- `crates/audio/tests/fixtures/pw_dump_app_streams.json` — fixture with native + pulse-compat streams + links.
- `crates/engine/src/state.rs` (modify) — `AppStream` snapshot type + new `Event` variants.
- `frontend/src/lib/streams.ts` — pure helpers + `AppStream` type + `onStreamsChanged` listener.
- `frontend/src/lib/stores/streams.ts` — Svelte store for the live stream list.
- `frontend/src/lib/components/AppPill.svelte` — one draggable app pill.
- `frontend/src/lib/components/MasterStrip.svelte` — Master volume/mute + Apps-to-be-routed tray.
- `frontend/src/lib/components/MicStrip.svelte` — Mic level/mute + link to Mic page.
- `frontend/src/lib/components/ChatmixSlider.svelte` — Game↔Chat balance slider.

**Modified files:**
- `crates/client/src/protocol.rs` — `Request` variants + `Response.streams` + round-trip tests.
- `crates/cli/src/daemon.rs` — `handle_request` arms.
- `crates/engine/src/engine.rs` — `list_streams`, `move_stream`, master/chatmix/default-sink methods, channel auto-seed.
- `crates/config/src/schema.rs` — `Profile` master/chatmix/default-sink fields; default channel set + Aux.
- `crates/cli/src/main.rs` — `streams`/`master`/`chatmix`/`default-sink` subcommands + parser tests.
- `src-tauri/src/commands.rs` — new commands.
- `src-tauri/src/lib.rs` — register commands + `streams-changed` poll task.
- `frontend/src/lib/ipc.ts` — ipc wrappers + `AppStream` type + `onStreamsChanged`.
- `frontend/src/lib/components/MixerPage.svelte` — new layout.
- `frontend/src/lib/components/ChannelStrip.svelte` — app-pill drop area.
- `frontend/src/lib/components/RouteList.svelte` — collapse to view-only "Remembered routes".
- `frontend/src/styles/tokens.css` — channel accents + drag highlight.

---

# PHASE 1 — Stream discovery core (headless)

## Task 1: `ParsedStream` parsing in `crates/audio/src/streams.rs`

**Files:**
- Create: `crates/audio/src/streams.rs`
- Create: `crates/audio/tests/fixtures/pw_dump_app_streams.json`
- Modify: `crates/audio/src/lib.rs` (add `pub mod streams;` and re-export)

**Interfaces:**
- Produces: `pub struct ParsedStream { pub id: u32, pub binary: String, pub app_name: String, pub pid: Option<u32>, pub icon_name: Option<String>, pub media_name: Option<String>, pub sink_node_name: Option<String> }` and `pub fn parse_app_streams(pw_dump_json: &str) -> Result<Vec<ParsedStream>, AudioError>`.
- Consumes: `crate::error::AudioError` (existing).

- [ ] **Step 1: Create the fixture**

Create `crates/audio/tests/fixtures/pw_dump_app_streams.json`:

```json
[
  { "id": 40, "type": "PipeWire:Interface:Node",
    "info": { "props": {
      "media.class": "Audio/Sink", "node.name": "Arctis_Game" } } },
  { "id": 41, "type": "PipeWire:Interface:Node",
    "info": { "props": {
      "media.class": "Audio/Sink", "node.name": "Arctis_Chat" } } },
  { "id": 70, "type": "PipeWire:Interface:Node",
    "info": { "props": {
      "media.class": "Stream/Output/Audio",
      "application.name": "Firefox",
      "application.process.binary": "firefox",
      "application.process.id": "1234",
      "application.icon-name": "firefox",
      "media.name": "YouTube" } } },
  { "id": 71, "type": "PipeWire:Interface:Node",
    "info": { "props": {
      "media.class": "Stream/Output/Audio",
      "client.api": "pipewire-pulse",
      "application.name": "Spotify",
      "application.process.binary": "spotify",
      "application.process.id": "5678",
      "media.name": "Some Song" } } },
  { "id": 72, "type": "PipeWire:Interface:Node",
    "info": { "props": {
      "media.class": "Stream/Input/Audio",
      "application.name": "OBS", "application.process.binary": "obs" } } },
  { "id": 900, "type": "PipeWire:Interface:Link",
    "info": { "output-node-id": 70, "input-node-id": 40 } },
  { "id": 901, "type": "PipeWire:Interface:Link",
    "info": { "output-node-id": 70, "input-node-id": 40 } }
]
```

- [ ] **Step 2: Write the failing test**

Create `crates/audio/src/streams.rs` with only the test module body first:

```rust
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
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p arctis-audio streams:: 2>&1 | tail -20`
Expected: FAIL — `cannot find function parse_app_streams`.

- [ ] **Step 4: Implement `parse_app_streams`**

Insert this **above** the `#[cfg(test)]` module in `crates/audio/src/streams.rs`:

```rust
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
```

- [ ] **Step 5: Wire the module**

In `crates/audio/src/lib.rs` add `pub mod streams;` near the other `pub mod` lines, and add `pub use streams::{parse_app_streams, ParsedStream};` near the other re-exports.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p arctis-audio streams:: 2>&1 | tail -20`
Expected: PASS (5 tests).

- [ ] **Step 7: Commit**

```bash
git add crates/audio/src/streams.rs crates/audio/src/lib.rs crates/audio/tests/fixtures/pw_dump_app_streams.json
git commit -m "feat(audio): pure pw-dump app-stream parsing with link-based sink resolution"
```

---

## Task 2: `AppStream` snapshot + `Engine::list_streams`

**Files:**
- Modify: `crates/engine/src/state.rs` (add `AppStream`)
- Modify: `crates/engine/src/engine.rs` (add `list_streams`)
- Test: inline `#[cfg(test)]` in `crates/engine/src/engine.rs`

**Interfaces:**
- Produces: `pub struct AppStream { pub id: u32, pub binary: String, pub app_name: String, pub pid: Option<u32>, pub icon_name: Option<String>, pub media_name: Option<String>, pub current_channel: Option<String>, pub routed: bool }` (in `state.rs`); `pub fn list_streams(&mut self) -> Result<Vec<AppStream>, EngineError>` (in `engine.rs`).
- Consumes: `arctis_audio::parse_app_streams`, `self.runner.run("pw-dump", &[])`, the active profile's channels (`node_name` → `id` map) and `routes`.

- [ ] **Step 1: Add the `AppStream` type**

In `crates/engine/src/state.rs`, after the `EngineState` struct, add:

```rust
/// One running application audio stream, resolved to a channel id, for the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppStream {
    pub id: u32,
    pub binary: String,
    pub app_name: String,
    pub pid: Option<u32>,
    pub icon_name: Option<String>,
    pub media_name: Option<String>,
    /// Resolved channel id, or None = unrouted (shown in the Master tray).
    pub current_channel: Option<String>,
    /// True when a persistent routing rule exists for this binary.
    pub routed: bool,
}
```

Ensure `state.rs` re-exports it if it has an explicit export list; otherwise it is reachable as `crate::state::AppStream`. Add `pub use state::AppStream;` to `crates/engine/src/lib.rs` next to the other `state::` re-exports.

- [ ] **Step 2: Write the failing test**

In `crates/engine/src/engine.rs` test module, add (uses the existing `MockRunner` helpers; mirror how other engine tests build an `Engine`):

```rust
#[test]
fn list_streams_maps_sink_to_channel_and_marks_routed() {
    // Active profile: game/chat/media (node_name Arctis_Game/Chat/Media).
    let mut cfg = make_config_no_eq_no_routes();
    // Add a persistent route so `routed` flips for firefox.
    cfg.profiles[0].routes = vec![arctis_config::RouteConfig {
        app_binary: "firefox".into(),
        target_sink: "Arctis_Game".into(),
    }];
    let dump = include_str!("../../audio/tests/fixtures/pw_dump_app_streams.json");
    let runner = arctis_audio::MockRunner::new().with_output(0, dump, ""); // pw-dump
    let mut engine = Engine::new(runner, cfg);
    let streams = engine.list_streams().unwrap();

    let ff = streams.iter().find(|s| s.binary == "firefox").unwrap();
    assert_eq!(ff.current_channel.as_deref(), Some("game")); // Arctis_Game → game
    assert!(ff.routed, "firefox has a persistent rule");

    let sp = streams.iter().find(|s| s.binary == "spotify").unwrap();
    assert_eq!(sp.current_channel, None, "unlinked spotify is unrouted");
    assert!(!sp.routed);
}
```

> If `make_config_no_eq_no_routes` / `MockRunner` import paths differ in the test module, match the surrounding tests' exact imports. Do **not** invent new helpers.

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p arctis-engine list_streams_maps 2>&1 | tail -20`
Expected: FAIL — no method `list_streams`.

- [ ] **Step 4: Implement `list_streams`**

Add to the `impl<R: CommandRunner> Engine<R>` block in `crates/engine/src/engine.rs` (mirror the `set_route` style for runner + error handling):

```rust
/// Discover running application output streams, resolving each to a channel id
/// (via its linked sink node.name) and flagging those with a persistent route.
/// One `pw-dump` per call; pure mapping otherwise. Read-only (no graph mutation).
pub fn list_streams(&mut self) -> Result<Vec<crate::state::AppStream>, EngineError> {
    // node.name -> channel id, built from the active profile (never hard-coded).
    let (name_to_id, routed_bins): (
        std::collections::HashMap<String, String>,
        std::collections::HashSet<String>,
    ) = {
        let profile = self.config.active()?;
        let map = profile
            .channels
            .iter()
            .map(|c| (c.node_name.clone(), c.id.clone()))
            .collect();
        let routed = profile
            .routes
            .iter()
            .map(|r| r.app_binary.clone())
            .collect();
        (map, routed)
    };

    let out = self.runner.run("pw-dump", &[])?;
    if out.status != 0 {
        return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
            program: "pw-dump".into(),
            status: out.status,
            stderr: out.stderr,
        }));
    }
    let parsed = arctis_audio::parse_app_streams(&out.stdout)?;
    Ok(parsed
        .into_iter()
        .map(|p| crate::state::AppStream {
            current_channel: p
                .sink_node_name
                .as_ref()
                .and_then(|n| name_to_id.get(n).cloned()),
            routed: routed_bins.contains(&p.binary),
            id: p.id,
            binary: p.binary,
            app_name: p.app_name,
            pid: p.pid,
            icon_name: p.icon_name,
            media_name: p.media_name,
        })
        .collect())
}
```

> If `EngineError` lacks an `Audio` variant wrapping `AudioError`, use the same error-conversion pattern other engine methods use for `AudioError` (check the `?` usages on `arctis_audio` calls elsewhere in this file and match them). `parse_app_streams` returns `AudioError`, so `?` must convert it the same way existing audio calls do.

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p arctis-engine list_streams_maps 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/engine/src/state.rs crates/engine/src/engine.rs crates/engine/src/lib.rs
git commit -m "feat(engine): list_streams — discover apps, map sink->channel, flag routed"
```

---

## Task 3: `Engine::move_stream` (live move + persist)

**Files:**
- Modify: `crates/engine/src/engine.rs`
- Test: inline `#[cfg(test)]` in `crates/engine/src/engine.rs`

**Interfaces:**
- Produces: `pub fn move_stream(&mut self, stream: &str, channel_id: &str) -> Result<(), EngineError>` where `stream` is a node id (as string) **or** a binary; `channel_id` is a channel id.
- Consumes: `self.list_streams()`, `arctis_audio::routing::move_stream_argv`, `Router::{set_rule, save_persistent}`, `RouteRule`, the existing `set_route` precedent.

- [ ] **Step 1: Write the failing test**

In the engine test module:

```rust
#[test]
fn move_stream_by_id_persists_rule_and_moves_live() {
    let cfg = make_config_no_eq_no_routes();
    let dump = include_str!("../../audio/tests/fixtures/pw_dump_app_streams.json");
    // 1) list_streams pw-dump, 2) pw-metadata move
    let runner = arctis_audio::MockRunner::new()
        .with_output(0, dump, "")
        .with_output(0, "", "");
    let mut engine = Engine::new(runner, cfg);
    engine.move_stream("70", "chat").unwrap(); // firefox node id 70 -> chat

    // Persistent route recorded in active profile.
    let active = engine.config_ref().active().unwrap();
    assert!(
        active.routes.iter().any(|r| r.app_binary == "firefox" && r.target_sink == "Arctis_Chat"),
        "expected persisted firefox->Arctis_Chat route: {:?}", active.routes
    );
}

#[test]
fn move_stream_unknown_channel_errors() {
    let cfg = make_config_no_eq_no_routes();
    let dump = include_str!("../../audio/tests/fixtures/pw_dump_app_streams.json");
    let runner = arctis_audio::MockRunner::new().with_output(0, dump, "");
    let mut engine = Engine::new(runner, cfg);
    assert!(engine.move_stream("70", "nope").is_err());
}
```

> If there is no public `config_ref()`/`config()` accessor returning `&Config`, assert via `engine.state()` and the persisted routes there, or use the existing accessor at `engine.rs:192` (`&self.config`) exposed publicly — match what other tests use.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p arctis-engine move_stream 2>&1 | tail -20`
Expected: FAIL — no method `move_stream`.

- [ ] **Step 3: Implement `move_stream`**

```rust
/// Route a running stream to a channel: resolve channel id -> sink node.name,
/// live-move the specific stream (by node id) via pw-metadata, and persist a
/// binary->sink rule so it sticks next launch. `stream` may be a node id or a
/// binary; the binary is resolved from discovery for persistence.
pub fn move_stream(&mut self, stream: &str, channel_id: &str) -> Result<(), EngineError> {
    // Resolve channel -> sink node.name from the active profile.
    let sink = {
        let profile = self.config.active()?;
        profile
            .channels
            .iter()
            .find(|c| c.id == channel_id)
            .map(|c| c.node_name.clone())
            .ok_or_else(|| EngineError::BadRequest(format!("unknown channel: {channel_id}")))?
    };

    // Find the target stream (by node id string or by binary) for its id + binary.
    let streams = self.list_streams()?;
    let target = streams
        .iter()
        .find(|s| s.id.to_string() == stream || s.binary == stream)
        .ok_or_else(|| EngineError::BadRequest(format!("no running stream: {stream}")))?
        .clone();

    // Live move the exact stream node id.
    let argv = arctis_audio::routing::move_stream_argv(&target.id.to_string(), &sink)?;
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let out = self.runner.run("pw-metadata", &args)?;
    if out.status != 0 {
        return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
            program: "pw-metadata".into(),
            status: out.status,
            stderr: out.stderr,
        }));
    }

    // Persist binary -> sink (reuses set_route's persistence path).
    self.set_route(&target.binary, &sink)?;
    Ok(())
}
```

> `set_route` itself emits `RouteSet` and does its own live `apply_live` by binary; calling it after the explicit-id move is intentional (id-move targets the exact instance; `set_route` writes the rule + WirePlumber fragment). If duplicate live moves are undesirable in review, split out the persistence-only part — but the default here is correct and reuses existing code. Match `EngineError::Audio` conversion to the file's actual pattern (see Task 2 note).

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p arctis-engine move_stream 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/engine.rs
git commit -m "feat(engine): move_stream — live move by node id + persist binary rule"
```

---

## Task 4: Protocol verbs `ListStreams` + `MoveStream` + `Response.streams`

**Files:**
- Modify: `crates/client/src/protocol.rs`
- Test: inline `#[cfg(test)]` in `crates/client/src/protocol.rs`

**Interfaces:**
- Produces: `Request::ListStreams`, `Request::MoveStream { stream: String, channel: String }`, `Response.streams: Option<Vec<arctis_engine::AppStream>>`, and `Response::ok_with_streams(streams)`.
- Consumes: `arctis_engine::AppStream`.

- [ ] **Step 1: Write the failing tests**

In the `protocol.rs` test module:

```rust
#[test]
fn parse_list_streams_wire_tag() {
    let req: Request = serde_json::from_str(r#"{"cmd":"list-streams"}"#).unwrap();
    assert_eq!(req, Request::ListStreams);
}

#[test]
fn parse_move_stream_wire_tag() {
    let req: Request =
        serde_json::from_str(r#"{"cmd":"move-stream","stream":"70","channel":"chat"}"#).unwrap();
    assert_eq!(req, Request::MoveStream { stream: "70".into(), channel: "chat".into() });
}

#[test]
fn request_move_stream_round_trips() {
    let req = Request::MoveStream { stream: "firefox".into(), channel: "game".into() };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("move-stream"), "cmd tag must be move-stream: {json}");
    let back: Request = serde_json::from_str(&json).unwrap();
    assert_eq!(req, back);
}

#[test]
fn response_ok_with_streams_round_trips() {
    let resp = Response::ok_with_streams(vec![]);
    let json = serde_json::to_string(&resp).unwrap();
    let back: Response = serde_json::from_str(&json).unwrap();
    assert!(back.ok);
    assert!(back.streams.is_some());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p arctis-client move_stream 2>&1 | tail -20`
Expected: FAIL — variants/field/constructor missing.

- [ ] **Step 3: Add the variants, field, and constructor**

In `Request` (before the closing `}` of the enum):

```rust
    /// List running application output streams resolved to channel ids.
    ListStreams,
    /// Move a running stream to a channel (live + persistent). `stream` is a
    /// node id or a binary; `channel` is a channel id.
    MoveStream {
        stream: String,
        channel: String,
    },
```

In `Response` struct, add a field next to `text`:

```rust
    /// Stream list payload. Populated only for ListStreams responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streams: Option<Vec<arctis_engine::AppStream>>,
```

Add `streams: None` to **every** existing `Response { ... }` literal in this file (the `ok_with_state`, `ok_with_text`, `err`, `ok_with_coexist_report`, `ok_with_coexist_result` constructors and the test literal at `response_ok_with_state_round_trips`). Then add the constructor:

```rust
impl Response {
    pub fn ok_with_streams(streams: Vec<arctis_engine::AppStream>) -> Self {
        Self {
            ok: true,
            state: None,
            error: None,
            text: None,
            streams: Some(streams),
            coexist_report: None,
            coexist_result: None,
        }
    }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p arctis-client 2>&1 | tail -20`
Expected: PASS (new + existing tests).

- [ ] **Step 5: Commit**

```bash
git add crates/client/src/protocol.rs
git commit -m "feat(protocol): ListStreams + MoveStream verbs + Response.streams"
```

---

## Task 5: Daemon handlers + CLI `streams` subcommand

**Files:**
- Modify: `crates/cli/src/daemon.rs` (`handle_request`)
- Modify: `crates/cli/src/main.rs` (`Command::Streams` + handler + parser tests)

**Interfaces:**
- Consumes: `engine.list_streams()`, `engine.move_stream(..)`, `Request::ListStreams`, `Request::MoveStream`, `Response::ok_with_streams`.

- [ ] **Step 1: Add daemon handler arms**

In `crates/cli/src/daemon.rs` `handle_request`, add arms in the `match req`:

```rust
        Request::ListStreams => match engine.list_streams() {
            Ok(streams) => Response::ok_with_streams(streams),
            Err(e) => Response::err(e.to_string()),
        },
        Request::MoveStream { stream, channel } => match engine.move_stream(&stream, &channel) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
```

- [ ] **Step 2: Add the CLI subcommand enum + dispatch**

In `crates/cli/src/main.rs`, add to the `Command` enum:

```rust
    /// Live application audio streams: list and move between channels.
    Streams {
        #[command(subcommand)]
        action: StreamsAction,
    },
```

Add the action enum near the other `#[derive(Subcommand, Debug)]` enums:

```rust
#[derive(Subcommand, Debug)]
enum StreamsAction {
    /// List running app streams with their current channel.
    List,
    /// Move a running stream to a channel: `streams move <stream> <channel>`.
    Move {
        /// Stream node id or app binary.
        stream: String,
        /// Target channel id: game | chat | media | aux | ...
        channel: String,
    },
}
```

Add the dispatch arm in the `match command` block (mirror `ChannelCmd::Volume` which uses `daemon::send_request`):

```rust
        Command::Streams { action } => match action {
            StreamsAction::List => {
                match daemon::send_request(&daemon::Request::ListStreams) {
                    Ok(resp) if resp.ok => {
                        let streams = resp.streams.unwrap_or_default();
                        if streams.is_empty() {
                            println!("no running app streams");
                        } else {
                            for s in &streams {
                                let ch = s.current_channel.as_deref().unwrap_or("(unrouted)");
                                let pin = if s.routed { " [pinned]" } else { "" };
                                println!("{:>5}  {:<20} -> {}{}", s.id, s.app_name, ch, pin);
                            }
                        }
                        ExitCode::SUCCESS
                    }
                    Ok(resp) => {
                        eprintln!("error: {}", resp.error.unwrap_or_else(|| "unknown".into()));
                        ExitCode::FAILURE
                    }
                    Err(e) => {
                        eprintln!("error sending request: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            StreamsAction::Move { stream, channel } => {
                let req = daemon::Request::MoveStream {
                    stream: stream.clone(),
                    channel: channel.clone(),
                };
                match daemon::send_request(&req) {
                    Ok(resp) if resp.ok => {
                        println!("moved stream '{stream}' -> {channel}");
                        ExitCode::SUCCESS
                    }
                    Ok(resp) => {
                        eprintln!("error: {}", resp.error.unwrap_or_else(|| "unknown".into()));
                        ExitCode::FAILURE
                    }
                    Err(e) => {
                        eprintln!("error sending request: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
        },
```

- [ ] **Step 3: Add parser tests**

In the `main.rs` test module (mirror existing parser tests near line 2034):

```rust
#[test]
fn streams_list_parses() {
    let cli = super::Cli::try_parse_from(["asm-cli", "streams", "list"]).unwrap();
    assert!(matches!(
        cli.command,
        super::Command::Streams { action: super::StreamsAction::List }
    ));
}

#[test]
fn streams_move_parses() {
    let cli = super::Cli::try_parse_from(["asm-cli", "streams", "move", "70", "chat"]).unwrap();
    match cli.command {
        super::Command::Streams { action: super::StreamsAction::Move { stream, channel } } => {
            assert_eq!(stream, "70");
            assert_eq!(channel, "chat");
        }
        other => panic!("unexpected: {other:?}"),
    }
}
```

> Use the exact `Cli`/parse entry the surrounding tests use (e.g. `Cli::try_parse_from` vs `Command::try_parse_from`). Match the neighbours.

- [ ] **Step 4: Run + verify**

Run: `cargo test -p arctis-cli streams_ 2>&1 | tail -20` then `cargo build -p arctis-cli 2>&1 | tail -5`
Expected: tests PASS; build OK.

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/daemon.rs crates/cli/src/main.rs
git commit -m "feat(cli): streams list/move subcommands + daemon handlers"
```

---

## Task 6: Phase 1 workspace gate

- [ ] **Step 1: Full build + test**

Run: `cargo test --workspace 2>&1 | tail -25`
Expected: PASS, no warnings-as-errors. Fix any breakage before continuing.

- [ ] **Step 2: Commit (only if fixes were needed)**

```bash
git add -A && git commit -m "test: green workspace after stream-discovery core"
```

---

# PHASE 2 — Mixer model: Master, ChatMix, default-sink, Aux (headless)

## Task 7: Config schema — Master/ChatMix/default-sink fields + Aux default + auto-seed

**Files:**
- Modify: `crates/config/src/schema.rs`
- Test: inline `#[cfg(test)]` in `crates/config/src/schema.rs`

**Interfaces:**
- Produces: `Profile.master_volume_db: f32`, `Profile.master_mute: bool`, `Profile.chatmix_position: i64`, `Profile.default_sink_channel: Option<String>`; an `aux` channel in `default_config()`; `pub fn ensure_standard_channels(&mut self)` on `Config`.
- Consumes: existing `ChannelConfig`, `Profile`.

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn default_config_includes_aux_channel() {
    let cfg = Config::default_config();
    let ids: Vec<&str> = cfg.profiles[0].channels.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, vec!["game", "chat", "media", "aux"]);
}

#[test]
fn ensure_standard_channels_adds_missing_aux_preserving_custom() {
    let mut cfg = Config::default_config();
    // Simulate an old profile without aux but with a custom channel.
    cfg.profiles[0].channels.retain(|c| c.id != "aux");
    cfg.profiles[0].channels.push(ChannelConfig {
        id: "stream".into(), node_name: "Arctis_Stream".into(),
        description: "Custom".into(), output_device: None,
        eq: vec![], volume_db: 0.0, muted: false,
    });
    cfg.ensure_standard_channels();
    let ids: Vec<String> = cfg.profiles[0].channels.iter().map(|c| c.id.clone()).collect();
    assert!(ids.contains(&"aux".to_string()), "aux seeded: {ids:?}");
    assert!(ids.contains(&"stream".to_string()), "custom preserved: {ids:?}");
}

#[test]
fn profile_new_fields_default_sane() {
    let cfg = Config::default_config();
    let p = &cfg.profiles[0];
    assert_eq!(p.master_volume_db, 0.0);
    assert!(!p.master_mute);
    assert_eq!(p.chatmix_position, 4); // center of 0..=9 (game/chat balanced)
    assert_eq!(p.default_sink_channel, None);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p arctis-config default_config_includes_aux 2>&1 | tail -20`
Expected: FAIL.

- [ ] **Step 3: Add fields + Aux + seeding**

In the `Profile` struct add (with `#[serde(default)]` for back-compat):

```rust
    /// Master output gain in dB applied to the headset output. 0.0 = unity.
    #[serde(default)]
    pub master_volume_db: f32,
    /// Master mute (mutes the headset output).
    #[serde(default)]
    pub master_mute: bool,
    /// ChatMix position 0..=9 (0 = full chat, 9 = full game, 4 = balanced).
    #[serde(default = "default_chatmix")]
    pub chatmix_position: i64,
    /// Channel id whose sink is set as the system default output, or None.
    #[serde(default)]
    pub default_sink_channel: Option<String>,
```

Add near `default_true`:

```rust
fn default_chatmix() -> i64 {
    4
}
```

In `default_config()` push a fourth channel after `media`:

```rust
            ChannelConfig {
                id: "aux".to_string(),
                node_name: "Arctis_Aux".to_string(),
                description: "Aux audio channel".to_string(),
                output_device: None,
                eq: Vec::new(),
                volume_db: 0.0,
                muted: false,
            },
```

Add to `impl Config` (define the standard set once, reuse for seeding):

```rust
    /// Ensure every profile has the standard channels (game, chat, media, aux),
    /// preserving any custom channels and existing settings. Idempotent.
    pub fn ensure_standard_channels(&mut self) {
        const STANDARD: &[(&str, &str, &str)] = &[
            ("game", "Arctis_Game", "Game audio channel"),
            ("chat", "Arctis_Chat", "Chat audio channel"),
            ("media", "Arctis_Media", "Media audio channel"),
            ("aux", "Arctis_Aux", "Aux audio channel"),
        ];
        for profile in &mut self.profiles {
            for (id, node, desc) in STANDARD {
                if !profile.channels.iter().any(|c| c.id == *id) {
                    profile.channels.push(ChannelConfig {
                        id: id.to_string(),
                        node_name: node.to_string(),
                        description: desc.to_string(),
                        output_device: None,
                        eq: Vec::new(),
                        volume_db: 0.0,
                        muted: false,
                    });
                }
            }
        }
    }
```

> If any existing `Profile { ... }` literal in `schema.rs` (e.g. inside `default_config`) does not use `..Default::default()`, add the four new fields to it explicitly: `master_volume_db: 0.0, master_mute: false, chatmix_position: 4, default_sink_channel: None`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p arctis-config 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/config/src/schema.rs
git commit -m "feat(config): master/chatmix/default-sink fields + Aux default + ensure_standard_channels"
```

---

## Task 8: Engine — call `ensure_standard_channels` on load + master/chatmix/default-sink methods

**Files:**
- Modify: `crates/engine/src/engine.rs`
- Modify: `crates/engine/src/state.rs` (Event variants + EngineState fields)
- Test: inline `#[cfg(test)]` in `crates/engine/src/engine.rs`

**Interfaces:**
- Produces: `pub fn set_master_volume(&mut self, db: f32) -> Result<(), EngineError>`, `set_master_mute(bool)`, `set_chatmix(i64)`, `set_default_sink_channel(Option<String>)`; `EngineState` gains `master_volume_db`, `master_mute`, `chatmix_position`, `default_sink_channel`. New `Event` variants `MasterVolumeSet { volume_db }`, `MasterMuteSet { muted }`, `ChatmixSet { position }`, `DefaultSinkChannelSet { channel }`.
- Consumes: existing `apply_dial_balance` logic in `crates/cli/src/dial.rs` — **NOTE:** `dial.rs` lives in the CLI crate. Move the pure mapping `dial_to_channel_volumes` into the engine (or call `set_channel_volume` directly here) so the engine does not depend on the CLI. See Step 3.

- [ ] **Step 1: Add EngineState fields + Event variants**

In `crates/engine/src/state.rs`, add to `EngineState` (with `#[serde(default)]`):

```rust
    #[serde(default)]
    pub master_volume_db: f32,
    #[serde(default)]
    pub master_mute: bool,
    #[serde(default)]
    pub chatmix_position: i64,
    #[serde(default)]
    pub default_sink_channel: Option<String>,
```

Add to the `Event` enum:

```rust
    MasterVolumeSet { volume_db: f32 },
    MasterMuteSet { muted: bool },
    ChatmixSet { position: i64 },
    DefaultSinkChannelSet { channel: Option<String> },
```

In `engine.rs` `state()` builder (around line 421 where `active_profile`/`profiles` are set), populate the four new fields from the active profile (fallback to defaults when `active` is `Err`):

```rust
            master_volume_db: active.as_ref().map(|p| p.master_volume_db).unwrap_or(0.0),
            master_mute: active.as_ref().map(|p| p.master_mute).unwrap_or(false),
            chatmix_position: active.as_ref().map(|p| p.chatmix_position).unwrap_or(4),
            default_sink_channel: active.as_ref().and_then(|p| p.default_sink_channel.clone()),
```

> Match the exact name of the local holding the active profile in `state()` (the snippet at line 213 uses `let active = self.config.active().ok();`). Use that binding.

- [ ] **Step 2: Write failing tests**

```rust
#[test]
fn engine_new_seeds_standard_channels_for_old_profile() {
    let mut cfg = make_config_no_eq_no_routes(); // game/chat/media, no aux
    cfg.profiles[0].channels.retain(|c| c.id != "aux");
    let engine = Engine::new(arctis_audio::MockRunner::new(), cfg);
    let st = engine.state();
    assert!(st.channels.iter().any(|c| c.id == "aux"), "aux auto-seeded on load");
}

#[test]
fn set_master_volume_persists_and_reports() {
    let cfg = make_config_no_eq_no_routes();
    // wpctl call for the gain (status 0).
    let runner = arctis_audio::MockRunner::new().with_output(0, "", "");
    let mut engine = Engine::new(runner, cfg);
    engine.set_master_volume(-6.0).unwrap();
    assert_eq!(engine.state().master_volume_db, -6.0);
}

#[test]
fn set_chatmix_updates_game_chat_volumes() {
    let cfg = make_config_no_eq_no_routes();
    // chatmix applies two channel-volume changes (game + chat) -> 2 backend runs.
    let runner = arctis_audio::MockRunner::new()
        .with_output(0, "", "")
        .with_output(0, "", "");
    let mut engine = Engine::new(runner, cfg);
    engine.set_chatmix(9).unwrap(); // full game
    assert_eq!(engine.state().chatmix_position, 9);
}
```

- [ ] **Step 3: Implement the methods + the load-time seed**

In `Engine::new` (around line 97), after the config is stored, call seeding. Find the line that assigns the config into the struct and add immediately after constructing `self`/before returning — simplest is to mutate `config` before storing:

```rust
    pub fn new(runner: R, mut config: Config) -> Self {
        config.ensure_standard_channels();
        // ... rest unchanged, storing `config` ...
```

Add the pure chatmix mapping (copy of the CLI `dial_to_channel_volumes`, kept in the engine so the engine has no CLI dependency) near the other helpers in `engine.rs`:

```rust
/// Map a ChatMix position 0..=9 to (game_db, chat_db). 4 = balanced (0,0);
/// 9 = full game (chat at FULL_ATTEN); 0 = full chat (game at FULL_ATTEN).
const CHATMIX_FULL_ATTEN_DB: f32 = -40.0;
fn chatmix_to_volumes(position: i64) -> (f32, f32) {
    let p = position.clamp(0, 9) as f32;
    let center = 4.5_f32;
    if (p - center).abs() < f32::EPSILON {
        return (0.0, 0.0);
    }
    if p > center {
        // bias toward game: attenuate chat proportionally
        let t = (p - center) / (9.0 - center); // 0..1
        (0.0, CHATMIX_FULL_ATTEN_DB * t)
    } else {
        let t = (center - p) / center; // 0..1
        (CHATMIX_FULL_ATTEN_DB * t, 0.0)
    }
}
```

Add the methods to the `Engine` impl (mirror `set_channel_volume`'s persist-then-apply-then-emit shape):

```rust
/// Set the master output gain (dB) on the headset output via wpctl, persist,
/// and emit MasterVolumeSet.
pub fn set_master_volume(&mut self, db: f32) -> Result<(), EngineError> {
    {
        let name = self.config.active_profile.clone();
        let p = self.config.profile_mut(&name).ok_or_else(|| {
            EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
        })?;
        p.master_volume_db = db;
    }
    self.save_config()?;
    // wpctl set-volume on @DEFAULT_AUDIO_SINK@ using a linear factor.
    let linear = 10f32.powf(db / 20.0);
    let factor = format!("{linear:.4}");
    let out = self
        .runner
        .run("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &factor])?;
    if out.status != 0 {
        return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
            program: "wpctl".into(),
            status: out.status,
            stderr: out.stderr,
        }));
    }
    self.emit(Event::MasterVolumeSet { volume_db: db });
    Ok(())
}

/// Mute/unmute the master output via wpctl, persist, emit MasterMuteSet.
pub fn set_master_mute(&mut self, muted: bool) -> Result<(), EngineError> {
    {
        let name = self.config.active_profile.clone();
        let p = self.config.profile_mut(&name).ok_or_else(|| {
            EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
        })?;
        p.master_mute = muted;
    }
    self.save_config()?;
    let arg = if muted { "1" } else { "0" };
    let out = self
        .runner
        .run("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", arg])?;
    if out.status != 0 {
        return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
            program: "wpctl".into(),
            status: out.status,
            stderr: out.stderr,
        }));
    }
    self.emit(Event::MasterMuteSet { muted });
    Ok(())
}

/// Set ChatMix position (Game<->Chat balance); applies derived volumes to the
/// game and chat channels, persists position, emits ChatmixSet.
pub fn set_chatmix(&mut self, position: i64) -> Result<(), EngineError> {
    let pos = position.clamp(0, 9);
    {
        let name = self.config.active_profile.clone();
        let p = self.config.profile_mut(&name).ok_or_else(|| {
            EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
        })?;
        p.chatmix_position = pos;
    }
    let (game_db, chat_db) = chatmix_to_volumes(pos);
    // Reuse set_channel_volume (live + persist) for each side; ignore "channel
    // not found" so profiles lacking game/chat don't hard-fail.
    let _ = self.set_channel_volume("game", game_db);
    let _ = self.set_channel_volume("chat", chat_db);
    self.save_config()?;
    self.emit(Event::ChatmixSet { position: pos });
    Ok(())
}

/// Set (or clear) which channel's sink is the system default output. When set,
/// runs `wpctl set-default` on that sink. Persists + emits.
pub fn set_default_sink_channel(&mut self, channel: Option<String>) -> Result<(), EngineError> {
    // Validate + resolve sink before mutating.
    let sink = match &channel {
        Some(id) => {
            let p = self.config.active()?;
            Some(
                p.channels
                    .iter()
                    .find(|c| &c.id == id)
                    .map(|c| c.node_name.clone())
                    .ok_or_else(|| EngineError::BadRequest(format!("unknown channel: {id}")))?,
            )
        }
        None => None,
    };
    {
        let name = self.config.active_profile.clone();
        let p = self.config.profile_mut(&name).ok_or_else(|| {
            EngineError::Config(arctis_config::ConfigError::ProfileNotFound(name.clone()))
        })?;
        p.default_sink_channel = channel.clone();
    }
    self.save_config()?;
    if let Some(sink_name) = sink {
        let out = self.runner.run("wpctl", &["set-default", &sink_name])?;
        if out.status != 0 {
            return Err(EngineError::Audio(arctis_audio::AudioError::NonZeroExit {
                program: "wpctl".into(),
                status: out.status,
                stderr: out.stderr,
            }));
        }
    }
    self.emit(Event::DefaultSinkChannelSet { channel });
    Ok(())
}
```

> `wpctl set-default` takes a numeric object id, not a node.name, in some WirePlumber builds. The fixture/mock test only checks status; the live correctness (name vs id) is an **owner-run validation item** — note it in the PR. If owner testing shows it needs an id, resolve the sink node.name → id via a `pw-dump` lookup here (same parse approach as `streams.rs`). Keep the node.name path as the default; do not block the plan on it.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p arctis-engine 'master\|chatmix\|seeds_standard' 2>&1 | tail -25`
Expected: PASS. Then `cargo test -p arctis-engine 2>&1 | tail -10` (no regressions).

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/engine.rs crates/engine/src/state.rs
git commit -m "feat(engine): master volume/mute, chatmix, default-sink + load-time channel seed"
```

---

## Task 9: Protocol + daemon + CLI for master/chatmix/default-sink

**Files:**
- Modify: `crates/client/src/protocol.rs` (+tests)
- Modify: `crates/cli/src/daemon.rs`
- Modify: `crates/cli/src/main.rs` (+parser tests)

**Interfaces:**
- Produces: `Request::SetMasterVolume { volume_db: f32 }`, `SetMasterMute { muted: bool }`, `SetChatmix { position: i64 }`, `SetDefaultSinkChannel { channel: Option<String> }`. CLI: `master volume <db>`, `master mute <on|off>`, `chatmix <0-9>`, `default-sink set <channel>` / `default-sink clear`.

- [ ] **Step 1: Add protocol variants + round-trip tests**

Add to `Request`:

```rust
    SetMasterVolume { volume_db: f32 },
    SetMasterMute { muted: bool },
    SetChatmix { position: i64 },
    SetDefaultSinkChannel { channel: Option<String> },
```

Add tests mirroring the existing round-trip style:

```rust
#[test]
fn request_set_master_volume_round_trips() {
    let req = Request::SetMasterVolume { volume_db: -6.0 };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("set-master-volume"), "{json}");
    assert_eq!(req, serde_json::from_str::<Request>(&json).unwrap());
}

#[test]
fn request_set_chatmix_round_trips() {
    let req = Request::SetChatmix { position: 7 };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("set-chatmix"), "{json}");
    assert_eq!(req, serde_json::from_str::<Request>(&json).unwrap());
}

#[test]
fn request_set_default_sink_channel_round_trips() {
    let req = Request::SetDefaultSinkChannel { channel: Some("game".into()) };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("set-default-sink-channel"), "{json}");
    assert_eq!(req, serde_json::from_str::<Request>(&json).unwrap());
}
```

- [ ] **Step 2: Add daemon arms**

```rust
        Request::SetMasterVolume { volume_db } => match engine.set_master_volume(volume_db) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::SetMasterMute { muted } => match engine.set_master_mute(muted) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::SetChatmix { position } => match engine.set_chatmix(position) {
            Ok(()) => Response::ok_with_state(engine.state()),
            Err(e) => Response::err(e.to_string()),
        },
        Request::SetDefaultSinkChannel { channel } => {
            match engine.set_default_sink_channel(channel) {
                Ok(()) => Response::ok_with_state(engine.state()),
                Err(e) => Response::err(e.to_string()),
            }
        }
```

- [ ] **Step 3: Add CLI subcommands + dispatch + parser tests**

Add to `Command`:

```rust
    /// Master output: volume + mute.
    Master {
        #[command(subcommand)]
        action: MasterAction,
    },
    /// ChatMix Game<->Chat balance (0=chat .. 9=game).
    Chatmix {
        position: i64,
    },
    /// System default-output channel (apps auto-land here).
    DefaultSink {
        #[command(subcommand)]
        action: DefaultSinkAction,
    },
```

Add enums:

```rust
#[derive(Subcommand, Debug)]
enum MasterAction {
    Volume {
        #[arg(allow_negative_numbers = true)]
        db: f32,
    },
    Mute {
        state: String,
    },
}

#[derive(Subcommand, Debug)]
enum DefaultSinkAction {
    Set { channel: String },
    Clear,
}
```

Add dispatch (mirror `ChannelCmd::Volume` send_request pattern):

```rust
        Command::Master { action } => {
            let req = match action {
                MasterAction::Volume { db } => daemon::Request::SetMasterVolume { volume_db: db },
                MasterAction::Mute { state } => {
                    let muted = match state.as_str() {
                        "on" => true,
                        "off" => false,
                        other => {
                            eprintln!("mute state must be 'on' or 'off', got: {other}");
                            return ExitCode::FAILURE;
                        }
                    };
                    daemon::Request::SetMasterMute { muted }
                }
            };
            send_state_request(&req)
        }
        Command::Chatmix { position } => {
            send_state_request(&daemon::Request::SetChatmix { position })
        }
        Command::DefaultSink { action } => {
            let req = match action {
                DefaultSinkAction::Set { channel } => {
                    daemon::Request::SetDefaultSinkChannel { channel: Some(channel) }
                }
                DefaultSinkAction::Clear => {
                    daemon::Request::SetDefaultSinkChannel { channel: None }
                }
            };
            send_state_request(&req)
        }
```

If a shared `send_state_request` helper does not already exist in `main.rs`, add this small helper near the top of the dispatch section (it collapses the repeated `send_request` match used by `ChannelCmd::Volume`):

```rust
fn send_state_request(req: &daemon::Request) -> ExitCode {
    match daemon::send_request(req) {
        Ok(resp) if resp.ok => ExitCode::SUCCESS,
        Ok(resp) => {
            eprintln!("error: {}", resp.error.unwrap_or_else(|| "unknown".into()));
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error sending request: {e}");
            ExitCode::FAILURE
        }
    }
}
```

Add parser tests:

```rust
#[test]
fn master_volume_parses() {
    let cli = super::Cli::try_parse_from(["asm-cli", "master", "volume", "-6"]).unwrap();
    assert!(matches!(cli.command,
        super::Command::Master { action: super::MasterAction::Volume { db } } if db == -6.0));
}

#[test]
fn chatmix_parses() {
    let cli = super::Cli::try_parse_from(["asm-cli", "chatmix", "7"]).unwrap();
    assert!(matches!(cli.command, super::Command::Chatmix { position: 7 }));
}

#[test]
fn default_sink_set_parses() {
    let cli = super::Cli::try_parse_from(["asm-cli", "default-sink", "set", "game"]).unwrap();
    assert!(matches!(cli.command,
        super::Command::DefaultSink { action: super::DefaultSinkAction::Set { channel } } if channel == "game"));
}
```

- [ ] **Step 4: Run + verify**

Run: `cargo test -p arctis-client 2>&1 | tail -10 && cargo test -p arctis-cli 'master\|chatmix\|default_sink' 2>&1 | tail -20`
Expected: PASS. Then `cargo build -p arctis-cli 2>&1 | tail -5`.

- [ ] **Step 5: Commit**

```bash
git add crates/client/src/protocol.rs crates/cli/src/daemon.rs crates/cli/src/main.rs
git commit -m "feat(cli/protocol): master/chatmix/default-sink verbs + handlers + parser tests"
```

---

## Task 10: Phase 2 workspace gate

- [ ] **Step 1: Full build + test**

Run: `cargo test --workspace 2>&1 | tail -25`
Expected: PASS.

- [ ] **Step 2: Commit (only if fixes were needed)**

```bash
git add -A && git commit -m "test: green workspace after mixer model (master/chatmix/default-sink/aux)"
```

---

# PHASE 3 — GUI redesign

> Frontend tests run with `pnpm --dir frontend test`. Match the existing test runner used by `*.test.ts` files (e.g. `connection.test.ts`). Components are Svelte 5 runes.

## Task 11: Tauri commands + `streams-changed` poll task

**Files:**
- Modify: `src-tauri/src/commands.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Produces Tauri commands: `list_streams`, `move_stream`, `set_master_volume`, `set_master_mute`, `set_chatmix`, `set_default_sink_channel`; a `streams-changed` event payload `Vec<AppStream>`.
- Consumes: the `call`/`call_text` helpers + a new `call_streams` helper for the `ListStreams` response.

- [ ] **Step 1: Add a `call_streams` helper + commands**

In `src-tauri/src/commands.rs`, after `call_text`, add:

```rust
use arctis_engine::AppStream;

/// Variant of `call` for ListStreams (returns the `streams` payload).
async fn call_streams(
    state: &State<'_, Mutex<DaemonState>>,
    req: Request,
) -> Result<Vec<AppStream>, CommandError> {
    let socket = state.lock().await.socket.clone();
    let resp = tauri::async_runtime::spawn_blocking(move || send_request_to(&socket, &req))
        .await
        .map_err(|e| CommandError::DaemonUnavailable(format!("join error: {e}")))??;
    if resp.ok {
        Ok(resp.streams.unwrap_or_default())
    } else {
        Err(CommandError::Daemon(
            resp.error.unwrap_or_else(|| "unknown daemon error".into()),
        ))
    }
}

#[tauri::command]
pub async fn list_streams(
    state: State<'_, Mutex<DaemonState>>,
) -> Result<Vec<AppStream>, CommandError> {
    call_streams(&state, Request::ListStreams).await
}

#[tauri::command]
pub async fn move_stream(
    stream: String,
    channel: String,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::MoveStream { stream, channel }).await
}

#[tauri::command]
pub async fn set_master_volume(
    volume_db: f32,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetMasterVolume { volume_db }).await
}

#[tauri::command]
pub async fn set_master_mute(
    muted: bool,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetMasterMute { muted }).await
}

#[tauri::command]
pub async fn set_chatmix(
    position: i64,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetChatmix { position }).await
}

#[tauri::command]
pub async fn set_default_sink_channel(
    channel: Option<String>,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::SetDefaultSinkChannel { channel }).await
}
```

- [ ] **Step 2: Register commands**

In `src-tauri/src/lib.rs` `generate_handler![ ... ]`, add:

```rust
            commands::list_streams,
            commands::move_stream,
            commands::set_master_volume,
            commands::set_master_mute,
            commands::set_chatmix,
            commands::set_default_sink_channel,
```

- [ ] **Step 3: Add the `streams-changed` poll task**

In `src-tauri/src/lib.rs` `.setup(|app| { ... })`, add a third spawned task (after the state-poll block, mirroring it exactly but ~1.5s + `ListStreams` + `streams-changed`):

```rust
            // ── Streams-poll task (every ~1.5 s) ────────────────────────────
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let mut ticker =
                        tokio::time::interval(std::time::Duration::from_millis(1500));
                    loop {
                        ticker.tick().await;
                        let socket = {
                            let st = handle.state::<Mutex<DaemonState>>();
                            let guard = st.lock().await;
                            guard.socket.clone()
                        };
                        let result = tauri::async_runtime::spawn_blocking(move || {
                            arctis_client::send_request_to(
                                &socket,
                                &arctis_client::Request::ListStreams,
                            )
                        })
                        .await;
                        if let Ok(Ok(resp)) = result {
                            if resp.ok {
                                if let Some(streams) = resp.streams {
                                    let _ = handle.emit("streams-changed", &streams);
                                }
                            }
                        }
                    }
                });
            }
```

- [ ] **Step 4: Build the Tauri crate**

Run: `cargo build -p arctis-sound-manager-tauri 2>&1 | tail -15` (use the crate name from `src-tauri/Cargo.toml`; if unsure: `cargo build -p $(sed -n 's/^name = "\(.*\)"/\1/p' src-tauri/Cargo.toml | head -1) 2>&1 | tail -15`).
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat(tauri): stream/master/chatmix/default-sink commands + streams-changed poll task"
```

---

## Task 12: Frontend ipc + stream store + pure helpers

**Files:**
- Modify: `frontend/src/lib/ipc.ts`
- Create: `frontend/src/lib/streams.ts`
- Create: `frontend/src/lib/stores/streams.ts`
- Create: `frontend/src/lib/streams.test.ts`

**Interfaces:**
- Produces: TS `AppStream` interface; ipc `listStreams()`, `moveStream(stream, channel)`, `setMasterVolume(db)`, `setMasterMute(muted)`, `setChatmix(position)`, `setDefaultSinkChannel(channel)`, `onStreamsChanged(cb)`; `streamsStore` + `initStreams()`; pure `groupStreamsByChannel(streams, channelIds)`.

- [ ] **Step 1: Add ipc types + wrappers**

In `frontend/src/lib/ipc.ts`, add the interface near the other interfaces:

```ts
export interface AppStream {
  id: number;
  binary: string;
  app_name: string;
  pid: number | null;
  icon_name: string | null;
  media_name: string | null;
  current_channel: string | null;
  routed: boolean;
}
```

Add the new `EngineState` fields to the existing `EngineState` interface (master/chatmix/default-sink):

```ts
  master_volume_db: number;
  master_mute: boolean;
  chatmix_position: number;
  default_sink_channel: string | null;
```

Add wrappers (mirror `setChannelVolume`/`clearRoute`):

```ts
export const listStreams = (): Promise<AppStream[]> => invoke<AppStream[]>("list_streams");

export const moveStream = (stream: string, channel: string): Promise<EngineState> =>
  invoke<EngineState>("move_stream", { stream, channel });

export const setMasterVolume = (volumeDb: number): Promise<EngineState> =>
  invoke<EngineState>("set_master_volume", { volumeDb });

export const setMasterMute = (muted: boolean): Promise<EngineState> =>
  invoke<EngineState>("set_master_mute", { muted });

export const setChatmix = (position: number): Promise<EngineState> =>
  invoke<EngineState>("set_chatmix", { position });

export const setDefaultSinkChannel = (channel: string | null): Promise<EngineState> =>
  invoke<EngineState>("set_default_sink_channel", { channel });
```

> Tauri v2 maps snake_case Rust params to camelCase JS keys by default. Confirm against an existing wrapper (e.g. `setChannelVolume` passes `{ channel, volumeDb }` for Rust `volume_db`). Match that convention exactly.

Add the listener near `onStateChanged`/`onLevels`:

```ts
export const onStreamsChanged = (cb: (s: AppStream[]) => void): Promise<UnlistenFn> =>
  listen<AppStream[]>("streams-changed", (e) => cb(e.payload));
```

- [ ] **Step 2: Write the failing pure-helper test**

Create `frontend/src/lib/streams.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { groupStreamsByChannel } from "./streams.js";
import type { AppStream } from "./ipc.js";

const mk = (binary: string, current_channel: string | null): AppStream => ({
  id: 1, binary, app_name: binary, pid: null, icon_name: null,
  media_name: null, current_channel, routed: false,
});

describe("groupStreamsByChannel", () => {
  it("buckets streams by channel id and unrouted", () => {
    const streams = [mk("firefox", "game"), mk("spotify", null), mk("discord", "chat")];
    const g = groupStreamsByChannel(streams, ["game", "chat", "media", "aux"]);
    expect(g.byChannel.game.map((s) => s.binary)).toEqual(["firefox"]);
    expect(g.byChannel.chat.map((s) => s.binary)).toEqual(["discord"]);
    expect(g.byChannel.media).toEqual([]);
    expect(g.unrouted.map((s) => s.binary)).toEqual(["spotify"]);
  });

  it("treats streams on unknown channels as unrouted", () => {
    const g = groupStreamsByChannel([mk("x", "ghost")], ["game"]);
    expect(g.unrouted.map((s) => s.binary)).toEqual(["x"]);
  });
});
```

- [ ] **Step 3: Run to verify failure**

Run: `pnpm --dir frontend test streams 2>&1 | tail -20`
Expected: FAIL — module/function missing.

- [ ] **Step 4: Implement `streams.ts`**

Create `frontend/src/lib/streams.ts`:

```ts
import type { AppStream } from "./ipc.js";

export interface GroupedStreams {
  byChannel: Record<string, AppStream[]>;
  unrouted: AppStream[];
}

/**
 * Group live streams under their resolved channel id. Streams whose
 * current_channel is null or not in `channelIds` go to `unrouted` (the
 * Master "Apps to be routed" tray).
 */
export function groupStreamsByChannel(
  streams: AppStream[],
  channelIds: string[],
): GroupedStreams {
  const byChannel: Record<string, AppStream[]> = {};
  for (const id of channelIds) byChannel[id] = [];
  const unrouted: AppStream[] = [];
  for (const s of streams) {
    if (s.current_channel && byChannel[s.current_channel]) {
      byChannel[s.current_channel].push(s);
    } else {
      unrouted.push(s);
    }
  }
  return { byChannel, unrouted };
}
```

- [ ] **Step 5: Implement the store**

Create `frontend/src/lib/stores/streams.ts`:

```ts
import { writable } from "svelte/store";
import { listStreams, onStreamsChanged, type AppStream } from "../ipc.js";
import type { UnlistenFn } from "@tauri-apps/api/event";

export const streamsStore = writable<AppStream[]>([]);

let unlisten: UnlistenFn | null = null;
let started = false;

export async function initStreams(): Promise<void> {
  if (started) return;
  started = true;
  try {
    unlisten = await onStreamsChanged((s) => streamsStore.set(s));
  } catch (e) {
    console.warn("[streams] subscribe failed:", e);
  }
  try {
    streamsStore.set(await listStreams());
  } catch {
    // daemon down — keep empty; poll will refill.
  }
}

export function destroyStreams(): void {
  if (unlisten) {
    unlisten();
    unlisten = null;
  }
  started = false;
}
```

- [ ] **Step 6: Run to verify pass**

Run: `pnpm --dir frontend test streams 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/lib/ipc.ts frontend/src/lib/streams.ts frontend/src/lib/streams.test.ts frontend/src/lib/stores/streams.ts
git commit -m "feat(frontend): stream ipc wrappers, store, and pure grouping helper"
```

---

## Task 13: `AppPill` + drag/drop into `ChannelStrip`

**Files:**
- Create: `frontend/src/lib/components/AppPill.svelte`
- Modify: `frontend/src/lib/components/ChannelStrip.svelte`
- Modify: `frontend/src/styles/tokens.css`

**Interfaces:**
- Produces: `AppPill` (props: `stream: AppStream`, `accent: string`); `ChannelStrip` gains props `streams: AppStream[]` and `onDropStream: (streamId: string, channelId: string) => void`.

- [ ] **Step 1: Add drag tokens**

In `frontend/src/styles/tokens.css`, add inside `:root` near other tokens:

```css
  --ss-drag-highlight: rgba(255, 82, 0, 0.18);
  --ss-accent-game: #41d18b;
  --ss-accent-chat: #38b6ff;
  --ss-accent-media: #ff3d7f;
  --ss-accent-aux: #9b6bff;
  --ss-accent-mic: #ffb020;
  --ss-accent-master: #6b8cff;
```

- [ ] **Step 2: Create `AppPill.svelte`**

```svelte
<script lang="ts">
  import type { AppStream } from "../ipc.js";
  let { stream, accent = "var(--ss-accent)" }: { stream: AppStream; accent?: string } = $props();

  function onDragStart(e: DragEvent) {
    if (!e.dataTransfer) return;
    // Carry the live node id so the drop target can move the exact instance.
    e.dataTransfer.setData("text/asm-stream-id", String(stream.id));
    e.dataTransfer.effectAllowed = "move";
  }
</script>

<div
  class="app-pill"
  draggable="true"
  ondragstart={onDragStart}
  style="--pill-accent: {accent}"
  title={stream.media_name ?? stream.app_name}
  role="listitem"
>
  <span class="pill-dot" aria-hidden="true"></span>
  <span class="pill-name">{stream.app_name}</span>
</div>

<style>
  .app-pill {
    display: inline-flex;
    align-items: center;
    gap: var(--ss-space-1);
    padding: 2px var(--ss-space-2);
    background: var(--ss-surface-2);
    border: var(--ss-border-width) solid var(--pill-accent);
    border-radius: var(--ss-radius-pill);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    cursor: grab;
    user-select: none;
    max-width: 100%;
  }
  .app-pill:active { cursor: grabbing; }
  .pill-dot {
    width: 8px; height: 8px; border-radius: 50%;
    background: var(--pill-accent); flex-shrink: 0;
  }
  .pill-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
```

- [ ] **Step 3: Add a drop area to `ChannelStrip.svelte`**

Add `streams` + `onDropStream` to the component's `$props()` destructure. Add a local drag-over flag and handlers in the `<script>`:

```ts
  let dragOver = $state(false);
  function onDragOver(e: DragEvent) {
    if (e.dataTransfer?.types.includes("text/asm-stream-id")) {
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      dragOver = true;
    }
  }
  function onDragLeave() { dragOver = false; }
  function onDrop(e: DragEvent) {
    e.preventDefault();
    dragOver = false;
    const id = e.dataTransfer?.getData("text/asm-stream-id");
    if (id) onDropStream(id, channel.id);
  }
```

Add the apps area markup near the bottom of the strip (after the EQ button, before closing the strip container):

```svelte
  <div
    class="strip-apps"
    class:drag-over={dragOver}
    role="list"
    aria-label="{channel.id} applications"
    ondragover={onDragOver}
    ondragleave={onDragLeave}
    ondrop={onDrop}
  >
    {#each streams as s (s.id)}
      <AppPill stream={s} accent={accentFor(channel.id)} />
    {/each}
  </div>
```

Add the import and an accent helper in `<script>`:

```ts
  import AppPill from "./AppPill.svelte";
  function accentFor(id: string): string {
    const map: Record<string, string> = {
      game: "var(--ss-accent-game)", chat: "var(--ss-accent-chat)",
      media: "var(--ss-accent-media)", aux: "var(--ss-accent-aux)",
    };
    return map[id] ?? "var(--ss-accent)";
  }
```

Add styles:

```css
  .strip-apps {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-1);
    min-height: 48px;
    padding: var(--ss-space-2);
    margin-top: var(--ss-space-2);
    border: var(--ss-border-width) dashed var(--ss-border);
    border-radius: var(--ss-radius-sm);
    transition: background var(--ss-dur-fast) var(--ss-ease-standard);
  }
  .strip-apps.drag-over {
    background: var(--ss-drag-highlight);
    border-color: var(--ss-accent);
  }
```

- [ ] **Step 4: Verify the frontend builds/type-checks**

Run: `pnpm --dir frontend build 2>&1 | tail -20`
Expected: builds (Svelte + tsc pass). Fix prop/type mismatches against the real `ChannelStrip` `$props()` shape.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/components/AppPill.svelte frontend/src/lib/components/ChannelStrip.svelte frontend/src/styles/tokens.css
git commit -m "feat(frontend): draggable AppPill + drop area on ChannelStrip"
```

---

## Task 14: `MasterStrip`, `MicStrip`, `ChatmixSlider`

**Files:**
- Create: `frontend/src/lib/components/MasterStrip.svelte`
- Create: `frontend/src/lib/components/MicStrip.svelte`
- Create: `frontend/src/lib/components/ChatmixSlider.svelte`

**Interfaces:**
- Produces: `MasterStrip` (props: `state: EngineState`, `unrouted: AppStream[]`, `onClearStream: (id: string) => void`); `MicStrip` (props: `mic: MicSnapshot`); `ChatmixSlider` (props: `position: number`, `hardwareActive: boolean`).

- [ ] **Step 1: Create `MasterStrip.svelte`**

```svelte
<script lang="ts">
  import { setMasterVolume, setMasterMute, setDefaultSinkChannel,
    type EngineState, type AppStream } from "../ipc.js";
  import { engineState } from "../stores.js";
  import AppPill from "./AppPill.svelte";

  let { state, unrouted, onClearStream }:
    { state: EngineState; unrouted: AppStream[]; onClearStream: (id: string) => void } = $props();

  let dragOver = $state(false);
  function onDragOver(e: DragEvent) {
    if (e.dataTransfer?.types.includes("text/asm-stream-id")) {
      e.preventDefault(); dragOver = true;
    }
  }
  function onDrop(e: DragEvent) {
    e.preventDefault(); dragOver = false;
    const id = e.dataTransfer?.getData("text/asm-stream-id");
    if (id) onClearStream(id); // drop back to tray = unroute
  }

  async function onVol(e: Event) {
    const db = Number((e.target as HTMLInputElement).value);
    engineState.set(await setMasterVolume(db));
  }
  async function onMute() { engineState.set(await setMasterMute(!state.master_mute)); }
  async function onDefaultToggle(e: Event) {
    const checked = (e.target as HTMLInputElement).checked;
    engineState.set(await setDefaultSinkChannel(checked ? "game" : null));
  }
</script>

<div class="strip master" style="--accent: var(--ss-accent-master)">
  <h3 class="strip-name">MASTER</h3>
  <input class="vol" type="range" min="-60" max="6" step="1"
    value={state.master_volume_db} oninput={onVol} aria-label="Master volume" />
  <span class="vol-label">{state.master_volume_db.toFixed(0)} dB</span>
  <button class="mute" class:on={state.master_mute} onclick={onMute}
    aria-pressed={state.master_mute}>{state.master_mute ? "Muted" : "Mute"}</button>

  <label class="default-toggle">
    <input type="checkbox" checked={state.default_sink_channel != null}
      onchange={onDefaultToggle} />
    Auto-route new apps
  </label>

  <p class="tray-label">Apps to be routed</p>
  <div class="tray" class:drag-over={dragOver}
    role="list" aria-label="Unrouted applications"
    ondragover={onDragOver} ondragleave={() => (dragOver = false)} ondrop={onDrop}>
    {#each unrouted as s (s.id)}
      <AppPill stream={s} accent="var(--ss-accent-master)" />
    {/each}
    {#if unrouted.length === 0}
      <span class="tray-empty">All apps routed</span>
    {/if}
  </div>
</div>

<style>
  .strip.master {
    display: flex; flex-direction: column; gap: var(--ss-space-2);
    width: var(--ss-channel-strip-w, 120px); min-width: var(--ss-channel-strip-w-min, 100px);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--accent);
    border-radius: var(--ss-radius-md); padding: var(--ss-space-3); flex-shrink: 0;
  }
  .strip-name { font-family: var(--ss-font-display); text-transform: uppercase;
    color: var(--accent); margin: 0; font-size: var(--ss-type-h2-size); }
  .vol { width: 100%; accent-color: var(--accent); }
  .vol-label { font-family: var(--ss-font-mono); font-size: var(--ss-type-caption-size);
    color: var(--ss-text-secondary); }
  .mute { cursor: pointer; }
  .mute.on { color: var(--ss-danger); }
  .default-toggle { display: flex; gap: var(--ss-space-1); align-items: center;
    font-size: var(--ss-type-caption-size); color: var(--ss-text-tertiary); }
  .tray-label { font-size: var(--ss-type-micro-size); text-transform: uppercase;
    color: var(--ss-text-tertiary); margin: var(--ss-space-2) 0 0; }
  .tray { display: flex; flex-direction: column; gap: var(--ss-space-1); min-height: 60px;
    padding: var(--ss-space-2); border: var(--ss-border-width) dashed var(--ss-border);
    border-radius: var(--ss-radius-sm); }
  .tray.drag-over { background: var(--ss-drag-highlight); border-color: var(--accent); }
  .tray-empty { font-size: var(--ss-type-caption-size); color: var(--ss-text-disabled);
    font-style: italic; }
</style>
```

- [ ] **Step 2: Create `MicStrip.svelte`**

```svelte
<script lang="ts">
  import { micEnable, type MicSnapshot } from "../ipc.js";
  import { engineState } from "../stores.js";
  import { setPage } from "../stores/page.js";

  let { mic }: { mic: MicSnapshot } = $props();
  async function onToggle() { engineState.set(await micEnable(!mic.enabled)); }
</script>

<div class="strip mic" style="--accent: var(--ss-accent-mic)">
  <h3 class="strip-name">MIC</h3>
  <button class="mute" class:on={!mic.enabled} onclick={onToggle}
    aria-pressed={!mic.enabled}>{mic.enabled ? "On" : "Off"}</button>
  <button class="edit" onclick={() => setPage("mic")}>Edit</button>
</div>

<style>
  .strip.mic { display: flex; flex-direction: column; gap: var(--ss-space-2);
    width: var(--ss-channel-strip-w, 120px); min-width: var(--ss-channel-strip-w-min, 100px);
    background: var(--ss-surface-1); border: var(--ss-border-width) solid var(--accent);
    border-radius: var(--ss-radius-md); padding: var(--ss-space-3); flex-shrink: 0; }
  .strip-name { font-family: var(--ss-font-display); text-transform: uppercase;
    color: var(--accent); margin: 0; font-size: var(--ss-type-h2-size); }
  .mute.on { color: var(--ss-danger); }
  .edit, .mute { cursor: pointer; }
</style>
```

> Confirm the page-store API: `frontend/src/lib/stores/page.ts` — use its real setter (e.g. `page.set("mic")` or `setPage("mic")`). Match what `AppShell.svelte` uses for navigation. Also confirm `MicSnapshot` has an `enabled` field (it does per `ipc.ts`).

- [ ] **Step 3: Create `ChatmixSlider.svelte`**

```svelte
<script lang="ts">
  import { setChatmix } from "../ipc.js";
  import { engineState } from "../stores.js";

  let { position, hardwareActive = false }:
    { position: number; hardwareActive?: boolean } = $props();

  async function onInput(e: Event) {
    const pos = Number((e.target as HTMLInputElement).value);
    engineState.set(await setChatmix(pos));
  }
</script>

<div class="chatmix" class:disabled={hardwareActive}>
  <span class="end game">Game</span>
  <input type="range" min="0" max="9" step="1" value={position}
    disabled={hardwareActive} oninput={onInput} aria-label="ChatMix balance" />
  <span class="end chat">Chat</span>
  {#if hardwareActive}
    <span class="hw-note">Hardware dial active</span>
  {/if}
</div>

<style>
  .chatmix { display: flex; align-items: center; gap: var(--ss-space-3);
    padding: var(--ss-space-3); background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border); border-radius: var(--ss-radius-md); }
  .chatmix input { flex: 1; accent-color: var(--ss-accent); }
  .chatmix.disabled { opacity: 0.6; }
  .end { font-family: var(--ss-font-display); text-transform: uppercase;
    font-size: var(--ss-type-caption-size); }
  .end.game { color: var(--ss-accent-game); }
  .end.chat { color: var(--ss-accent-chat); }
  .hw-note { font-size: var(--ss-type-caption-size); color: var(--ss-text-tertiary); }
</style>
```

- [ ] **Step 4: Verify build**

Run: `pnpm --dir frontend build 2>&1 | tail -20`
Expected: builds. Fix any import/type mismatches against real store/ipc APIs.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/components/MasterStrip.svelte frontend/src/lib/components/MicStrip.svelte frontend/src/lib/components/ChatmixSlider.svelte
git commit -m "feat(frontend): MasterStrip (tray + default toggle), MicStrip, ChatmixSlider"
```

---

## Task 15: Assemble `MixerPage` + view-only "Remembered routes"

**Files:**
- Modify: `frontend/src/lib/components/MixerPage.svelte`
- Modify: `frontend/src/lib/components/RouteList.svelte`

**Interfaces:**
- Consumes: `streamsStore`/`initStreams`, `groupStreamsByChannel`, `MasterStrip`, `MicStrip`, `ChatmixSlider`, `moveStream`, `clearRoute`, the existing `ChannelStrip`, `engineState`.

- [ ] **Step 1: Rewrite `MixerPage` mixer body**

In `MixerPage.svelte` `<script>`, add imports + wiring:

```ts
  import { streamsStore, initStreams, destroyStreams } from "../stores/streams.js";
  import { groupStreamsByChannel } from "../streams.js";
  import { moveStream, clearRoute } from "../ipc.js";
  import MasterStrip from "./MasterStrip.svelte";
  import MicStrip from "./MicStrip.svelte";
  import ChatmixSlider from "./ChatmixSlider.svelte";

  $effect(() => { initStreams(); return () => destroyStreams(); });

  let grouped = $derived(
    groupStreamsByChannel(
      $streamsStore,
      ($engineState?.channels ?? []).map((c) => c.id),
    ),
  );

  async function handleDropStream(streamId: string, channelId: string) {
    try { engineState.set(await moveStream(streamId, channelId)); }
    catch (e) { console.error("[mixer] moveStream failed:", e); }
  }
  async function handleClearStream(streamId: string) {
    // streamId is a node id; resolve to binary for clearRoute.
    const s = $streamsStore.find((x) => String(x.id) === streamId);
    if (!s) return;
    try { engineState.set(await clearRoute(s.binary)); }
    catch (e) { console.error("[mixer] clearRoute failed:", e); }
  }
```

Replace the channels-row block so it renders Master first, then channels with their pills, then Mic, then the add affordance, and the ChatMix slider + RouteList below:

```svelte
        <div class="channels-row" role="list" aria-label="Audio channel strips">
          <MasterStrip state={$engineState} unrouted={grouped.unrouted}
            onClearStream={handleClearStream} />

          {#each $engineState.channels as channel (channel.id)}
            <div role="listitem">
              <ChannelStrip
                {channel}
                streams={grouped.byChannel[channel.id] ?? []}
                onDropStream={handleDropStream}
                onOutputChanged={() => {}}
                onRemove={$engineState.channels.length > 1 && !removeBusy
                  ? () => handleRemoveChannel(channel.id) : undefined}
              />
            </div>
          {/each}

          <MicStrip mic={$engineState.mic} />

          <!-- existing add-channel affordance stays here -->
        </div>

        <ChatmixSlider position={$engineState.chatmix_position}
          hardwareActive={$engineState.device_present} />
```

> Keep the existing add-channel affordance markup; just relocate it to follow `MicStrip`. `hardwareActive={$engineState.device_present}` is a conservative default (grey the slider whenever a device is connected, since the hardware dial drives balance). If owner testing prefers always-enabled, change to `false`.

- [ ] **Step 2: Collapse `RouteList` to view-only**

In `RouteList.svelte`, remove the add-route `<form>` block and its handlers (`handleAdd`, `newApp`, `newSink`, `adding`, `addError`, `handleAddKeydown`). Keep the heading, the routes table, and the per-row remove button. Change the heading text to `REMEMBERED ROUTES` and wrap the card in a `<details>` so it is collapsible:

```svelte
<details class="remembered">
  <summary>Remembered routes ({routes.length})</summary>
  <!-- existing routes table + per-row remove button stay -->
</details>
```

Leave the empty-state text as `No routes remembered yet — drag an app onto a channel.`

- [ ] **Step 3: Verify build**

Run: `pnpm --dir frontend build 2>&1 | tail -20`
Expected: builds.

- [ ] **Step 4: Run the frontend test suite**

Run: `pnpm --dir frontend test 2>&1 | tail -20`
Expected: PASS (existing + new streams test). Fix any breakage from the RouteList edit (e.g. a `RouteList` test asserting the form).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/components/MixerPage.svelte frontend/src/lib/components/RouteList.svelte
git commit -m "feat(frontend): Sonar mixer layout — Master/Mic strips, app pills, ChatMix, view-only routes"
```

---

## Task 16: Final workspace + frontend gate

- [ ] **Step 1: Rust workspace**

Run: `cargo test --workspace 2>&1 | tail -25`
Expected: PASS.

- [ ] **Step 2: Frontend**

Run: `pnpm --dir frontend test 2>&1 | tail -20 && pnpm --dir frontend build 2>&1 | tail -10`
Expected: tests PASS, build OK.

- [ ] **Step 3: Commit (if fixes needed)**

```bash
git add -A && git commit -m "test: green workspace + frontend after Sonar mixer redesign"
```

---

## Owner-run validation (after implementation, real hardware)

These cannot be unit-tested and must be checked on the target machine with the daemon + GUI running:

1. `asm-cli streams list` shows Chrome/Spotify/Discord (pulse-compat apps) with correct channels.
2. Dragging a pill between strips moves audio live and survives the app restart (persisted rule).
3. Dragging a pill to the Master tray returns it to default routing.
4. `asm-cli master volume -10` / GUI master slider attenuates the headset output; mute works.
5. `asm-cli chatmix 9` / the slider biases Game vs Chat; confirm whether the hardware Nova Pro dial should grey out the slider (adjust `hardwareActive` accordingly).
6. `asm-cli default-sink set game` makes new apps land in Game; verify `wpctl set-default` accepts the node.name (else switch to id resolution — see Task 8 note).
7. Aux strip appears for the existing `default` profile (auto-seed) and routes audio.

---

## Self-Review Notes (author)

- **Spec coverage:** discovery (T1–T2), pulse-compat fix (T1), sink→channel map (T2), MoveStream live+persist (T3), protocol/daemon/CLI parity (T4–T5, T9), Master/ChatMix/default-sink/Aux (T7–T9), `streams-changed` poll (T11), GUI strips + drag/drop + tray + ChatMix + view-only routes (T12–T15). Streamer mode intentionally excluded (deferred per spec).
- **Parity:** every verb appears in protocol + daemon + CLI + Tauri command (registered) + ipc + GUI.
- **Reuse:** `Router`, `move_stream_argv`, `set_route`, `ChannelManager`, `LevelMeter`, output dropdown, add-channel affordance all reused; no duplicated routing logic.
- **Open validation flags surfaced inline:** `EngineError::Audio` conversion (match file pattern), `wpctl set-default` name-vs-id, Tauri camelCase param mapping, `chatmix` hardware-dial grey-out — all called out at their tasks for the implementer/owner.
