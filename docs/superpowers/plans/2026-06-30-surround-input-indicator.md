# Surround Input Indicator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Show on the Spatial page whether the app(s) routed into a surround channel (e.g. DayZ on `game`) are negotiating real surround (rear/side channels present) vs only stereo.

**Architecture:** Extend the pure `pw-dump` parser (`audio` crate) to capture each stream's negotiated channel count + position map. Add pure classifier/selector functions. Wire them into the engine's existing `state()` snapshot (reusing the already-cached `pw-dump`, no new subprocess) to populate `SurroundSnapshot.negotiated_channels` (currently hard-coded `None`) plus a new `negotiated_surround` flag. Mirror the field in the TS IPC type and render a status banner on the Spatial page.

**Tech Stack:** Rust (Cargo workspace: `arctis-audio`, `arctis-engine`), Tauri v2, Svelte 5 + TypeScript frontend, vitest.

## Global Constraints

- Read-only throughout. No device writes, no graph mutation (ARCHITECTURE G2). The indicator must never alter routing or `effective_mode`/Auto-mode resolution.
- No new subprocess in `state()`: reuse the `pw_dump_json` already obtained via `volume_dump()` (`crates/engine/src/engine.rs:385`).
- Typed errors, no `unwrap` on runtime paths; malformed/empty pw-dump degrades to `None` (G7).
- Reuse over duplication (G1): fold the dead `parse_stream_channels` into the per-stream path and remove it.
- Audio is 48 kHz; the relevant signal is channel *count* + *position map*, not sample rate.
- Rust tests: prefer `cargo test --workspace -- --test-threads=1`. CI gate: `cargo clippy -D warnings` + `cargo test`.
- Frontend tests: `pnpm -C frontend test` (vitest); type-check: `pnpm -C frontend check`.
- "True surround" = a rear/side channel (`RL`/`RR`/`SL`/`SR`, case-insensitive) is present in the negotiated position map.

---

### Task 1: Capture negotiated channels + positions in `ParsedStream`

**Files:**
- Modify: `crates/audio/src/streams.rs` (struct `ParsedStream` ~13-21; `parse_app_streams` ~108-188; tests module ~199)

