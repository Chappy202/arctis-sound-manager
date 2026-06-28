# Mixer Fixes & Daemon Resilience — Design Spec

**Date:** 2026-06-27
**Status:** Approved (decisions captured below); ready for planning.
**Authoritative refs:** `ARCHITECTURE.md` (G1–G10), `DESIGN.md`, project memory.

## Problem

The owner reported the mixer GUI as glitchy and partly non-functional:

1. **"Arctis_…" badges in the master app list** — junk entries that aren't real apps.
2. **Per-channel Output dropdown shows no real devices** — can't pick the headset/speakers.
3. **Moving apps between channels "makes no difference"** — routing appears inert.
4. **Channel volume bars "don't work."**
5. **Profile selector "flawed."**

## Root causes (confirmed by live read-only investigation on the target machine)

- **R5 (badges):** `arctis_audio::parse_app_streams` (`crates/audio/src/streams.rs`) includes
  our own EQ filter-chain output nodes. They are `Stream/Output/Audio` nodes with **no app
  identity** (`application.process.binary`/`application.name` absent) but carry
  `node.link-group = "filter-chain-<pid>-<n>"`. The parser falls back to `node.name`
  (`Arctis_Game.output`, `Arctis_Media.output`, …) and surfaces them as apps.
  Live evidence: `pw-dump` props + `asm-cli streams list`.

- **R2 (no devices):** The engine **never enumerates output sinks**. `EngineState` has no device
  list; `buildDeviceOptions` in `ChannelStrip.svelte` is a stub returning only
  "Default (follow system)" + the already-set device. Real sinks (Arctis headset, Speakers,
  MacBook AirPlay) exist in `pw-dump` but never reach the UI.

- **R3 (inert routing):** Every virtual channel's chain output links to the **system default sink**
  (the onboard `alsa_output.pci-…analog-stereo`) because each channel's `output_device = None`
  ⇒ "follow default". The headset is a *different* device
  (`alsa_output.usb-SteelSeries_Arctis_Nova_Pro_Wireless-00.analog-stereo`). So apps routed to any
  channel land on the same physical output → no audible difference. Engine routing itself **works**
  (CLI moved a stream between sinks correctly); the problem is the shared/default output target,
  compounded by R5 (dragging non-apps).

