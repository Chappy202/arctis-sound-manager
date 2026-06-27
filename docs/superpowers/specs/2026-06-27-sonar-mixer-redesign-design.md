# Sonar-style Mixer Redesign — Design Spec

**Date:** 2026-06-27
**Status:** Approved design → ready for implementation planning
**Scope:** The GUI Mixer view + the engine/protocol/CLI capabilities it depends on.

## 1. Problem & Goal

The current Mixer requires the user to **manually type** an app binary and a target sink to
route audio. There is no way to see which apps are currently producing sound, where they are
routed, or whether a routing rule actually took effect. This is the opposite of the
SteelSeries Sonar experience, where running apps are **discovered automatically**, shown as
pills under the channel they currently play through, and **dragged** between channels.

**Goal:** Bring the Mixer to Sonar parity (Classic mode):

- Automatic discovery of running application audio streams, shown live.
- Drag-and-drop of apps between channels; bindings persist across app restarts.
- Full Sonar layout: a **Master** strip, the existing playback channels plus a default
  **Aux** channel, a **Mic** strip, and a **CHATMIX** slider.
- **Keep** the two things the user values in the current app: per-channel **output-device
  selection** and user-addable **custom virtual channels**.

**Explicitly out of scope (future spec):** *Streamer mode* — the per-channel dual mix
(Personal vs Stream) and the separate virtual "Stream" output for OBS. The strips are
designed so a second slider/row can be added later without a rewrite.

## 2. Decisions (locked)

1. **Classic mode now**, Streamer mode later. One volume slider per channel.
2. **Default-sink + staging tray.** A channel can optionally be set as the system default
   output (toggle) so new apps auto-land in a managed channel; unassigned/bypassing apps show
   in an "Apps to be routed" tray on the Master strip and are dragged out from there.
3. **Auto-seed standard channels on load.** On daemon start, ensure every profile has
   `game, chat, media, aux`. Custom channels preserved. Existing profiles gain Aux with no
   manual step.
4. **Replace the manual route form** with drag/drop. Keep a small collapsible **"Remembered
   routes"** list (view + clear only; no manual binary typing).

## 3. Research grounding (validated, not assumed)

Three findings drive correctness and resolve open questions:

- **Include pulse-compat streams.** PipeWire exposes every PulseAudio-compat client as an
  ordinary `Stream/Output/Audio` node carrying `client.api == "pipewire-pulse"`. The old
  Python app *skipped* these (it had a separate `pulsectl` path); for us, skipping them would
  **hide Chrome, Spotify, Discord** and most apps. We enumerate **all**
  `Stream/Output/Audio` nodes uniformly.
- **Current sink via Link objects.** A stream's *actual* sink is found by walking
  `PipeWire:Interface:Link` objects (`output-node-id` → `input-node-id`), not by reading
  `target.object` metadata (which is intent, not reality). **Topology note (verified):** each
  of our channels is a single sink whose exposed `node.name` *is* the channel sink (e.g.
  `Arctis_Game`) — streams link directly to it; there is no `effect_input.*` indirection (that
  was the old Python app's filter-chain design, not ours; confirmed by `meters.rs` capturing
  `Arctis_Game.monitor`). So the sink→channel reverse map is simply
  `{ node_name → channel_id }`, built **dynamically from the active profile's channels**, not
  hard-coded.
- **`target.object` accepts a `node.name`.** Confirmed against PipeWire docs
  (`PW_KEY_TARGET_OBJECT`: "an object name or object.serial"). Our existing
  `routing.rs::move_stream_argv` (which passes the sink `node.name`) is **already correct**;
  the deprecated `node.target` key can be dropped from the WirePlumber fragment over time.

Identity for persistence: **`application.process.binary`** (with `application.name` as a
fallback / disambiguator for generic Electron/Chromium binaries). Never persist the numeric
node id or `object.serial` (session-ephemeral); those are used only as the *live* move
subject.

Freshness: a debounced `pw-dump` poll at ~1.5s in the daemon. No `pipewire-rs` on the hot
path (per ARCHITECTURE guardrails). A `pw-mon`/`pw-dump -m` event trigger is a possible later
optimization, not part of this spec.

## 4. Architecture

### 4.1 Channel kinds

The mixer renders three strip kinds. Only **Playback** strips are drag targets.

| Kind | Backed by | Drop target | Controls |
|---|---|---|---|
| **Master** | new `Profile` fields `master_volume_db: f32`, `master_mute: bool` | no | volume, mute, level meter, **Apps-to-be-routed tray** |
| **Playback** (`game`,`chat`,`media`,`aux`,+custom) | existing `ChannelConfig` | **yes** | volume, mute, **output-device dropdown**, EQ button, level meter, **app pills** |
| **Mic** | existing `MicSnapshot` | no | mic level, mute, "Edit" → Mic page |

