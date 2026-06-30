# Surround Input Indicator (true 7.1 vs stereo) — Design

**Date:** 2026-06-30
**Status:** Approved (pending spec review)
**Scope:** Single implementation plan.

## Problem

A game configured for HRIR surround (e.g. the **DayZ** factory profile → `Hrir71` on the
`game` channel) only benefits from virtual surround if the game actually **outputs a
multichannel stream**. If the game sends stereo, the HRIR chain receives 2 channels and
there is nothing to spatialize — but the UI gives no feedback, so a misconfigured game
looks identical to a correctly-configured one.

We want a passive **indicator on the Spatial page** showing whether the app(s) routed into a
surround channel are negotiating a real surround layout (rear/side channels present) vs only
stereo.

### Validated reality (target machine, DayZ running, 2026-06-30)

`pw-dump` exposes each stream's *negotiated* `info.params.Format[0]`:

- `DayZ` node → `channels: 8`, `position: [FL, FR, FC, LFE, RL, RR, SL, SR]`, target `Arctis_Game`.
- Chat / Media / Music → `channels: 2`, `position: [FL, FR]`.

So a 2ch game would read `channels: 2 / [FL, FR]` even though the `Arctis_Game` virtual sink
is itself always configured as 8ch. **The app node's negotiated format is the source of
truth** — probing the sink would always report 8ch and never reveal a stereo source.

## Current state (what exists vs what's missing)

- `parse_stream_channels(dump, matcher) -> Option<u8>` exists and is unit-tested with DayZ
  fixtures (`crates/audio/src/sinks.rs:134`), but has **no callers** — dead code.
- `SurroundSnapshot.negotiated_channels: Option<u8>` is wired end-to-end (Rust struct → IPC
  → `frontend/src/lib/ipc.ts:95`) but the engine **hard-codes it to `None`**
  (`crates/engine/src/engine.rs:622`) and no UI renders it.
- `parse_app_streams` (`crates/audio/src/streams.rs:74`) already enumerates app streams and
  resolves each stream's linked sink `node.name` (`ParsedStream.sink_node_name`), but
  `ParsedStream` carries **no channel/format** information.

So ~90% of the plumbing is scaffolded and never connected. This design connects it.

## Decisions

| Decision | Choice |
|---|---|
| Indicator placement | Spatial-page profile banner (reuse `SurroundSnapshot.negotiated_channels`) |
| "True surround" test | Rear/side channel present in the negotiated position map (`RL`/`RR`/`SL`/`SR`) |
| Multiple apps on one surround channel | Richest layout wins (max channels) |
| CLI | GUI only — snapshot field serializes automatically; no dedicated CLI formatting |
| `effective_mode` / routing | **Unchanged** — indicator is passive/additive, must not alter Auto-mode resolution |

## Design

### 1. Pure parsing — `crates/audio/src/streams.rs`

Extend `ParsedStream`:

```rust
pub struct ParsedStream {
    // …existing fields…
    pub sink_node_name: Option<String>,
    pub channels: Option<u8>,       // negotiated channel count
    pub positions: Vec<String>,     // negotiated channel position map, e.g. ["FL","FR","FC",…]
}
```

In `parse_app_streams`, for each stream node read the negotiated format from the same node
object, mirroring `parse_stream_channels`' precedence:

1. `info.props["audio.channels"]` (JSON number or numeric string) for the count.
2. Fallback `info.params.Format[0].channels` for the count.
3. `info.params.Format[0].position` (array of strings) for `positions` (empty `Vec` if absent).

`parse_stream_channels` becomes redundant (its logic now lives in the per-stream path) and is
**removed** along with its tests (G1: reuse over duplication, no dead code). Its DayZ/stereo/5.1
fixtures are migrated into the `parse_app_streams` tests.

### 2. Pure classifier — `crates/audio/src/streams.rs` (or a small sibling module)

```rust
pub struct SurroundInput {
    pub channels: u8,
    pub label: &'static str,     // "7.1" | "5.1" | "Quad" | "Stereo" | "Mono" | "Multichannel"
    pub is_true_surround: bool,  // a rear/side position is present
}

pub fn classify_surround_input(channels: u8, positions: &[String]) -> SurroundInput
```