- **R4/R1 (writes don't stick):** The **daemon had crashed**, leaving a stale socket
  (`connect: Connection refused`). A graceful/SIGTERM shutdown removes the socket; a stale socket
  ⇒ **abnormal death** (an unwound panic in the accept loop, or SIGKILL). With the daemon dead,
  every GUI write fails and the optimistic UI reverts ⇒ "nothing sticks / glitchy". Two structural
  gaps make this catastrophic and invisible:
  - The daemon has **no `catch_unwind`** around request handling — any panic in `handle_request`
    unwinds out of the accept loop and kills the whole process (skipping socket cleanup → stale
    socket). (Confirmed: zero `catch_unwind` in the codebase.)
  - The GUI has **no dead-daemon detection/reconnect** beyond the initial load; write failures in
    `ChannelStrip` are swallowed to `console.error`, so the user sees a silent revert.
  The exact panic trigger is **not yet pinned** (not reproducible with read-only ops; owner-run,
  write-permitted reproduction deferred — see Decisions). G7 holds for the scanned runtime paths
  (the one `nodes[0]` index in `config.rs` is guarded by an `is_empty()` check upstream).

## Decisions (owner)

- **D1 — Output model: per-channel device selectors.** Keep a separate Output dropdown on each
  channel, populate it with the real device list, and **default each channel to the detected
  headset** (not the system default). (Chosen over the Sonar single-master-output model.)
- **D2 — Crash handling: harden now, investigate trigger later.** Add panic isolation + stale-socket
  recovery + GUI reconnect UX now. Pin the exact panic backtrace later in an owner-run,
  write-permitted session.

## Non-negotiable constraints (unchanged)

- Device-write safety (ARCHITECTURE G2): never write OLED, never replay unverified opcodes,
  allowlist stays empty until owner validation, single serialized writer, surface failures.
- **No live audio writes during development without explicit per-test owner consent** (see project
  memory `no-live-audio-writes-during-debug`). All automated tests use `MockRunner`/fixtures; no
  test touches real PipeWire/the owner's audio.
- 48 kHz only; PipeWire subprocess model; engine UI-agnostic (tauri only in `src-tauri`).
- Reuse over duplication (G1); typed errors, no `unwrap` on runtime paths (G7); small focused files (G6).

## Design

### F1 — Filter our own infra out of stream discovery (R5)

`parse_app_streams`: after confirming `media.class` starts with `Stream/Output/Audio`, **skip the
node** when it is our own infrastructure:
- `node.link-group` (or `node.group`) starts with `"filter-chain-"`, **or**
- it has neither `application.process.binary` nor `application.name` (no real app identity).

Keep the existing `binary` fallback for genuine apps. This precisely removes `Arctis_*.output`
(and any future EQ/mic chain output) with zero false-positives for real apps (which all carry
`application.name`). Add a `pw-dump` fixture entry for a filter-chain node and assert it is excluded.

*Files:* `crates/audio/src/streams.rs`, `crates/audio/tests/fixtures/pw_dump_app_streams.json`.

### F2 — Enumerate real output sinks and surface them (R2)

New **pure** parser `parse_output_sinks(pw_dump_json) -> Vec<OutputSink>` in `crates/audio`:
- Select `media.class == "Audio/Sink"`.
- **Exclude our virtual sinks**: `node.link-group` starts `"filter-chain-"` **or** `node.name`
  starts `"Arctis_"` (our managed channel sinks).
- Fields: `node_name`, `description` (`node.description`/`device.description` fallback `node.name`).
- Mark the system default via `default.audio.sink` (parsed from `pw-metadata 0` or the
  `Default/Sink` metadata already available) → `is_default: bool`.

Engine: `Engine::list_output_devices()` runs `pw-dump` (+ default-sink read) and returns
`Vec<OutputSink>`. Surface to the GUI by adding `output_devices: Vec<OutputDeviceSnapshot>` to
`EngineState` (already polled by the GUI — no new IPC round-trip). Mirror the type in
`frontend/src/lib/ipc.ts`.

GUI: `ChannelStrip.buildDeviceOptions` consumes `$engineState.output_devices`:
`[{null,"Default (follow system)"}, …each real sink as {node_name, description}]`, marking the
default and the headset. Keep the current selection even if absent (stale-device safety).

*Files:* `crates/audio/src/sinks.rs` (new) + fixture/tests; `crates/engine/src/engine.rs`,
`crates/engine/src/state.rs`; `src-tauri/src/commands.rs` (only if a dedicated command is preferred
over the state field); `frontend/src/lib/ipc.ts`, `frontend/src/lib/components/ChannelStrip.svelte`.

### F3 — Default channels to the headset (R3)

- Engine detects the **headset sink** (the SteelSeries Arctis `alsa_output.usb-…` node) from the
  enumerated sinks (match by `node.name` containing `SteelSeries`/`Arctis`, or device api=alsa +
  product match — exact predicate settled in the plan with a fixture).
- **Seed unset channels:** during `reconcile`, when a channel's `output_device is None` and a headset
  sink is present, target the channel's chain at the headset (explicit `target.object`) rather than
  letting it follow the system default. This is applied at the live-graph layer; whether it also
  persists `output_device` in config is settled in the plan (prefer: keep `None` = "auto → headset"
  semantics, document it, so we don't silently rewrite the owner's config).
- The per-channel dropdown (F2) lets the owner override to any real device or to
  "Default (follow system)".

*Files:* `crates/audio/src/channels.rs` / `crates/engine/src/convert.rs` (chain target resolution),
`crates/engine/src/engine.rs` (reconcile seeding + headset detection).

### F4 — Daemon panic isolation + stale-socket recovery (R4, D2)

- Wrap the per-request engine call in `std::panic::catch_unwind(AssertUnwindSafe(...))` inside
  `serve_connection` (`crates/cli/src/daemon.rs`): a caught panic returns
  `Response::err("internal error: <msg>")` and the accept loop **keeps serving**. Install a panic
  hook that logs the message + location to the daemon log.
- **Stale-socket recovery on start:** `run_daemon` already `remove_file`s an existing path; add a
  liveness probe — if an existing socket is *live* (a peer answers `get-state`), refuse to start a
  second instance with a clear message; if it's *stale* (connect refused), remove and bind.
- Provide an optional `systemd --user` unit (`Restart=on-failure`) and document it; the Tauri app's
  daemon-spawn path (if any) gets `Restart`-style supervision. (Packaging detail; flagged, minimal.)

*Files:* `crates/cli/src/daemon.rs`, plus a `packaging/systemd/arctis-sound-manager.service` doc/unit.

### F5 — GUI resilience: detect dead daemon, surface failures (R4, D2)

- App-wide **daemon-disconnected banner** with a Reconnect action (extend the existing
  `loadError` daemon-down card in `MixerPage` to the shell, driven by a shared store) + light
  auto-retry/backoff on the poll.
- **Surface write failures** instead of silently reverting: `ChannelStrip` volume/mute/output
  handlers route errors to the existing mixer error-banner pattern (`dropError`) rather than only
  `console.error`. The optimistic revert stays, but the user sees why.

*Files:* `frontend/src/lib/stores/*` (connection store), `frontend/src/lib/components/AppShell.svelte`,
`MixerPage.svelte`, `ChannelStrip.svelte`.

## Build order (engine-first, per CLAUDE.md)

1. **F1** stream filter (pure, isolated, immediate visible win).
2. **F2** sink enumeration: pure parser → engine → `EngineState` → GUI dropdown.
3. **F3** headset detection + default-to-headset seeding.
4. **F4** daemon panic isolation + stale-socket recovery.
5. **F5** GUI reconnect banner + surfaced write errors.

Each step is independently shippable and testable.

## Testing

- All unit/integration tests use `MockRunner` + `pw-dump` JSON fixtures. **No test performs a live
  audio write.**
- F1/F2: fixture-driven parser tests (infra excluded, real apps kept; virtual sinks excluded, real
  sinks kept, default marked).
- F3: reconcile test asserting an unset channel targets the detected headset; override respected.
- F4: a request that panics returns an error `Response` and the daemon keeps serving (inject a
  panicking handler via the test seam); stale vs live socket start behaviour.
- F5: component tests for the disconnected banner + surfaced write error.
- Owner-run (manual, write-permitted, deferred): confirm audible per-channel routing to the headset,
  volume faders, profile create/switch, and capture the original crash backtrace.

## Out of scope

- Sonar single-master-output model (D1 chose per-channel).
- Streamer mode; HRIR/surround changes; device-write allowlist (still owner-gated).
- Pinning the exact historical panic trigger (deferred to an owner-run session — D2).