- **Master = output gain, not a new sink.** Implemented as a gain applied to the headset
  hardware output sink via `wpctl`. It scales the overall mix (Sonar behavior) with zero
  audio-graph topology change. `master_mute` mutes that output.
- **Aux = a normal playback channel** (`id: "aux"`, `node_name: "Arctis_Aux"`). Added to the
  default channel set and auto-seeded into existing profiles.
- **Mic = existing source.** The strip surfaces mic level + mute (reusing `MicEnable` /
  hardware mute) and links to the Mic page. No new mic sink, no app drops.

### 4.2 App-stream snapshot

```rust
// crates/engine/src/state.rs
pub struct AppStream {
    pub id: u32,                       // live PipeWire node id (move subject; not persisted)
    pub binary: String,                // application.process.binary (persistence key)
    pub app_name: String,              // application.name (UI label)
    pub pid: Option<u32>,              // application.process.id (session correlation only)
    pub icon_name: Option<String>,     // application.icon-name (UI icon)
    pub media_name: Option<String>,    // media.name (UI subtitle, volatile)
    pub current_channel: Option<String>, // resolved channel id, or None = unrouted (tray)
    pub routed: bool,                  // a persistent RouteRule exists for this binary
}
```

Surfaced to the GUI via a dedicated `streams-changed` Tauri event (emitted by a src-tauri
poll task, see §7.1), kept in a **separate frontend store** from `EngineState` so frequent
stream churn does not re-render the whole app.

### 4.3 Sink → channel reverse map

The engine owns each channel's `node_name` and its EQ effect-node name. It builds a map
`{ node_name | effect_node_name → channel_id }` and uses it to resolve a stream's linked sink
back to a channel id. Anything that resolves to a non-channel sink (e.g. the raw headset
output) is **unrouted** and appears in the Master tray.

## 5. Protocol / engine additions

Each new verb follows the established 7-layer parity recipe:
`protocol.rs` → `daemon.rs` handler → `engine.rs` method → `state.rs` `Event` →
`src-tauri/commands.rs` → `cli/main.rs` subcommand → `frontend/lib/ipc.ts` + GUI control.

| Request | Engine method | Behavior |
|---|---|---|
| `ListStreams` | `list_streams() -> Vec<AppStream>` | One-shot discovery (CLI / initial GUI load). |
| `MoveStream { stream, channel }` | `move_stream(stream, channel)` | Live move (by node id) **+** persist `RouteRule(binary→sink)`. Reuses `Router::apply_live` + `set_rule` + `save_persistent`. |
| `SetMasterVolume { volume_db }` | `set_master_volume(db)` | Gain on headset output via `wpctl`; store in profile. |
| `SetMasterMute { muted }` | `set_master_mute(b)` | Mute headset output; store in profile. |
| `SetChatmix { position }` | `set_chatmix(pos)` | Reuses `dial.rs::apply_dial_balance` (Game↔Chat). Store position in profile. |
| `SetDefaultSinkChannel { channel: Option<String> }` | `set_default_sink_channel(opt)` | `wpctl set-default` on that channel's sink; store in profile (`default_sink_channel`). `None` = leave system default untouched. |

New engine `Event` variants: `MasterVolumeSet`, `MasterMuteSet`, `ChatmixSet`,
`DefaultSinkChannelSet` (emitted by their engine methods, existing pattern). Stream freshness
is **not** an engine event — it is the `streams-changed` Tauri event emitted by a src-tauri
poll task calling `ListStreams` (§7.1).

New `Profile`/`Config` fields: `master_volume_db`, `master_mute`, `chatmix_position`,
`default_sink_channel: Option<String>`. All `#[serde(default)]` for backward compatibility
with existing config files.

**`ListStreams` payload:** add `streams: Option<Vec<AppStream>>` to `Response` (mirrors the
existing `text` / `coexist_report` optional-payload pattern) rather than overloading
`EngineState`.

## 6. CLI surface (parity)

New `streams` command group, mirroring `route`/`channel` style:

- `asm-cli streams list` — print discovered apps, their channel, and routed flag.
- `asm-cli streams move <stream> <channel>` — `stream` = node id or binary; `channel` = id.
- `asm-cli master volume <db>` / `asm-cli master mute <on|off>`
- `asm-cli chatmix <position>`
- `asm-cli default-sink set <channel>` / `asm-cli default-sink clear`

## 7. GUI design (Classic mode)

```
┌ MASTER ┬ GAME ┬ CHAT ┬ MEDIA ┬ AUX ┬ (+custom) ┬ MIC ┬ +Add ┐
│ vol    │ vol  │ vol  │ vol   │ vol │           │ lvl │      │
│ mute   │ mute │ mute │ mute  │ mute│           │mute │      │
│ meter  │ out▾ │ out▾ │ out▾  │ out▾│           │Edit │      │
│ ┌tray┐ │ EQ   │ EQ   │ EQ    │ EQ  │           │     │      │
│ │pill│ │┌pill┐│┌pill┐│┌pill┐ │     │           │     │      │
│ └────┘ │└────┘│└────┘│└────┘ │     │           │     │      │
└────────┴──────┴──────┴───────┴─────┴───────────┴─────┴──────┘
┌──────────────  CHATMIX   Game ◄────────►  Chat  ──────────────┐
└───────────────────────────────────────────────────────────────┘
```

