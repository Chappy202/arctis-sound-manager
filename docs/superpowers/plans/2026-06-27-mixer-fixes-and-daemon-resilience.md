# Mixer Fixes & Daemon Resilience Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the mixer's app list, output-device selection, channel routing, volume, and profile
actions actually work, and make the daemon survive panics so the GUI stops silently reverting.

**Architecture:** Engine-first. Pure pw-dump parsers in `crates/audio` → engine methods + `EngineState`
fields → Tauri/IPC types → Svelte GUI. Daemon hardened with panic isolation; GUI gains
dead-daemon detection.

**Tech Stack:** Rust workspace (`domain/device/audio/config/engine/client/cli`, `src-tauri`),
Svelte 5 + Vite + Vitest frontend, PipeWire subprocesses (`pw-dump`, `pw-metadata`).

## Global Constraints

- **No live audio writes in any automated test.** Use `MockRunner` + `pw-dump` JSON fixtures only.
- Device-write safety (G2): allowlist stays empty; never write OLED/unverified opcodes.
- Typed errors, no `unwrap`/`expect`/panic on runtime paths (G7). Small focused files (G6). Reuse (G1).
- `~/.cargo/bin/cargo` is the cargo path. Run `cargo test --workspace` + `pnpm -C frontend test`.
- Commit trailers: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` and the
  `Claude-Session:` URL. Never `git add` `.superpowers/` or `.claude/`, or the stray `*.rpm`.
- Canonical fact: headset sink node.name = `alsa_output.usb-SteelSeries_Arctis_Nova_Pro_Wireless-00.analog-stereo`;
  our virtual sinks are `Arctis_Game|Arctis_Chat|Arctis_Media|Arctis_Aux`; our chains carry
  `node.link-group = "filter-chain-<pid>-<n>"`.

---

## Phase 1 — Stream discovery filter (F1 / R5)

### Task 1: Exclude our filter-chain infra from `parse_app_streams`

**Files:**
- Modify: `crates/audio/src/streams.rs`
- Test: same file (`#[cfg(test)] mod tests`) + fixture `crates/audio/tests/fixtures/pw_dump_app_streams.json`

**Interfaces:**
- Consumes: existing `parse_app_streams(&str) -> Result<Vec<ParsedStream>, AudioError>`.
- Produces: same signature; behaviour change only (infra nodes excluded).

- [ ] **Step 1 — Add a failing fixture + test.** Append to the fixture two `Stream/Output/Audio`
  nodes mimicking a filter-chain output: one with `"node.link-group":"filter-chain-99-8"` and
  `"node.name":"Arctis_Game.output"` and **no** `application.*`; keep at least one real app
  (`firefox`/`spotify`). New test:

```rust
#[test]
fn excludes_our_filter_chain_outputs() {
    let streams = parse_app_streams(DUMP).unwrap();
    let bins: Vec<&str> = streams.iter().map(|s| s.binary.as_str()).collect();
    assert!(!bins.iter().any(|b| b.contains(".output")),
        "filter-chain infra must be excluded: {bins:?}");
    assert!(bins.contains(&"firefox"), "real apps must remain: {bins:?}");
}
```

- [ ] **Step 2 — Run, verify it fails.** `~/.cargo/bin/cargo test -p arctis-audio excludes_our_filter_chain_outputs` → FAIL.
- [ ] **Step 3 — Implement the skip.** In the `Stream/Output/Audio` branch, before pushing, read
  `node.link-group` (fallback `node.group`); `continue` if it starts with `"filter-chain-"`. Also
  `continue` when **both** `application.process.binary` and `application.name` are absent (no real
  app identity). Keep the existing `binary` fallback for genuine apps.
- [ ] **Step 4 — Run tests.** `~/.cargo/bin/cargo test -p arctis-audio` → all PASS (existing 5 + new).
- [ ] **Step 5 — Commit.** `fix(audio): exclude EQ filter-chain outputs from app-stream discovery`.

---

## Phase 2 — Output-device enumeration (F2 / R2)

### Task 2: Pure `parse_output_sinks` parser

**Files:**
- Create: `crates/audio/src/sinks.rs`; export from `crates/audio/src/lib.rs`.
- Test: in `sinks.rs`; fixture `crates/audio/tests/fixtures/pw_dump_sinks.json`.

**Interfaces:**
- Produces:
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSink { pub node_name: String, pub description: String, pub is_default: bool }
pub fn parse_output_sinks(pw_dump_json: &str, default_sink_name: Option<&str>)
    -> Result<Vec<OutputSink>, AudioError>;