**Interfaces:**
- Consumes: nothing new.
- Produces: `ParsedStream` gains `pub channels: Option<u8>` and `pub positions: Vec<String>`. New private helper `fn parse_node_format(info: &serde_json::Value) -> (Option<u8>, Vec<String>)`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/audio/src/streams.rs` (after the existing tests, before the closing `}`):

```rust
    // Inline pw-dump: one DayZ stream (8ch 7.1) linked to the Arctis_Game sink.
    const DAYZ_8CH: &str = r#"[
      { "id": 50, "type": "PipeWire:Interface:Node",
        "info": { "props": { "media.class": "Audio/Sink", "node.name": "Arctis_Game" } } },
      { "id": 51, "type": "PipeWire:Interface:Node",
        "info": { "props": {
            "media.class": "Stream/Output/Audio",
            "application.name": "DayZ",
            "application.process.binary": "DayZ" },
          "params": { "Format": [ { "channels": 8,
            "position": ["FL","FR","FC","LFE","RL","RR","SL","SR"] } ] } } },
      { "id": 99, "type": "PipeWire:Interface:Link",
        "info": { "output-node-id": 51, "input-node-id": 50 } }
    ]"#;

    #[test]
    fn parses_negotiated_channels_and_positions_from_format() {
        let streams = parse_app_streams(DAYZ_8CH).unwrap();
        let dz = streams.iter().find(|s| s.binary == "DayZ").unwrap();
        assert_eq!(dz.channels, Some(8));
        assert!(dz.positions.contains(&"RL".to_string()));
        assert_eq!(dz.sink_node_name.as_deref(), Some("Arctis_Game"));
    }

    #[test]
    fn channels_from_audio_channels_prop_takes_precedence() {
        // props.audio.channels as a numeric string; no params.Format.
        let dump = r#"[
          { "id": 1, "type": "PipeWire:Interface:Node",
            "info": { "props": {
                "media.class": "Stream/Output/Audio",
                "application.name": "DayZ",
                "application.process.binary": "DayZ",
                "audio.channels": "2" } } }
        ]"#;
        let streams = parse_app_streams(dump).unwrap();
        let dz = streams.iter().find(|s| s.binary == "DayZ").unwrap();
        assert_eq!(dz.channels, Some(2));
        assert!(dz.positions.is_empty());
    }

    #[test]
    fn missing_format_yields_none_channels_and_empty_positions() {
        let dump = r#"[
          { "id": 1, "type": "PipeWire:Interface:Node",
            "info": { "props": {
                "media.class": "Stream/Output/Audio",
                "application.name": "DayZ",
                "application.process.binary": "DayZ" } } }
        ]"#;
        let streams = parse_app_streams(dump).unwrap();
        let dz = streams.iter().find(|s| s.binary == "DayZ").unwrap();
        assert_eq!(dz.channels, None);
        assert!(dz.positions.is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-audio --lib streams:: -- --test-threads=1`
Expected: FAIL — `no field 'channels' on type '&ParsedStream'` (compile error).

- [ ] **Step 3: Add the fields and the parsing helper**

In `crates/audio/src/streams.rs`, extend the struct (the `ParsedStream` definition, currently ending with `sink_node_name`):

```rust
    pub sink_node_name: Option<String>,
    /// Negotiated channel count from the node's format, if known.
    pub channels: Option<u8>,
    /// Negotiated channel position map (e.g. ["FL","FR","FC",…]); empty if unknown.
    pub positions: Vec<String>,
}
```

Add this helper just above `pub fn parse_app_streams` (after the `resolve_app_name` fn):

```rust
/// Read a node's negotiated audio format from its `info` object: channel count
/// and channel position map. Count precedence: `props["audio.channels"]`
/// (number or numeric string) → `params.Format[0].channels`. Positions come
/// from `params.Format[0].position`. Never panics.
fn parse_node_format(info: &serde_json::Value) -> (Option<u8>, Vec<String>) {
    let count = info
        .get("props")
        .and_then(|p| p.get("audio.channels"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .or_else(|| {
            info.get("params")
                .and_then(|p| p.get("Format"))
                .and_then(|f| f.as_array())
                .and_then(|a| a.first())
                .and_then(|e| e.get("channels"))
                .and_then(|c| c.as_u64())
        })
        .map(|c| c as u8);
    let positions = info
        .get("params")
        .and_then(|p| p.get("Format"))
        .and_then(|f| f.as_array())
        .and_then(|a| a.first())
        .and_then(|e| e.get("position"))
        .and_then(|p| p.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    (count, positions)
}
```

Restructure the `info`/`props` binding inside the loop. Replace:

```rust
        let props = match obj.get("info").and_then(|i| i.get("props")) {
            Some(p) => p,
            None => continue,
        };
```

with:

```rust
        let info = match obj.get("info") {
            Some(i) => i,
            None => continue,
        };
        let props = match info.get("props") {
            Some(p) => p,
            None => continue,
        };
```

Then in the `streams.push(ParsedStream { … })` call, compute the format just before the push and add the two fields:

```rust
        let (channels, positions) = parse_node_format(info);
        streams.push(ParsedStream {
            id,
            binary,
            app_name,
            pid,
            icon_name,
            media_name,
            sink_node_name: None,
            channels,
            positions,
        });
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p arctis-audio --lib streams:: -- --test-threads=1`
Expected: PASS (all `streams::tests::…`, including the 3 new tests).

- [ ] **Step 5: Commit**

```bash
git add crates/audio/src/streams.rs
git commit -m "feat(audio): parse negotiated channels + position map per stream"
```

---

### Task 2: Pure surround-input classifier + richest-stream selector

**Files:**
- Modify: `crates/audio/src/streams.rs` (add public items + tests)
- Modify: `crates/audio/src/lib.rs` (exports)

**Interfaces:**
- Consumes: `ParsedStream { channels, positions, sink_node_name }` from Task 1.
- Produces:
  - `pub struct SurroundInput { pub channels: u8, pub label: &'static str, pub is_true_surround: bool }`
  - `pub fn classify_surround_input(channels: u8, positions: &[String]) -> SurroundInput`
  - `pub fn richest_surround_input(streams: &[ParsedStream], surround_sinks: &[String]) -> Option<SurroundInput>`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/audio/src/streams.rs`:

```rust
    #[test]
    fn classify_true_surround_requires_rear_or_side_channel() {
        let p71 = ["FL","FR","FC","LFE","RL","RR","SL","SR"].map(String::from);
        let c = classify_surround_input(8, &p71);
        assert_eq!(c.label, "7.1");
        assert!(c.is_true_surround);

        let p51 = ["FL","FR","FC","LFE","RL","RR"].map(String::from);
        assert_eq!(classify_surround_input(6, &p51).label, "5.1");
        assert!(classify_surround_input(6, &p51).is_true_surround);

        // Padded 8ch with only front channels → NOT true surround.
        let padded = ["FL","FR"].map(String::from);
        let c = classify_surround_input(8, &padded);
        assert_eq!(c.label, "7.1");
        assert!(!c.is_true_surround);

        let stereo = ["FL","FR"].map(String::from);
        let c = classify_surround_input(2, &stereo);
        assert_eq!(c.label, "Stereo");
        assert!(!c.is_true_surround);

        assert_eq!(classify_surround_input(1, &[]).label, "Mono");
    }

    #[test]
    fn richest_input_picks_max_channels_on_surround_sink() {
        // DayZ 7.1 on Arctis_Game + Discord stereo on Arctis_Game → richest = DayZ.
        let dump = r#"[
          { "id": 50, "type": "PipeWire:Interface:Node",
            "info": { "props": { "media.class": "Audio/Sink", "node.name": "Arctis_Game" } } },
          { "id": 51, "type": "PipeWire:Interface:Node",
            "info": { "props": { "media.class": "Stream/Output/Audio",
                "application.name": "DayZ", "application.process.binary": "DayZ" },
              "params": { "Format": [ { "channels": 8,
                "position": ["FL","FR","FC","LFE","RL","RR","SL","SR"] } ] } } },
          { "id": 52, "type": "PipeWire:Interface:Node",
            "info": { "props": { "media.class": "Stream/Output/Audio",
                "application.name": "Discord", "application.process.binary": "Discord" },
              "params": { "Format": [ { "channels": 2, "position": ["FL","FR"] } ] } } },
          { "id": 98, "type": "PipeWire:Interface:Link",
            "info": { "output-node-id": 51, "input-node-id": 50 } },
          { "id": 99, "type": "PipeWire:Interface:Link",
            "info": { "output-node-id": 52, "input-node-id": 50 } }
        ]"#;
        let streams = parse_app_streams(dump).unwrap();
        let si = richest_surround_input(&streams, &["Arctis_Game".to_string()]).unwrap();
        assert_eq!(si.channels, 8);
        assert!(si.is_true_surround);
    }

    #[test]
    fn richest_input_none_when_no_stream_on_surround_sink() {
        let dump = r#"[
          { "id": 50, "type": "PipeWire:Interface:Node",
            "info": { "props": { "media.class": "Audio/Sink", "node.name": "Arctis_Chat" } } },
          { "id": 51, "type": "PipeWire:Interface:Node",
            "info": { "props": { "media.class": "Stream/Output/Audio",
                "application.name": "DayZ", "application.process.binary": "DayZ" },
              "params": { "Format": [ { "channels": 8,
                "position": ["FL","FR","FC","LFE","RL","RR","SL","SR"] } ] } } },
          { "id": 99, "type": "PipeWire:Interface:Link",
            "info": { "output-node-id": 51, "input-node-id": 50 } }
        ]"#;
        let streams = parse_app_streams(dump).unwrap();
        assert!(richest_surround_input(&streams, &["Arctis_Game".to_string()]).is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-audio --lib streams:: -- --test-threads=1`
Expected: FAIL — `cannot find function 'classify_surround_input'`.

- [ ] **Step 3: Implement the public items**

Add to `crates/audio/src/streams.rs` (after `parse_app_streams`, before the `#[cfg(test)]` module):

```rust
/// A negotiated surround input layout, classified for UI display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurroundInput {
    pub channels: u8,
    /// Human label by channel count: "Mono" | "Stereo" | "Quad" | "5.1" | "7.1" | "Multichannel".
    pub label: &'static str,
    /// True only when a rear/side channel (RL/RR/SL/SR) is present in the position map.
    pub is_true_surround: bool,
}

/// Classify a negotiated `(channels, positions)` pair. `is_true_surround` is
/// true only when a rear/side channel is present, so a padded layout carrying
/// only front channels does not read as surround.
pub fn classify_surround_input(channels: u8, positions: &[String]) -> SurroundInput {
    let label = match channels {
        1 => "Mono",
        2 => "Stereo",
        4 => "Quad",
        6 => "5.1",
        8 => "7.1",
        _ => "Multichannel",
    };
    let is_true_surround = positions
        .iter()
        .any(|p| matches!(p.to_ascii_uppercase().as_str(), "RL" | "RR" | "SL" | "SR"));
    SurroundInput { channels, label, is_true_surround }
}

/// Pick the richest (highest channel count) negotiated input among the app
/// streams currently linked to one of `surround_sinks` (matched on
/// `sink_node_name`). `None` when no such stream has a known channel count.
pub fn richest_surround_input(
    streams: &[ParsedStream],
    surround_sinks: &[String],
) -> Option<SurroundInput> {
    streams
        .iter()
        .filter(|s| {
            s.sink_node_name
                .as_ref()
                .is_some_and(|n| surround_sinks.iter().any(|x| x == n))
        })
        .filter_map(|s| s.channels.map(|c| (c, s)))
        .max_by_key(|(c, _)| *c)
        .map(|(c, s)| classify_surround_input(c, &s.positions))
}
```

In `crates/audio/src/lib.rs`, update the `streams` re-export:

```rust
pub use streams::{
    classify_surround_input, parse_app_streams, richest_surround_input, ParsedStream, SurroundInput,
};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p arctis-audio --lib streams:: -- --test-threads=1`
Expected: PASS (all new tests).

- [ ] **Step 5: Commit**

```bash
git add crates/audio/src/streams.rs crates/audio/src/lib.rs
git commit -m "feat(audio): classify_surround_input + richest_surround_input selector"
```

---

### Task 3: Remove the dead `parse_stream_channels`

**Files:**
- Modify: `crates/audio/src/sinks.rs` (delete fn ~122-193 and its tests ~306-368)
- Modify: `crates/audio/src/lib.rs` (drop from `sinks` re-export)

**Interfaces:**
- Consumes: nothing.
- Produces: removes `parse_stream_channels` from the public API (its functionality now lives in `parse_app_streams` / `parse_node_format`).

- [ ] **Step 1: Confirm there are no non-test callers**

Run: `grep -rn "parse_stream_channels" crates src-tauri`
Expected: matches only in `crates/audio/src/sinks.rs` and `crates/audio/src/lib.rs` (no `engine`/`cli`/`src-tauri` call sites).

- [ ] **Step 2: Delete the function and its tests**

In `crates/audio/src/sinks.rs`, delete the entire doc-comment + `pub fn parse_stream_channels(...) -> Option<u8> { … }` block (the function spanning the `/// Parse the negotiated channel count …` doc comment through its closing `}`), and delete its test block — the `// ── TDD: parse_stream_channels …` comment line through the last `parse_stream_channels_*` test (`parse_stream_channels_matches_on_node_name_too`), leaving the surrounding `parse_node_volume` tests intact.

In `crates/audio/src/lib.rs`, change the `sinks` re-export to drop `parse_stream_channels`:

```rust
pub use sinks::{parse_default_sink_name, parse_node_volume, parse_output_sinks, OutputSink};
```

- [ ] **Step 3: Build + clippy to verify nothing references it**

Run: `cargo clippy -p arctis-audio --all-targets -- -D warnings`
Expected: PASS (no unused-import / unresolved-name errors).

- [ ] **Step 4: Run the audio crate tests**

Run: `cargo test -p arctis-audio -- --test-threads=1`
Expected: PASS (no `parse_stream_channels_*` tests remain; everything else green).

- [ ] **Step 5: Commit**

```bash
git add crates/audio/src/sinks.rs crates/audio/src/lib.rs
git commit -m "refactor(audio): drop dead parse_stream_channels (folded into parse_app_streams)"
```

---

### Task 4: Populate the snapshot in the engine `state()`

**Files:**
- Modify: `crates/engine/src/state.rs` (`SurroundSnapshot` struct ~139-167)
- Modify: `crates/engine/src/engine.rs` (`state()` surround block ~604-625; add a test in the `tests` module)

**Interfaces:**
- Consumes: `arctis_audio::{parse_app_streams, richest_surround_input}` (Tasks 1–2); `pw_dump_json` local in `state()`.
- Produces: `SurroundSnapshot.negotiated_channels` is now populated (was always `None`); new field `pub negotiated_surround: Option<bool>`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/engine/src/engine.rs`:

```rust
    #[test]
    fn state_reports_negotiated_surround_input_for_game_channel() {
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround.enabled = true;
        cfg.profiles[0].surround.channels = vec!["game".into()];

        // DayZ 7.1 stream linked to the Arctis_Game sink.
        let dump = r#"[
          { "id": 50, "type": "PipeWire:Interface:Node",
            "info": { "props": { "media.class": "Audio/Sink", "node.name": "Arctis_Game" } } },
          { "id": 51, "type": "PipeWire:Interface:Node",
            "info": { "props": { "media.class": "Stream/Output/Audio",
                "application.name": "DayZ", "application.process.binary": "DayZ" },
              "params": { "Format": [ { "channels": 8,
                "position": ["FL","FR","FC","LFE","RL","RR","SL","SR"] } ] } } },
          { "id": 99, "type": "PipeWire:Interface:Link",
            "info": { "output-node-id": 51, "input-node-id": 50 } }
        ]"#;
        let runner = MockRunner::new().with_output(0, dump, "");
        let mut engine = Engine::new(runner, cfg);
        let st = engine.state();
        assert_eq!(st.surround.negotiated_channels, Some(8));
        assert_eq!(st.surround.negotiated_surround, Some(true));
    }

    #[test]
    fn state_reports_none_surround_input_when_no_game_stream() {
        let mut cfg = make_config_no_eq_no_routes();
        cfg.profiles[0].surround.enabled = true;
        cfg.profiles[0].surround.channels = vec!["game".into()];
        // pw-dump with no app streams.
        let runner = MockRunner::new().with_output(0, "[]", "");
        let mut engine = Engine::new(runner, cfg);
        let st = engine.state();
        assert_eq!(st.surround.negotiated_channels, None);
        assert_eq!(st.surround.negotiated_surround, None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-engine state_reports_negotiated -- --test-threads=1`
Expected: FAIL — `no field 'negotiated_surround' on type 'SurroundSnapshot'`.

- [ ] **Step 3: Add the struct field**

In `crates/engine/src/state.rs`, inside `SurroundSnapshot`, add after `negotiated_channels`:

```rust
    /// Whether the negotiated surround input has a rear/side channel (true 7.1/5.1)
    /// vs only stereo. `None` = no probe / no source feeding a surround channel.
    /// Old engine versions omit this field.
    #[serde(default)]
    pub negotiated_surround: Option<bool>,
```

- [ ] **Step 4: Populate it in `state()`**

In `crates/engine/src/engine.rs`, inside the `let surround = if let Ok(p) = self.config.active() {` block, after `let sc = &p.surround;` and before constructing `SurroundSnapshot`, add:

```rust
            // Probe the negotiated input layout of whatever app feeds a surround
            // channel (reuses the cached pw-dump; read-only). Richest source wins.
            let surround_sinks: Vec<String> = p
                .channels
                .iter()
                .filter(|c| sc.channels.iter().any(|id| id == &c.id))
                .map(|c| c.node_name.clone())
                .collect();
            let neg = arctis_audio::parse_app_streams(&pw_dump_json)
                .ok()
                .and_then(|streams| arctis_audio::richest_surround_input(&streams, &surround_sinks));
            let (negotiated_channels, negotiated_surround) = match &neg {
                Some(si) => (Some(si.channels), Some(si.is_true_surround)),
                None => (None, None),
            };
```

Then in the `crate::state::SurroundSnapshot { … }` literal, replace `negotiated_channels: None,` with:

```rust
                negotiated_channels,
                negotiated_surround,
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p arctis-engine state_reports_negotiated state_reports_none_surround -- --test-threads=1`
Expected: PASS (both tests).

- [ ] **Step 6: Workspace clippy + tests**

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace -- --test-threads=1`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/engine/src/state.rs crates/engine/src/engine.rs
git commit -m "feat(engine): populate negotiated_channels + negotiated_surround in state()"
```

---

### Task 5: Frontend — IPC type, banner helper, Spatial-page render

**Files:**
- Modify: `frontend/src/lib/ipc.ts` (`SurroundSnapshot` interface ~83-101)
- Modify: `frontend/src/lib/surround.ts` (add `surroundInputStatus`)
- Modify: `frontend/src/lib/surround.test.ts` (tests)
- Modify: `frontend/src/lib/components/SpatialPage.svelte` (derive + render)

**Interfaces:**
- Consumes: `SurroundSnapshot.negotiated_channels` (exists) + new `negotiated_surround`.
- Produces: `surroundInputStatus(channels, isTrueSurround) -> { text: string; tone: "ok" | "warn" | "muted" }`.

- [ ] **Step 1: Write the failing test**

Add to `frontend/src/lib/surround.test.ts`:

```ts
import { surroundInputStatus } from "./surround.js";

describe("surroundInputStatus", () => {
  it("reports true surround as ok with the layout label", () => {
    expect(surroundInputStatus(8, true)).toEqual({ text: "Input: 7.1 surround ✓", tone: "ok" });
    expect(surroundInputStatus(6, true)).toEqual({ text: "Input: 5.1 surround ✓", tone: "ok" });
  });
  it("warns when the source is only stereo / has no rear channels", () => {
    expect(surroundInputStatus(2, false)).toEqual({
      text: "Input: Stereo — game not sending surround",
      tone: "warn",
    });
    expect(surroundInputStatus(8, false)).toEqual({
      text: "Input: Stereo — game not sending surround",
      tone: "warn",
    });
  });
  it("is muted when nothing is feeding a surround channel", () => {
    expect(surroundInputStatus(null, null)).toEqual({
      text: "Input: no surround source active",
      tone: "muted",
    });
    expect(surroundInputStatus(undefined, undefined)).toEqual({
      text: "Input: no surround source active",
      tone: "muted",
    });
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm -C frontend test`
Expected: FAIL — `surroundInputStatus is not a function` / import error.

- [ ] **Step 3: Add the IPC field and the helper**

In `frontend/src/lib/ipc.ts`, inside `SurroundSnapshot`, after `negotiated_channels`:

```ts
  /** Whether the negotiated surround input has rear/side channels (true 7.1/5.1) vs stereo. null = no source. */
  negotiated_surround?: boolean | null;
```

Append to `frontend/src/lib/surround.ts`:

```ts
// ---------------------------------------------------------------------------
// Surround input indicator (true 7.1 vs stereo)
// ---------------------------------------------------------------------------

/** Map channel count to a layout label for the input banner. */
function inputLayoutLabel(channels: number): string {
  switch (channels) {
    case 8: return "7.1";
    case 6: return "5.1";
    case 4: return "Quad";
    default: return "Surround";
  }
}

/**
 * Status line for the Spatial page showing whether the game routed into a
 * surround channel is actually sending surround (rear/side channels present)
 * vs stereo. `channels`/`isTrueSurround` come from the engine snapshot's
 * negotiated_channels / negotiated_surround.
 */
export function surroundInputStatus(
  channels: number | null | undefined,
  isTrueSurround: boolean | null | undefined,
): { text: string; tone: "ok" | "warn" | "muted" } {
  if (channels == null) {
    return { text: "Input: no surround source active", tone: "muted" };
  }
  if (isTrueSurround) {
    return { text: `Input: ${inputLayoutLabel(channels)} surround ✓`, tone: "ok" };
  }
  return { text: "Input: Stereo — game not sending surround", tone: "warn" };
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `pnpm -C frontend test`
Expected: PASS (all surround tests incl. `surroundInputStatus`).

- [ ] **Step 5: Render the banner on the Spatial page**

In `frontend/src/lib/components/SpatialPage.svelte`, add `surroundInputStatus` to the import from `../surround.js` (the `import { … } from "../surround.js"` block):

```ts
    formatBlocksize,
    surroundInputStatus,
  } from "../surround.js";
```

Add a derived value after `const blocksizeLabel = …` (~line 63):

```ts
  const inputStatus = $derived(
    surroundInputStatus(surround?.negotiated_channels, surround?.negotiated_surround),
  );
```

In the VIRTUAL SURROUND card body, after the BLOCKSIZE `field-row` (the block ending `</div>` for blocksize, ~line 319) and still inside `.card-body`, add:

```svelte
        <div class="field-row">
          <span class="field-label">INPUT</span>
          <span class="field-value input-status input-status--{inputStatus.tone}">{inputStatus.text}</span>
        </div>
```

Add to the `<style>` block (near `.field-value`):

```css
  .input-status--ok { color: var(--ss-accent); }
  .input-status--warn { color: var(--ss-warning); }
  .input-status--muted { color: var(--ss-text-tertiary); }
```

- [ ] **Step 6: Type-check the frontend**

Run: `pnpm -C frontend check`
Expected: PASS (no svelte-check / tsc errors).

- [ ] **Step 7: Commit**

```bash
git add frontend/src/lib/ipc.ts frontend/src/lib/surround.ts frontend/src/lib/surround.test.ts frontend/src/lib/components/SpatialPage.svelte
git commit -m "feat(ui): surround input indicator (true 7.1 vs stereo) on Spatial page"
```

---

## Post-implementation verification (manual, on target hardware)

The owner verifies live (per project memory: read-only, no audio writes needed here):

1. Restart the daemon (engine change requires it — see project memory).
2. With **DayZ running**, open the Spatial page → INPUT row should read `Input: 7.1 surround ✓`.
3. Close DayZ → INPUT row should read `Input: no surround source active`.
4. (Optional) A stereo-only game on the game channel → `Input: Stereo — game not sending surround`.

Cross-check against ground truth:
`pw-dump | grep -A12 '"application.name": "DayZ"'` should show `"channels": 8` with `RL`/`RR`/`SL`/`SR` in `position`.

## Self-Review

- **Spec coverage:** §1 parsing → Task 1; §2 classifier → Task 2; dead-code fold/remove → Task 3; §3 engine wiring + §4 state struct → Task 4; §4 IPC + §5 UI → Task 5; testing → tests in every task; non-goals (no `resolve_effective_mode` change, no CLI formatting, richest-wins) honored — `state()` leaves the existing `resolve_effective_mode(sc.mode, None)` call untouched. ✓
- **Placeholder scan:** none — every code/test/command step is concrete. ✓
- **Type consistency:** `SurroundInput { channels: u8, label: &'static str, is_true_surround: bool }` used identically in Tasks 2 & 4; `negotiated_surround: Option<bool>` (Rust) ↔ `negotiated_surround?: boolean | null` (TS); `surroundInputStatus(channels, isTrueSurround)` signature matches its caller in SpatialPage. ✓