- **Per-channel accent colors** (Sonar-like); pills carry the channel accent + app icon
  (`icon_name`) + `app_name`, with `media_name` as a subtitle when present.
- **Drag/drop:** native HTML5 `draggable` pills; `dragover`/`drop` on Playback strips with a
  drop-target highlight (`--ss-drag-highlight` token). On drop → `moveStream(id, channelId)`,
  optimistic pill move, reconciled by the next `streams-changed`. Dragging a pill back to the
  Master tray clears its binding.
- **Apps-to-be-routed tray** lives on the Master strip; shows `current_channel == None`
  streams. A default-sink toggle on Master controls whether new apps auto-land in a channel.
- **CHATMIX** is a full-width slider (Game↔Chat balance) below the strips. It greys out with
  an explanatory note when the hardware Nova Pro dial is the active balance source.
- **Remembered routes:** a collapsible list under the mixer showing persisted
  `(binary → channel)` rules with per-row clear. No manual entry.
- Reuse existing `LevelMeter.svelte`, the output-device dropdown, the EQ button, and the
  add-channel affordance. Replace `RouteList.svelte`'s manual form.

### 7.1 Live update flow

`init()` calls `ListStreams` once for an immediate render, then subscribes to a dedicated
`streams-changed` Tauri event (separate from `state-changed`). A src-tauri poll task in
`src-tauri/src/lib.rs` (a third `setup` task alongside the existing `state-changed` and
`levels` tasks) sends `ListStreams` to the daemon ~every 1.5s and emits `streams-changed` with
the `Vec<AppStream>`; the frontend updates a `streams` store; only the pills re-render.

## 8. Error handling

- **Discovery failure** (`pw-dump` non-zero / parse error): keep last-known streams, show a
  subtle "stale" indicator, never crash or clear the UI.
- **Move failure:** revert the optimistic pill, surface a typed error toast; the persistent
  rule is only written after the live move is attempted (failures reported, not swallowed —
  G7).
- **Master / default-sink (`wpctl`) failure:** propagate typed `AudioError`, surface in UI.
- **Mic strip** reflects mic-chain availability (disabled state when the chain is off).
- No `unwrap` on runtime paths (G7); all device/subprocess errors surfaced (G2).

## 9. Testing (TDD, engine-first)

Pure-function unit tests with fixtures (extend `crates/audio/tests/fixtures/pw_dump_streams.json`):

- Stream enumeration **including** `client.api == "pipewire-pulse"` streams.
- Current-sink resolution via Link objects, including `effect_input.*` → channel id mapping
  and the "unrouted" case.
- `MoveStream`: persists a `RouteRule` **and** issues the live move (assert exact argv).
- Master gain / default-sink / chatmix → exact `wpctl`/argv command sequences.
- Sink→channel reverse-map construction from channel configs.

Protocol round-trip tests for every new verb (kebab-case `cmd` tag). CLI parser tests for the
new subcommands. Frontend: ipc wrappers, `streams` store update from `streams-changed`, and a
pure drag-reducer unit (which channel a stream belongs to + optimistic update). Live-PipeWire
integration tests remain behind the existing flag; real-hardware ChatMix/default-sink checks
are owner-run.

## 10. Build order (engine-first, per CLAUDE.md)

- **Phase 1 — Discovery core (headless):** `AppStream`, enumeration (pulse fix), Link-based
  sink resolution + reverse map, `ListStreams`, `MoveStream`; protocol + daemon + engine +
  CLI + tests. Verifiable via `asm-cli streams list` / `streams move`.
- **Phase 2 — Mixer model (headless):** Master volume/mute, ChatMix, default-sink toggle,
  Aux default + auto-seed; protocol + daemon + engine + CLI + tests.
- **Phase 3 — GUI redesign:** Master/Playback/Mic strips, apps tray, drag/drop, CHATMIX
  slider, remembered-routes list, accent theming, `streams-changed` wiring; full
  GUI↔CLI↔daemon parity.

## 11. Risks & mitigations

- **Generic binaries** (`electron`, `chrome`) collide across apps → persistence may mis-match.
  Mitigation: persist binary + `application.name` disambiguator; document the limitation.
- **Filter-chain internal names** drift if EQ node naming changes → reverse map must be built
  from the live channel/EQ config, never hard-coded.
- **Default-sink change is system-wide** → strictly opt-in via the Master toggle; reversible.
- **`pw-dump` cost** at high poll rates → debounce to ~1.5s; never below 1s.
- **Parity drift** → every verb lands in all 7 layers in the same change; CLI parser + ipc
  tests guard it.