```

- [ ] **Step 1 — Fixture + failing tests.** Fixture with: headset sink, onboard `analog-stereo`
  (default), a `Arctis_Game` virtual sink, and a `filter-chain-…` node. Tests:

```rust
#[test]
fn lists_real_sinks_excludes_virtual() {
    let s = parse_output_sinks(DUMP, Some("alsa_output.pci-0000_00_1f.3.analog-stereo")).unwrap();
    let names: Vec<&str> = s.iter().map(|x| x.node_name.as_str()).collect();
    assert!(names.iter().any(|n| n.contains("SteelSeries_Arctis")));
    assert!(!names.iter().any(|n| n.starts_with("Arctis_")), "virtual sinks excluded: {names:?}");
    assert!(s.iter().find(|x| x.node_name.contains("analog-stereo")).unwrap().is_default);
}
```

- [ ] **Step 2 — Run, verify fail** (`parse_output_sinks` undefined). 
- [ ] **Step 3 — Implement.** Select `media.class=="Audio/Sink"`; exclude `node.name` starting
  `"Arctis_"` or `node.link-group` starting `"filter-chain-"`; `description` =
  `node.description` else `device.description` else `node.name`; `is_default` = `node.name ==
  default_sink_name`.
- [ ] **Step 4 — Run tests** → PASS.
- [ ] **Step 5 — Commit.** `feat(audio): parse_output_sinks (real sinks, excludes virtuals)`.

### Task 3: Engine `list_output_devices` + `EngineState.output_devices`

**Files:**
- Modify: `crates/engine/src/engine.rs` (new `list_output_devices`, populate in `state()`),
  `crates/engine/src/state.rs` (new `OutputDeviceSnapshot` + `output_devices` field).
- Test: `crates/engine/src/engine.rs` tests with a `MockRunner` queuing pw-dump + default-sink reads.

**Interfaces:**
- Consumes: `arctis_audio::parse_output_sinks`.
- Produces: `EngineState.output_devices: Vec<OutputDeviceSnapshot { node_name, description, is_default }>`.

- [ ] **Step 1 — Failing test.** `state()` (or `list_output_devices`) with a MockRunner returning the
  sinks fixture yields the headset + onboard sinks, excludes virtuals, marks default.
- [ ] **Step 2 — Run, verify fail.**
- [ ] **Step 3 — Implement.** Read default sink (reuse existing default-sink read if present, else
  `pw-metadata 0` parse) + `pw-dump`; map `OutputSink` → `OutputDeviceSnapshot`; fill `state()`.
  On pw-dump error, return an **empty list** (never panic; never fail `state()`).
- [ ] **Step 4 — Run** `~/.cargo/bin/cargo test -p arctis-engine` → PASS.
- [ ] **Step 5 — Commit.** `feat(engine): expose output_devices in EngineState`.

### Task 4: GUI — populate the real device dropdown

**Files:**
- Modify: `frontend/src/lib/ipc.ts` (add `output_devices` to `EngineState` + `OutputDeviceSnapshot`),
  `frontend/src/lib/components/ChannelStrip.svelte` (`buildDeviceOptions`).
- Test: `frontend/src/lib/components/*.test.ts` (unit test `buildDeviceOptions`).

**Interfaces:**
- Consumes: `$engineState.output_devices`.

- [ ] **Step 1 — Failing unit test.** `buildDeviceOptions(channel, devices)` returns
  `[Default, …each device]`, marks the headset, and keeps a stale current selection.
- [ ] **Step 2 — Run** `pnpm -C frontend test` → FAIL.
- [ ] **Step 3 — Implement.** Change `buildDeviceOptions` to take the device list (from
  `$engineState.output_devices`); render label = description, value = node_name; keep
  "Default (follow system)" first; if `channel.output_device` is set but absent from the list,
  append it as a "(unavailable)" option so the select still shows the truth.
- [ ] **Step 4 — Run tests** → PASS; `pnpm -C frontend build` clean (no warnings).
- [ ] **Step 5 — Commit.** `feat(frontend): populate channel Output with real devices`.

---

## Phase 3 — Default channels to the headset (F3 / R3)

### Task 5: Headset detection + default-to-headset on reconcile

**Files:**
- Modify: `crates/engine/src/engine.rs` (reconcile seeding + `detect_headset_sink`),
  `crates/engine/src/convert.rs` / `crates/audio/src/channels.rs` (chain target resolution).
- Test: engine tests (MockRunner) asserting an unset channel targets the headset; explicit override
  respected; no headset present ⇒ falls back to prior behaviour.

**Interfaces:**
- Consumes: `parse_output_sinks` (to find the headset).
- Produces: `Engine::detect_headset_sink(&self) -> Option<String>` (node.name); reconcile uses it.

- [ ] **Step 1 — Failing test.** With sinks fixture incl. headset, after `reconcile()` a channel with
  `output_device=None` builds its chain with `target.object = "<headset node.name>"`. A channel with
  an explicit `output_device` keeps it. With **no** headset in the fixture, behaviour is unchanged.
- [ ] **Step 2 — Run, verify fail.**
- [ ] **Step 3 — Implement.** `detect_headset_sink`: from enumerated sinks pick the one whose
  `node.name` contains `"SteelSeries"`/`"Arctis"` (case-insensitive). In reconcile's channel build,
  resolve the chain target = `output_device` if `Some`, else the detected headset if any, else None
  (current default-follow). **Do not** rewrite `output_device` in config (None keeps meaning
  "auto → headset"); document this in code.
- [ ] **Step 4 — Run** `~/.cargo/bin/cargo test --workspace` → PASS.
- [ ] **Step 5 — Commit.** `feat(engine): default channel output to detected headset`.

---

## Phase 4 — Daemon panic isolation + stale-socket recovery (F4 / R4)

### Task 6: `catch_unwind` around request handling

**Files:**
- Modify: `crates/cli/src/daemon.rs` (`serve_connection` / `handle_request` call site).
- Test: `crates/cli/src/daemon.rs` tests — a handler that panics yields an error `Response` and the
  loop continues.

**Interfaces:** unchanged wire protocol; a panic becomes `Response::err("internal error: …")`.

- [ ] **Step 1 — Failing test.** Add a test seam: drive `serve_connection` with two requests where the
  first triggers a panic in the engine (inject via a `MockRunner` panic or a test-only request) and
  assert: response 1 is `ok:false` with "internal error", response 2 (a `get-state`) is `ok:true`.
- [ ] **Step 2 — Run, verify fail** (panic currently unwinds the test).
- [ ] **Step 3 — Implement.** Wrap the `handle_request(engine, req)` call in
  `std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle_request(engine, req)))`; on
  `Err(payload)` build `Response::err(format!("internal error: {}", downcast_msg(payload)))`.
  Install a process panic hook (in `run_daemon`) that logs message + location to stderr/daemon log.
- [ ] **Step 4 — Run** `~/.cargo/bin/cargo test -p arctis-cli` → PASS.
- [ ] **Step 5 — Commit.** `fix(daemon): isolate request panics so one bad request can't kill the daemon`.

### Task 7: Stale-vs-live socket on start

**Files:**
- Modify: `crates/cli/src/daemon.rs` (`run_daemon` start path).
- Test: unit test for the liveness-probe decision (pure helper: given "connect refused" → remove+bind;
  given a live peer → refuse with message).

- [ ] **Step 1 — Failing test** for a `socket_is_live(path) -> bool` helper (mockable via a tiny
  connect+get-state probe abstraction).
- [ ] **Step 2 — Run, verify fail.**
- [ ] **Step 3 — Implement.** On start: if `path` exists, probe; if live → `eprintln!` "daemon already
  running" and exit non-zero; if stale → `remove_file` and bind (current behaviour, now justified).
- [ ] **Step 4 — Run tests** → PASS.
- [ ] **Step 5 — Commit.** `fix(daemon): distinguish stale vs live socket on start`.

### Task 8: systemd --user unit (supervision, doc-level)

**Files:**
- Create: `packaging/systemd/arctis-sound-manager.service` (+ a short README note).

- [ ] **Step 1 — Write the unit** (`Restart=on-failure`, `ExecStart=…/asm-cli daemon`,
  `WantedBy=default.target`), documented as optional. No code/test cycle (packaging asset).
- [ ] **Step 2 — Commit.** `chore(packaging): optional systemd --user unit with restart-on-failure`.

---

## Phase 5 — GUI resilience (F5 / R4)

### Task 9: Dead-daemon banner + auto-retry

**Files:**
- Create: `frontend/src/lib/stores/connection.ts` (shared connected/disconnected state + backoff).
- Modify: `frontend/src/lib/components/AppShell.svelte` (banner), `MixerPage.svelte` (reuse store).
- Test: `frontend/src/lib/stores/connection.test.ts` + a component test for the banner.

- [ ] **Step 1 — Failing test.** Store transitions to `disconnected` after a poll failure and back to
  `connected` on success; banner renders a Reconnect button when disconnected.
- [ ] **Step 2 — Run** `pnpm -C frontend test` → FAIL.
- [ ] **Step 3 — Implement.** Connection store updated from the existing poll/init path; AppShell shows
  a non-blocking banner with Reconnect (calls `init()`); light exponential backoff on retry.
- [ ] **Step 4 — Run tests + build** → PASS, no warnings.
- [ ] **Step 5 — Commit.** `feat(frontend): daemon-disconnected banner + auto-reconnect`.

### Task 10: Surface channel write failures

**Files:**
- Modify: `frontend/src/lib/components/ChannelStrip.svelte` (volume/mute/output error surfacing),
  `MixerPage.svelte` (shared error banner prop if needed).
- Test: component test — a failing `setChannelVolume` shows an error, not just `console.error`.

- [ ] **Step 1 — Failing test** asserting the error is surfaced to the UI.
- [ ] **Step 2 — Run** → FAIL.
- [ ] **Step 3 — Implement.** Route handler catch-blocks to the mixer error banner (reuse `dropError`
  pattern) while keeping the optimistic revert.
- [ ] **Step 4 — Run tests + build** → PASS.
- [ ] **Step 5 — Commit.** `feat(frontend): surface channel write failures instead of silent revert`.

---

## Self-review checklist (run after implementation)

- Spec coverage: F1→T1, F2→T2-4, F3→T5, F4→T6-8, F5→T9-10. ✓
- No automated test performs a live audio write (MockRunner/fixtures only). ✓
- Type names consistent: `OutputSink` (audio) → `OutputDeviceSnapshot` (engine/state) →
  `OutputDeviceSnapshot` (ipc.ts). ✓
- Owner-run validation (manual, write-permitted, deferred): per-channel routing audibly reaches the
  headset; faders/profiles work end-to-end; capture the original crash backtrace.