- `label` by count: 8→"7.1", 6→"5.1", 4→"Quad", 2→"Stereo", 1→"Mono", other→"Multichannel".
- `is_true_surround` = `positions` contains any of `RL`, `RR`, `SL`, `SR` (case-insensitive).
  A padded 8ch stream whose positions are only `FL`/`FR` is therefore **not** true surround.

### 3. Engine wiring — `crates/engine/src/engine.rs` `state()` surround block (~613–625)

Using the `pw_dump_json` already obtained in `state()` (`volume_dump()`, no new subprocess):

1. From the active profile, collect the `node_name`s of channels whose id is in
   `surround.channels` (e.g. `game` → `Arctis_Game`).
2. Parse app streams from `pw_dump_json`; keep those whose `sink_node_name` matches one of
   those surround-channel node names.
3. Pick the richest by `channels` (richest wins). Run `classify_surround_input`.
4. Set `negotiated_channels = Some(count)` and the new
   `SurroundSnapshot.negotiated_surround: Option<bool>` (= `is_true_surround`).
5. If no matching stream (e.g. DayZ closed), both stay `None`.

Failures (malformed/empty pw-dump) degrade to `None` — never panic, never block `state()`
(G7). `negotiated_channels` is **not** passed to `resolve_effective_mode`.

### 4. State struct + IPC — `crates/engine/src/state.rs`, `frontend/src/lib/ipc.ts`

- Add `negotiated_surround: Option<bool>` to `SurroundSnapshot` (default `None`).
- Mirror `negotiated_surround?: boolean | null` in `ipc.ts` (`negotiated_channels` already
  present).

### 5. UI — `frontend/src/lib/components/SpatialPage.svelte`

Below the existing read-only `effective_mode` label, render a status line **only when
surround is enabled**, derived from `surround.negotiated_channels` + `negotiated_surround`:

| State | Text | Tone |
|---|---|---|
| `is_true_surround` true | `Input: 7.1 surround ✓` (label from count) | positive |
| channels ≤ 2 (or no rear) | `Input: Stereo — game not sending surround` | warning |
| `negotiated_channels == null` | `Input: no surround source active` | muted |

A pure helper in `frontend/src/lib/surround.ts` maps `(channels, negotiated_surround)` →
`{ text, tone }` so the component stays declarative and the mapping is unit-testable.

## Data flow

```
pw-dump (cached in state()) ─► parse_app_streams (channels+positions)
        │
        ├─ filter: sink_node_name ∈ surround-channel node_names
        ├─ pick richest by channels
        └─ classify_surround_input ─► SurroundSnapshot { negotiated_channels, negotiated_surround }
                                              │ (serde → IPC)
                                              ▼
                          ipc.ts SurroundSnapshot ─► SpatialPage banner
```

## Testing (TDD)

Pure (no PipeWire):

- `parse_app_streams`: DayZ 8ch fixture → `channels=Some(8)`, positions include `RL`/`SR`;
  stereo fixture → `channels=Some(2)`, positions `[FL,FR]`; `audio.channels` string-prop
  fallback; missing-format → `channels=None`, empty positions.
- `classify_surround_input`: 8ch+rear→7.1/true; 6ch+rear→5.1/true; 8ch with only `FL/FR`→
  not true surround; 2ch→Stereo/false; 1ch→Mono/false.
- `frontend/src/lib/surround.ts` helper: each banner state → expected `{text, tone}`.

Engine integration:

- A `MockRunner` pw-dump where `DayZ` links to `Arctis_Game` and the active profile routes
  `game` through surround → `state().surround.negotiated_channels == Some(8)` and
  `negotiated_surround == Some(true)`.
- Same dump with DayZ at 2ch → `Some(2)` / `Some(false)`.
- No DayZ stream → `None` / `None`.

CI gate: `cargo clippy -D warnings` + `cargo test --workspace` (prefer `--test-threads=1`).

## Non-goals

- No change to routing, `effective_mode`, or Auto-mode resolution.
- No per-channel breakdown in the UI (richest-wins single banner).
- No dedicated CLI formatting (the field serializes via the shared snapshot anyway).
- No device writes — read-only throughout (G2 safety rules unaffected).
