> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to execute this plan task-by-task. Each task below is independently testable; dispatch one subagent per task, in order, and review at each checkpoint. Tasks marked **OWNER-RUN** are manual hardware/PipeWire validation steps the repo owner runs on their real machine — they are NOT part of CI and NOT to be auto-executed by a subagent.

# Multi-Channel Submixes + Per-App Routing + Per-Channel Output Device

**Date:** 2026-06-20
**Spec:** `docs/superpowers/specs/2026-06-20-arctis-sound-manager-design.md` (esp. §5 channel/submix model, §6 audio engine — per-app routing + per-channel output override)
**Guardrails:** `ARCHITECTURE.md` G1 (reuse-not-duplicate), G3 (live/idempotent/48 kHz), G6 (module boundaries / file discipline), G7 (typed errors), G8 (testing split)
**Builds on:** `docs/superpowers/plans/2026-06-20-audio-engine-foundation.md` (the PROVEN single-sink `arctis-audio` engine: `CommandRunner`/`MockRunner`, `SinkSpec`, `render_filter_chain_conf`, `AudioBackend::{create,remove,apply_band,apply_all,find_node_id}`)

## Goal

Extend the proven `arctis-audio` engine from **one** virtual EQ sink to a **set** of named submix sinks (`Arctis_Game`, `Arctis_Chat`, `Arctis_Media`) managed together, and add the two routing features the user loved (and the one that was broken):

1. **Multi-channel manager** — create/remove all configured channels at once, each its own filter-chain EQ sink, by **reusing** the existing single-sink machinery (one `AudioBackend` per channel; do NOT fork the conf/Props/argv logic).
2. **Per-channel output device that ACTUALLY works** (the bug fix) — changing a channel's output **rebuilds/retargets that channel's `playback.props` target** to a chosen physical device and re-spawns the channel's instance so the change is *enforced*, not merely stored. (The old app saved JSON and never retargeted the loopback.)
3. **Per-application routing** — assign a running app (by `application.process.binary` / `application.name`) to a channel sink, applied **LIVE** via `pw-metadata <stream-id> target.object <sink>` and **PERSISTENTLY** via a WirePlumber 0.5 `node.rules` SPA-JSON fragment in `~/.config/wireplumber/wireplumber.conf.d/`. Respect manually-pinned streams; manage `restore-stream` so it doesn't fight the rules.
4. **CLI** — `channels up|down`, `route set <app> <channel>`, `route list`, `channel output set <channel> <device>`, plus an **OWNER-RUN** E2E that validates all of the above on real PipeWire (route a browser to Media, retarget Media to a different device, tear down).

This is one coherent, E2E-testable increment of spec §5/§6. The pure logic (channel-set config, SPA-JSON rule generation, `pw-metadata` argv, stream-id parsing, retarget conf regeneration) is fully unit-tested with **no daemon**; the daemon-touching path is OWNER-RUN and out of CI.

## Architecture

- **No new crate.** All work lands in the existing `crates/audio` (package `arctis-audio`, lib `arctis_audio`), which depends only on `arctis-domain` (crate dependency rule, ARCHITECTURE §2). No `tauri`, no `engine`, no `config`. CLI wiring lands in `crates/cli`.
- **Generalize, don't duplicate (G1).** A new `ChannelManager` holds a `Vec` of channel definitions and constructs one `AudioBackend<R>` per channel from the **existing** `SinkSpec` + `render_filter_chain_conf` path. The per-channel EQ, conf, Props, create/remove, and node-id parsing are the *unchanged* Task-3/4/5 generators from the foundation plan, just driven N times.
- **Subprocess approach through `CommandRunner` (G1).** New external programs used: `pw-metadata` (live stream move), and `pw-dump` (stream enumeration for the parser; falls back to `pw-cli ls Node`). WirePlumber rule files are written to disk (pure string generation, unit-tested against a fixture) and activated by the daemon reading the fragment directory — no daemon API linkage.
- **Two TDD cores, mirroring the foundation plan's "pure generator + MockRunner argv" shape:**
  1. **Pure generators** (no I/O, fixture-tested): `render_channel_set` (config), `node_rules_fragment` (WirePlumber SPA-JSON), `move_stream_argv` (`pw-metadata`), `parse_stream_id` (filter `pw-dump`/`pw-cli` output by binary/app name).
  2. **`CommandRunner`-executed orchestration** (MockRunner argv assertions): `ChannelManager::{up,down,set_output}` and `Router::{apply_live, write_persistent}`.

Data/lifecycle, end to end:

```
ChannelSetConfig (Vec<ChannelDef{ id, node_name, description, output_device }>)
   │  pure generators (no I/O)
   ├─ per channel → SinkSpec → render_filter_chain_conf()  [REUSED from foundation]
   ├─ node_rules_fragment(rules)  ──► SPA-JSON written to ~/.config/wireplumber/wireplumber.conf.d/
   └─ move_stream_argv(stream_id, sink_name) ─► argv for `pw-metadata <id> target.object <sink>`
        │
        ▼  CommandRunner (real: std::process::Command | test: MockRunner)
   ChannelManager  → N× AudioBackend::{create,remove}                 (channels up/down)
   ChannelManager::set_output → regenerate conf w/ new target + re-spawn  (THE BUG FIX, enforced)
   Router::apply_live → parse_stream_id(pw-dump) → pw-metadata move      (live per-app route)
   Router::write_persistent → node_rules_fragment → file               (persistent per-app route)
        │
        ▼  actual PipeWire 1.4 / WirePlumber 0.5 daemon  (touched ONLY by OWNER-RUN tasks, out of CI)
```

Signal path (spec §6, this increment): `app → channel sink (EQ) → playback.props target` where target defaults to the **hardware Arctis sink**. Multiple filter-chain `Audio/Sink` nodes whose `playback.props.target.object` is the same hardware sink **mix at that hardware sink** — see Research basis for why no explicit Master node is needed in this increment.

## Tech Stack

- Rust 2021; workspace edition/lints in root `Cargo.toml`. No new crates, **no new dependencies** (reuse `arctis-domain`, `thiserror`; `serde`/`serde_json` already in the workspace if needed — but the SPA-JSON generator emits a literal string and does NOT require serde).
- `cargo` lives at `~/.cargo/bin/cargo` and is **not on PATH** — every command invokes it as `~/.cargo/bin/cargo`.
- Target runtime: PipeWire **1.4.x** / WirePlumber **0.5.x**, **48000 Hz only**, on the owner's Nobara machine (spec §3). WirePlumber 0.5 config is **SPA-JSON** (Lua dropped).

## Research basis (founded-in-research; cite, don't guess)

Confirmed against current docs (June 2026, PipeWire 1.4 / WirePlumber 0.5):

- **(a) Move a running stream live via metadata.** A stream node is moved by setting the `target.object` key on the stream's node id in the **`default`** metadata object; the value may be a target **`node.name`** *or* an **`object.serial`**. WirePlumber monitors the default metadata for `target.object` changes and re-links the stream. CLI form: `pw-metadata -n default <stream-node-id> target.object "<sink-node-name>"`. Clear with `pw-metadata -d <stream-node-id> target.object` (delete that key for the id) — or set value to an empty/`null` to release. — https://docs.pipewire.org/page_man_pw-metadata_1.html , https://pipewire.pages.freedesktop.org/wireplumber/policies/linking.html , https://docs.pipewire.org/page_streams.html
  - `pw-metadata` positional order is `[id [key [value [type]]]]`; `-n <name>` selects the metadata object (`default`), `-d` deletes. — https://docs.pipewire.org/page_man_pw-metadata_1.html
- **(b) Persistent per-app routing via WirePlumber 0.5 `node.rules` SPA-JSON.** Fragments live in `~/.config/wireplumber/wireplumber.conf.d/` with a `.conf` extension, loaded in alphanumeric order (numeric prefix recommended, e.g. `90-asm-routing.conf`). A rule is an object with a `matches` **array of objects** (key/value pairs AND-ed within an object, objects OR-ed across the array) and an `actions` object whose `update-props` sets properties such as `node.target` / `target.object`. **Lua config is no longer supported in 0.5.** Match key `application.process.binary` (and/or `application.name`). — https://pipewire.pages.freedesktop.org/wireplumber/daemon/configuration/migration.html , https://pipewire.pages.freedesktop.org/wireplumber/daemon/configuration/conf_file.html , https://pipewire.pages.freedesktop.org/wireplumber/daemon/configuration/modifying_configuration.html
- **(c) restore-stream interaction.** WirePlumber's `restore-stream` stores per-stream volume **and target**, and restores them on reconnect; it also reacts to manual `target.object` changes. To stop it from fighting our declarative rules, scope behavior via the `node.rules`/`restore.stream` settings and the well-known settings object. The exact setting keys are listed but their precise semantics for "let an explicit `node.rules` target win over a remembered target" are not fully pinned by the docs → **verify on-machine in the OWNER-RUN task and record the chosen setting**, rather than asserting a guess here. — https://pipewire.pages.freedesktop.org/wireplumber/daemon/configuration/settings.html , https://pipewire.pages.freedesktop.org/wireplumber/policies/linking.html
- **(d) Multiple filter-chain sinks → one hardware sink mix correctly.** Each channel is an independent `libpipewire-module-filter-chain` `Audio/Sink`; its `playback.props` output is a normal PipeWire output stream. Multiple output streams linked to the same hardware sink are summed by the audioconvert/adapter mixer at that sink — this is ordinary PipeWire mixing, exactly how several apps share one output. Therefore **no explicit Master filter node is required in this increment**: each channel's `playback.props.target.object = <hardware sink node.name>` is sufficient, and PipeWire mixes at the hardware sink. A dedicated Master node only becomes worthwhile when we need a single post-mix EQ/limiter or a single point to retarget all channels at once (a LATER plan); calling it out now would be premature. — https://docs.pipewire.org/page_module_filter_chain.html , https://wiki.archlinux.org/title/PipeWire
  - **Verify on-machine (OWNER-RUN):** confirm Game + Media playing simultaneously through two channel sinks both reach the headset (audible mix) and that no rate mismatch is introduced (48 kHz end-to-end).

**NOT fully nailed by docs — verify on-machine, then pin (do not assert a guess):**
- The **stream node id** to feed `pw-metadata` is discovered from `pw-dump`/`pw-cli ls Node` by matching `application.process.binary`/`application.name`. The exact textual layout of those fields in `pw-dump` JSON vs `pw-cli ls Node` is parsed by a single function (Task 3) tested against a **canned fixture**; the OWNER-RUN task captures a real `pw-dump` from the machine and, if the layout differs, the fixture + parser are corrected and the unit test re-run.
- Whether moving a stream should use the target **`node.name`** or **`object.serial`** as the `target.object` value. We default to **`node.name`** (stable, matches our `Arctis_Game` naming) and isolate the choice to one place; OWNER-RUN confirms the move takes effect with the name form and falls back to serial only if needed.
- The `restore-stream` override setting (item c) — pinned in the OWNER-RUN task.

> **Persistence note (record in code comment):** a live `pw-metadata target.object` move is **not** persisted across daemon restart by itself; the WirePlumber `node.rules` fragment is the persistent record. The engine applies **both** (live for instant effect, fragment for durability). Re-apply-on-startup orchestration and full profile persistence are a LATER (engine) plan — this plan writes the fragment file directly but does NOT own profile/config persistence.

## Non-goals (explicitly DEFERRED to later plans)

- HRIR / convolver virtual surround (spec §7).
- The microphone chain (spec §8).
- The engine **orchestrator**, reconciler, event stream, **config/profile persistence**, and **re-apply-on-startup** (this plan writes a WirePlumber fragment directly but does NOT implement profile persistence).
- Any UI (`src-tauri`, `ui/`).
- The headset **Game/Chat dial** integration (device-side HID; spec §5 — later).
- An explicit **Master** mix/EQ node (Research basis (d): not needed for this increment; LATER if a single post-mix stage is wanted).
- Native `pipewire-rs` monitoring / live event subscription (subprocess only here).
- Killing/owning the dedicated `pipewire -c` child processes beyond the foundation plan's best-effort `pkill -f <conf>` (clean child ownership is a LATER engine concern; carried forward unchanged).

## Global Constraints

- **Sample rate:** 48000 Hz only, end-to-end; never emit a resample or non-48k rate in any conf/argv (G3, spec §3).
- **Reuse, not duplicate (G1):** one channel = one existing `AudioBackend` over the existing `SinkSpec` + `render_filter_chain_conf`; the `ChannelManager` orchestrates N of them and MUST NOT re-implement conf/Props/node-id logic.
- **Per-channel output is ENFORCED, not stored:** `set_output` regenerates the channel conf with the new `playback.props` `target.object` and re-spawns that channel's instance; merely recording the device is forbidden (the explicit bug fix).
- **Live + persistent routing (G3):** per-app routing applies live via `pw-metadata <id> target.object <sink>` AND writes a persistent WirePlumber 0.5 `node.rules` SPA-JSON fragment to `~/.config/wireplumber/wireplumber.conf.d/`.
- **Respect manual pins / manage restore-stream (G3):** never override a stream whose target the user pinned; the persistent fragment + a recorded `restore-stream` setting keep our rules from being clobbered.
- **Subprocess approach:** all PipeWire/WirePlumber interaction is external-command driven through `CommandRunner` (`pw-cli`, `pw-metadata`, `pw-dump`, `pipewire -c`); no `pipewire-rs`/`wireplumber` library linkage.
- **WirePlumber 0.5 SPA-JSON only:** persistent rules are SPA-JSON `node.rules` in `wireplumber.conf.d/`; never emit Lua (dropped in 0.5).
- **Typed errors (G7):** all fallible paths return `thiserror`-derived `AudioError`; **no `unwrap()`/`expect()`** on runtime/fallible paths.
- **Idempotent (G3):** channels up/down reconcile against existing sinks (stable `node.name`); writing the fragment and applying a move are idempotent (re-running yields the same state).
- **Testability (G8, spec §14):** all pure generators + orchestration are unit-tested with `MockRunner`/fixtures and **no daemon**; the daemon/hardware path is OWNER-RUN and out of the CI gate.

---

## Task 1 — Channel set config + `ChannelManager` over N `AudioBackend`s (TDD with `MockRunner`)

**Files**
- create `crates/audio/src/channels.rs`
- modify `crates/audio/src/lib.rs` (add `pub mod channels;` + re-exports)

**Interfaces**
- Produces `struct ChannelDef { pub id: String, pub node_name: String, pub description: String, pub output_device: Option<String> }`
  - `id` is the stable logical name (`"game"`, `"chat"`, `"media"`); `node_name` is the PipeWire sink name (`"Arctis_Game"` …); `output_device` is `Some(hardware_sink_node_name)` or `None` (follow default).
- Produces `fn sink_spec(&self) -> SinkSpec` on `ChannelDef` (maps `node_name`/`description`/`output_device` → existing `SinkSpec { node_name, description, playback_target }`).
- Produces `struct ChannelSetConfig { pub channels: Vec<ChannelDef> }` with `fn default_sonar(hardware_sink: Option<&str>) -> Self` → Game/Chat/Media, each `output_device = hardware_sink.map(String::from)`, names `Arctis_Game`/`Arctis_Chat`/`Arctis_Media`.
- Produces `struct ChannelManager<R: CommandRunner> { … }` with:
  - `fn new(runner: R, config: ChannelSetConfig) -> Self`
  - `fn up(&mut self, eq: &EqModel) -> Result<Vec<ConfHandle>, AudioError>` (create every channel idempotently; reuse `AudioBackend::create`)
  - `fn down(&mut self) -> Result<(), AudioError>` (remove every channel idempotently; reuse `AudioBackend::remove`)
  - `fn find(&self, id: &str) -> Option<&ChannelDef>`

**Design notes**
- `ChannelManager` owns the `runner` and constructs a fresh `AudioBackend` per operation by **moving a cheap clone of the runner is not possible** (runner is generic and may be stateful). Therefore the manager borrows `&mut self.runner` per channel by taking the runner **out** for each backend call. To keep it simple and reuse `AudioBackend` exactly, implement the manager to hold `runner: R` and, for each channel, build a backend with a **mutable reference adapter**. Concretely: add a blanket `impl<R: CommandRunner> CommandRunner for &mut R` in `runner.rs` (Task 1 step 1) so `AudioBackend::new(&mut self.runner, spec)` compiles and every channel reuses the one underlying runner (and, for `MockRunner`, records into the one `calls` vec). This is the G1-correct reuse seam.
- Channels are processed in `config.channels` order; errors are returned on the first failure (typed `AudioError`), after which already-created channels remain (idempotent `up` makes a retry safe).

**Steps**

1. Add a forwarding impl so a channel backend can borrow the manager's runner. In `crates/audio/src/runner.rs`, **append** (after the `MockRunner` impl, before `#[cfg(test)]`):
   ```rust
   /// Forward `CommandRunner` through a mutable reference so one runner can be
   /// shared across N per-channel `AudioBackend`s without cloning (G1 reuse seam).
   impl<R: CommandRunner + ?Sized> CommandRunner for &mut R {
       fn run(&mut self, program: &str, args: &[&str]) -> Result<CmdOutput, AudioError> {
           (**self).run(program, args)
       }
       fn spawn_detached(&mut self, program: &str, args: &[&str]) -> Result<(), AudioError> {
           (**self).spawn_detached(program, args)
       }
   }
   ```
   Add a test in the existing `runner.rs` `#[cfg(test)] mod tests`:
   ```rust
   #[test]
   fn mut_ref_runner_forwards_and_records() {
       let mut r = MockRunner::new().with_output(0, "ok", "");
       {
           let mut by_ref = &mut r;
           let out = by_ref.run("pw-cli", &["ls", "Node"]).expect("forwards");
           assert_eq!(out.stdout, "ok");
       }
       assert_eq!(r.calls[0], vec!["pw-cli", "ls", "Node"]);
   }
   ```
   Run (expect fail → then pass after the impl above is present):
   ```
   ~/.cargo/bin/cargo test -p arctis-audio runner::
   ```

2. Create `crates/audio/src/channels.rs` with the model + manager + failing tests:
   ```rust
   use crate::backend::{AudioBackend, ConfHandle};
   use crate::config::SinkSpec;
   use crate::eq::EqModel;
   use crate::error::AudioError;
   use crate::runner::CommandRunner;

   /// One submix channel: a stable logical id, its PipeWire sink node.name,
   /// a human description, and an optional pinned output device (hardware sink
   /// node.name). `output_device = None` follows the default sink.
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub struct ChannelDef {
       pub id: String,
       pub node_name: String,
       pub description: String,
       pub output_device: Option<String>,
   }

   impl ChannelDef {
       pub fn new(
           id: &str,
           node_name: &str,
           description: &str,
           output_device: Option<String>,
       ) -> Self {
           Self {
               id: id.to_string(),
               node_name: node_name.to_string(),
               description: description.to_string(),
               output_device,
           }
       }

       /// Map to the existing single-sink `SinkSpec` (G1 reuse).
       pub fn sink_spec(&self) -> SinkSpec {
           SinkSpec {
               node_name: self.node_name.clone(),
               description: self.description.clone(),
               playback_target: self.output_device.clone(),
           }
       }
   }

   /// The full set of channels managed together.
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub struct ChannelSetConfig {
       pub channels: Vec<ChannelDef>,
   }

   impl ChannelSetConfig {
       /// Sonar-mirroring default: Game / Chat / Media, each pinned to
       /// `hardware_sink` if given (else following the default sink).
       pub fn default_sonar(hardware_sink: Option<&str>) -> Self {
           let hw = hardware_sink.map(String::from);
           Self {
               channels: vec![
                   ChannelDef::new("game", "Arctis_Game", "Arctis Game", hw.clone()),
                   ChannelDef::new("chat", "Arctis_Chat", "Arctis Chat", hw.clone()),
                   ChannelDef::new("media", "Arctis_Media", "Arctis Media", hw),
               ],
           }
       }

       pub fn find(&self, id: &str) -> Option<&ChannelDef> {
           self.channels.iter().find(|c| c.id == id)
       }
   }

   /// Manages the lifecycle of every channel sink by driving the existing
   /// single-sink `AudioBackend` once per channel (G1 — no duplicated logic).
   pub struct ChannelManager<R: CommandRunner> {
       runner: R,
       config: ChannelSetConfig,
   }

   impl<R: CommandRunner> ChannelManager<R> {
       pub fn new(runner: R, config: ChannelSetConfig) -> Self {
           Self { runner, config }
       }

       pub fn config(&self) -> &ChannelSetConfig {
           &self.config
       }

       #[cfg(test)]
       pub fn runner(&self) -> &R {
           &self.runner
       }

       pub fn find(&self, id: &str) -> Option<&ChannelDef> {
           self.config.find(id)
       }

       /// Create every channel sink idempotently. Reuses `AudioBackend::create`.
       pub fn up(&mut self, eq: &EqModel) -> Result<Vec<ConfHandle>, AudioError> {
           let mut handles = Vec::with_capacity(self.config.channels.len());
           for ch in &self.config.channels {
               let spec = ch.sink_spec();
               let mut be = AudioBackend::new(&mut self.runner, spec);
               handles.push(be.create(eq)?);
           }
           Ok(handles)
       }

       /// Remove every channel sink idempotently. Reuses `AudioBackend::remove`.
       pub fn down(&mut self) -> Result<(), AudioError> {
           for ch in &self.config.channels {
               let spec = ch.sink_spec();
               let mut be = AudioBackend::new(&mut self.runner, spec);
               be.remove()?;
           }
           Ok(())
       }
   }

   #[cfg(test)]
   mod tests {
       use super::*;
       use crate::runner::MockRunner;

       fn cfg() -> ChannelSetConfig {
           ChannelSetConfig::default_sonar(Some("alsa_output.arctis"))
       }

       #[test]
       fn default_sonar_has_three_named_channels() {
           let c = ChannelSetConfig::default_sonar(None);
           let names: Vec<&str> = c.channels.iter().map(|c| c.node_name.as_str()).collect();
           assert_eq!(names, vec!["Arctis_Game", "Arctis_Chat", "Arctis_Media"]);
           assert!(c.channels.iter().all(|c| c.output_device.is_none()));
       }

       #[test]
       fn sink_spec_maps_output_device_to_playback_target() {
           let ch = ChannelDef::new("media", "Arctis_Media", "Arctis Media", Some("spk".into()));
           let s = ch.sink_spec();
           assert_eq!(s.node_name, "Arctis_Media");
           assert_eq!(s.playback_target.as_deref(), Some("spk"));
       }

       #[test]
       fn up_creates_every_channel_when_absent() {
           // For each of 3 channels, create() runs: ls Node (absent) + spawn.
           let runner = MockRunner::new()
               .with_output(0, "id 1\n    node.name = \"x\"\n", "") // game ls
               .with_output(0, "", "")                                // game spawn
               .with_output(0, "id 1\n    node.name = \"x\"\n", "") // chat ls
               .with_output(0, "", "")                                // chat spawn
               .with_output(0, "id 1\n    node.name = \"x\"\n", "") // media ls
               .with_output(0, "", "");                               // media spawn
           let mut mgr = ChannelManager::new(runner, cfg());
           let handles = mgr.up(&EqModel::default_10band()).unwrap();
           assert_eq!(handles.len(), 3);
           let calls = &mgr.runner().calls;
           // 3 channels × (1 ls + 1 spawn) = 6 calls.
           assert_eq!(calls.len(), 6);
           assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
           assert_eq!(calls[1][0], "pipewire");
           assert!(calls[1][2].ends_with("Arctis_Game.conf"));
           assert!(calls[5][2].ends_with("Arctis_Media.conf"));
       }

       #[test]
       fn up_is_idempotent_when_all_present() {
           // Each create() sees its sink already present → only the ls check runs.
           let present = "\
   id 10\n    node.name = \"Arctis_Game\"\n\
   id 11\n    node.name = \"Arctis_Chat\"\n\
   id 12\n    node.name = \"Arctis_Media\"\n";
           let runner = MockRunner::new()
               .with_output(0, present, "")
               .with_output(0, present, "")
               .with_output(0, present, "");
           let mut mgr = ChannelManager::new(runner, cfg());
           mgr.up(&EqModel::default_10band()).unwrap();
           // 3 ls checks only; no spawns.
           assert_eq!(mgr.runner().calls.len(), 3);
           assert!(mgr.runner().calls.iter().all(|c| c == &vec!["pw-cli", "ls", "Node"]));
       }

       #[test]
       fn down_removes_every_channel_noop_when_absent() {
           // Each remove() sees its sink absent → only the existence check runs.
           let runner = MockRunner::new()
               .with_output(0, "id 1\n    node.name = \"other\"\n", "")
               .with_output(0, "id 1\n    node.name = \"other\"\n", "")
               .with_output(0, "id 1\n    node.name = \"other\"\n", "");
           let mut mgr = ChannelManager::new(runner, cfg());
           mgr.down().unwrap();
           assert_eq!(mgr.runner().calls.len(), 3);
       }
   }
   ```

3. Add to `crates/audio/src/lib.rs` after `pub mod channels;`-appropriate location (keep modules grouped):
   ```rust
   pub mod channels;
   ```
   and to the re-export block:
   ```rust
   pub use channels::{ChannelDef, ChannelManager, ChannelSetConfig};
   ```

4. Run the gate and commit:
   ```
   ~/.cargo/bin/cargo test -p arctis-audio
   ```
   Expected: `runner::tests::mut_ref_runner_forwards_and_records` + all `channels::tests` (5 tests) pass; full crate suite green; **no daemon touched**. Then:
   ```
   git add crates/audio && git commit -m "audio: ChannelManager generalizes single sink to a managed channel set (G1)"
   ```

---

## Task 2 — WirePlumber 0.5 `node.rules` SPA-JSON generator (pure, TDD against an exact fixture)

**Files**
- create `crates/audio/src/routing.rs`
- create `crates/audio/tests/fixtures/wp_node_rules.conf` (expected SPA-JSON output)
- modify `crates/audio/src/lib.rs`

**Interfaces**
- Produces `struct RouteRule { pub app_binary: String, pub target_sink: String }`
  - `app_binary` matches `application.process.binary`; `target_sink` is a channel sink `node.name` (e.g. `Arctis_Media`).
- Produces `fn node_rules_fragment(rules: &[RouteRule]) -> String` → the **entire** WirePlumber 0.5 `node.rules` SPA-JSON fragment file body.
- Produces `fn wireplumber_fragment_path() -> PathBuf` → `~/.config/wireplumber/wireplumber.conf.d/90-asm-routing.conf` (uses `$HOME`; falls back to `std::env::var("HOME")`).

**Steps**

1. Create the expected fixture `crates/audio/tests/fixtures/wp_node_rules.conf` (two rules: `firefox → Arctis_Media`, `Discord → Arctis_Chat`). This literal string is the contract the generator must reproduce exactly:
   ```
   # Managed by Arctis Sound Manager — do not edit by hand.
   # Persistent per-application routing (WirePlumber 0.5 SPA-JSON node.rules).
   node.rules = [
     {
       matches = [
         {
           application.process.binary = "firefox"
         }
       ]
       actions = {
         update-props = {
           node.target = "Arctis_Media"
           target.object = "Arctis_Media"
         }
       }
     }
     {
       matches = [
         {
           application.process.binary = "Discord"
         }
       ]
       actions = {
         update-props = {
           node.target = "Arctis_Chat"
           target.object = "Arctis_Chat"
         }
       }
     }
   ]
   ```
   > Notes for the generator: emit BOTH `node.target` (legacy/compat key) and `target.object` (current key) inside `update-props` so the rule works regardless of which key the running WirePlumber honours — both name the same sink `node.name`. Two-space indentation per nesting level. A trailing newline ends the file. For an empty `rules` slice, emit the two header comment lines followed by `node.rules = [\n]\n` (verified by the `empty` test below).

2. Create `crates/audio/src/routing.rs` implementing the generator to match the fixture exactly, plus failing tests:
   ```rust
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
   ```

3. Add to `crates/audio/src/lib.rs`:
   ```rust
   pub mod routing;
   ```
   and re-export:
   ```rust
   pub use routing::{node_rules_fragment, wireplumber_fragment_path, RouteRule};
   ```

4. Run and commit:
   ```
   ~/.cargo/bin/cargo test -p arctis-audio routing::
   ```
   Expected: `routing::tests` (3 tests) pass; suite green. If `renders_exact_two_rule_fixture` fails, diff `got` vs `want` and fix the **generator** indentation (the fixture is the contract). Then:
   ```
   git add crates/audio && git commit -m "audio: WirePlumber 0.5 node.rules SPA-JSON generator (persistent routing)"
   ```

---

## Task 3 — `pw-metadata` live-move argv + stream-id parser (pure, TDD against canned fixtures)

**Files**
- modify `crates/audio/src/routing.rs` (add the argv generator + parser + tests)
- create `crates/audio/tests/fixtures/pw_dump_streams.json` (canned `pw-dump` output)

**Interfaces**
- Produces `fn move_stream_argv(stream_id: &str, target_sink: &str) -> Result<Vec<String>, AudioError>`
  - → full argv after the `pw-metadata` program: `["-n", "default", "<id>", "target.object", "<sink>"]`.
- Produces `fn clear_stream_target_argv(stream_id: &str) -> Result<Vec<String>, AudioError>`
  - → `["-d", "<id>", "target.object"]` (release a live move).
- Produces `fn parse_stream_id(pw_dump_json: &str, app_match: &AppMatch) -> Result<String, AudioError>`
  - where `enum AppMatch { Binary(String), Name(String) }` matches `application.process.binary` or `application.name` and returns the stream node id.

**Design notes**
- The `target.object` value is the channel sink **`node.name`** (Research basis default). The metadata object is **`default`** (`-n default`). These two facts are isolated to `move_stream_argv` so an OWNER-RUN correction (e.g. to `object.serial`) is a one-line change.
- `parse_stream_id` reads `pw-dump`'s JSON array of objects. For robustness without pulling in serde for this one parse, scan for the object whose `info.props` contains the matched key/value and whose `type` is `PipeWire:Interface:Node`, then read its `"id":` field. The parser is tested against the canned fixture; the OWNER-RUN task captures a real `pw-dump` and corrects the fixture/parser if the layout differs. (If serde_json is preferred, it is already in the workspace; the executor MAY add `serde_json = { workspace = true }` to `crates/audio/Cargo.toml` and parse structurally — keep the public signature identical either way.)

**Steps**

1. Create the canned fixture `crates/audio/tests/fixtures/pw_dump_streams.json` — a trimmed but realistic two-stream `pw-dump` (a Firefox output stream id 73, a Discord output stream id 88):
   ```json
   [
     {
       "id": 73,
       "type": "PipeWire:Interface:Node",
       "info": {
         "props": {
           "application.name": "Firefox",
           "application.process.binary": "firefox",
           "media.class": "Stream/Output/Audio",
           "node.name": "Firefox"
         }
       }
     },
     {
       "id": 88,
       "type": "PipeWire:Interface:Node",
       "info": {
         "props": {
           "application.name": "Discord",
           "application.process.binary": "Discord",
           "media.class": "Stream/Output/Audio",
           "node.name": "WEBRTC VoiceEngine"
         }
       }
     }
   ]
   ```

2. Append to `crates/audio/src/routing.rs` (after the existing code, before `#[cfg(test)]`):
   ```rust
   use crate::error::AudioError;

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
   /// Lightweight scan (no serde): locates the matched key/value, then reads the
   /// nearest preceding `"id":` within the same object. Verified against a canned
   /// fixture; OWNER-RUN confirms the real layout.
   pub fn parse_stream_id(pw_dump_json: &str, app_match: &AppMatch) -> Result<String, AudioError> {
       let needle = format!("\"{}\": \"{}\"", app_match.key(), app_match.value());
       let Some(match_pos) = pw_dump_json.find(&needle) else {
           return Err(AudioError::Parse {
               what: "stream id".to_string(),
               detail: format!("no stream with {} = {}", app_match.key(), app_match.value()),
           });
       };
       // Scan backwards from the match for the most recent `"id":` field.
       let head = &pw_dump_json[..match_pos];
       let Some(id_kw) = head.rfind("\"id\":") else {
           return Err(AudioError::Parse {
               what: "stream id".to_string(),
               detail: "matched app but no preceding \"id\" field".to_string(),
           });
       };
       let after = &head[id_kw + "\"id\":".len()..];
       let digits: String = after
           .chars()
           .skip_while(|c| c.is_whitespace())
           .take_while(|c| c.is_ascii_digit())
           .collect();
       if digits.is_empty() {
           return Err(AudioError::Parse {
               what: "stream id".to_string(),
               detail: "could not read numeric id".to_string(),
           });
       }
       Ok(digits)
   }
   ```
   Add to the existing `#[cfg(test)] mod tests` in `routing.rs`:
   ```rust
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
       let id = parse_stream_id(&dump, &AppMatch::Binary("firefox".into())).unwrap();
       assert_eq!(id, "73");
   }

   #[test]
   fn parse_stream_id_by_name() {
       let dump = include_str!("../tests/fixtures/pw_dump_streams.json");
       let id = parse_stream_id(&dump, &AppMatch::Name("Discord".into())).unwrap();
       assert_eq!(id, "88");
   }

   #[test]
   fn parse_stream_id_absent_is_typed_error() {
       let dump = include_str!("../tests/fixtures/pw_dump_streams.json");
       let err = parse_stream_id(&dump, &AppMatch::Binary("nope".into())).unwrap_err();
       assert!(matches!(err, AudioError::Parse { .. }));
   }
   ```

3. Re-export from `crates/audio/src/lib.rs` (extend the routing re-export line):
   ```rust
   pub use routing::{
       clear_stream_target_argv, move_stream_argv, node_rules_fragment, parse_stream_id,
       wireplumber_fragment_path, AppMatch, RouteRule,
   };
   ```

4. Run and commit:
   ```
   ~/.cargo/bin/cargo test -p arctis-audio routing::
   ```
   Expected: all new `routing::tests` (6 added) pass; suite green. Then:
   ```
   git add crates/audio && git commit -m "audio: pw-metadata live-move argv + stream-id parser (pure, fixture-tested)"
   ```

---

## Task 4 — Per-channel output retarget in `ChannelManager` (THE BUG FIX) over `CommandRunner` (TDD)

**Files**
- modify `crates/audio/src/backend.rs` (add a `recreate` helper that forces a rebuild)
- modify `crates/audio/src/channels.rs` (add `set_output`)

**Interfaces**
- Adds to `AudioBackend<R>`: `fn recreate(&mut self, eq: &EqModel) -> Result<ConfHandle, AudioError>`
  - **forces** a rebuild: if the sink exists, remove it (destroy node + best-effort `pkill -f <conf>` + delete conf), then create it fresh with the current `spec` (which carries the new `playback_target`). This is the enforcement the old app lacked.
- Adds to `ChannelManager<R>`: `fn set_output(&mut self, channel_id: &str, output_device: Option<String>, eq: &EqModel) -> Result<ConfHandle, AudioError>`
  - mutates the stored `ChannelDef.output_device`, then calls `recreate` with the updated `SinkSpec` so the channel's `playback.props.target.object` is actually rewired.

**Design notes**
- Output change MUST regenerate the conf (the `target.object` line is produced by `render_filter_chain_conf` from `SinkSpec.playback_target`) and re-spawn the dedicated instance. Simply recording the device is the exact failure mode we are fixing (Global Constraints).
- `recreate` reuses `remove` + `create` (G1) — it does not re-implement teardown or spawn.

**Steps**

1. Add `recreate` to `crates/audio/src/backend.rs` inside `impl<R: CommandRunner> AudioBackend<R>` (after `create`):
   ```rust
   /// Force a rebuild so a changed `SinkSpec` (e.g. a new playback target) is
   /// actually applied: tear down any existing instance, then create fresh.
   /// This is the enforcement the old per-channel output selector lacked.
   pub fn recreate(&mut self, eq: &EqModel) -> Result<ConfHandle, AudioError> {
       self.remove()?;
       self.create(eq)
   }
   ```
   Add tests to `backend.rs` `#[cfg(test)] mod tests`:
   ```rust
   #[test]
   fn recreate_tears_down_then_creates_with_new_target() {
       // remove(): sink_exists ls (present) → find_node_id ls → destroy → pkill
       // create(): sink_exists ls (now absent) → spawn pipewire -c
       let runner = MockRunner::new()
           .with_output(0, LS_WITH_SINK, "")                       // remove: sink_exists
           .with_output(0, LS_WITH_SINK, "")                       // remove: find_node_id
           .with_output(0, "", "")                                  // remove: destroy
           .with_output(0, "", "")                                  // remove: pkill
           .with_output(0, "id 1\n    node.name = \"x\"\n", "")  // create: sink_exists (absent)
           .with_output(0, "", "");                                 // create: spawn
       let spec = SinkSpec {
           node_name: "arctis_eq".into(),
           description: "Arctis EQ Sink".into(),
           playback_target: Some("alsa_output.speakers".into()),
       };
       let mut be = AudioBackend::new(runner, spec);
       be.recreate(&EqModel::default_10band()).unwrap();
       let calls = &be.runner().calls;
       assert_eq!(calls[2], vec!["pw-cli", "destroy", "57"]);
       // last call spawns a fresh dedicated instance
       let last = calls.last().unwrap();
       assert_eq!(last[0], "pipewire");
       assert!(last[2].ends_with("arctis_eq.conf"));
   }
   ```

2. Add `set_output` to `crates/audio/src/channels.rs` inside `impl<R: CommandRunner> ChannelManager<R>`:
   ```rust
   /// Change a channel's output device and ENFORCE it: update the stored
   /// definition, then rebuild that channel's sink so its playback target is
   /// actually rewired (fixes the old dead selector). `output_device = None`
   /// returns the channel to following the default sink.
   pub fn set_output(
       &mut self,
       channel_id: &str,
       output_device: Option<String>,
       eq: &EqModel,
   ) -> Result<ConfHandle, AudioError> {
       let ch = self
           .config
           .channels
           .iter_mut()
           .find(|c| c.id == channel_id)
           .ok_or_else(|| AudioError::Invalid(format!("unknown channel: {channel_id}")))?;
       ch.output_device = output_device;
       let spec = ch.sink_spec();
       let mut be = AudioBackend::new(&mut self.runner, spec);
       be.recreate(eq)
   }
   ```
   Add tests to `channels.rs` `#[cfg(test)] mod tests`:
   ```rust
   #[test]
   fn set_output_updates_def_and_rebuilds_channel() {
       // remove() path (sink present) then create() path (absent), as in recreate.
       let present_media = "id 12\n    node.name = \"Arctis_Media\"\n";
       let runner = MockRunner::new()
           .with_output(0, present_media, "")  // remove: sink_exists (present)
           .with_output(0, present_media, "")  // remove: find_node_id
           .with_output(0, "", "")              // remove: destroy
           .with_output(0, "", "")              // remove: pkill
           .with_output(0, "id 1\n    node.name = \"x\"\n", "") // create: absent
           .with_output(0, "", "");             // create: spawn
       let mut mgr = ChannelManager::new(runner, cfg());
       mgr.set_output("media", Some("alsa_output.speakers".into()), &EqModel::default_10band())
           .unwrap();
       // The stored def now carries the new device (enforced, not just stored).
       assert_eq!(
           mgr.find("media").unwrap().output_device.as_deref(),
           Some("alsa_output.speakers")
       );
       // A fresh instance was spawned with the Media conf.
       let last = mgr.runner().calls.last().unwrap();
       assert_eq!(last[0], "pipewire");
       assert!(last[2].ends_with("Arctis_Media.conf"));
   }

   #[test]
   fn set_output_unknown_channel_errors() {
       let runner = MockRunner::new();
       let mut mgr = ChannelManager::new(runner, cfg());
       let err = mgr
           .set_output("nope", None, &EqModel::default_10band())
           .unwrap_err();
       assert!(matches!(err, AudioError::Invalid(_)));
   }
   ```

3. Run and commit:
   ```
   ~/.cargo/bin/cargo test -p arctis-audio
   ```
   Expected: new `backend::tests::recreate_*` + `channels::tests::set_output_*` pass; suite green. Then:
   ```
   git add crates/audio && git commit -m "audio: enforced per-channel output retarget (recreate + set_output) — fixes dead selector"
   ```

---

## Task 5 — `Router`: live `pw-metadata` move + persistent fragment write over `CommandRunner` (TDD)

**Files**
- modify `crates/audio/src/routing.rs` (add the `Router` orchestrator + tests)
- modify `crates/audio/src/lib.rs` (re-export `Router`)

**Interfaces**
- Produces `struct Router<R: CommandRunner> { runner: R, rules: Vec<RouteRule> }` with:
  - `fn new(runner: R) -> Self` (empty rule set)
  - `fn with_rules(runner: R, rules: Vec<RouteRule>) -> Self`
  - `fn apply_live(&mut self, app: &AppMatch, target_sink: &str) -> Result<String, AudioError>`
    — `pw-dump` → `parse_stream_id` → `pw-metadata` move; returns the moved stream id.
  - `fn set_rule(&mut self, rule: RouteRule)` — upsert a persistent rule by `app_binary` (replaces an existing rule for the same binary; the upsert respects "don't duplicate").
  - `fn write_persistent(&mut self) -> Result<PathBuf, AudioError>` — render `node_rules_fragment` and write it to `wireplumber_fragment_path()`, creating parent dirs.
  - `fn list(&self) -> &[RouteRule]`

**Design notes**
- `apply_live` runs `pw-dump` (no args) through the runner, parses the stream id by `AppMatch`, then runs `pw-metadata` with `move_stream_argv`. Both are asserted as exact argv under `MockRunner`.
- `write_persistent` is the **persistence** half; combined with `apply_live` it satisfies "live + persistent" (Global Constraints). It only writes the file — activating it is the daemon reading `wireplumber.conf.d/` (verified OWNER-RUN). **Restore-stream management** is handled in the OWNER-RUN task (record the setting) and noted here; the fragment itself is the durable record.
- `set_rule` upsert prevents duplicate rules for the same binary (idempotent persistence).

**Steps**

1. Append the `Router` to `crates/audio/src/routing.rs` (after the parser code, before `#[cfg(test)]`):
   ```rust
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
           Self { runner, rules: Vec::new() }
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

       fn check(out: crate::runner::CmdOutput, program: &str) -> Result<crate::runner::CmdOutput, AudioError> {
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
           if let Some(existing) = self.rules.iter_mut().find(|r| r.app_binary == rule.app_binary) {
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
   ```

2. Add tests to `routing.rs` `#[cfg(test)] mod tests`:
   ```rust
   use crate::runner::MockRunner;

   #[test]
   fn apply_live_dumps_parses_then_moves_with_exact_argv() {
       let dump = include_str!("../tests/fixtures/pw_dump_streams.json");
       let runner = MockRunner::new()
           .with_output(0, dump, "")  // pw-dump
           .with_output(0, "", "");    // pw-metadata move
       let mut router = Router::new(runner);
       let id = router
           .apply_live(&AppMatch::Binary("firefox".into()), "Arctis_Media")
           .unwrap();
       assert_eq!(id, "73");
       let calls = &router.runner().calls;
       assert_eq!(calls[0], vec!["pw-dump"]);
       assert_eq!(
           calls[1],
           vec!["pw-metadata", "-n", "default", "73", "target.object", "Arctis_Media"]
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
   ```
   > Note: `write_persistent_writes_fragment_to_temp_home` mutates the `HOME` env var. Keep it the only test that does so (env is process-global). It restores nothing but writes only under a unique temp dir and cleans up.

3. Extend the `lib.rs` routing re-export to include `Router`:
   ```rust
   pub use routing::{
       clear_stream_target_argv, move_stream_argv, node_rules_fragment, parse_stream_id,
       wireplumber_fragment_path, AppMatch, RouteRule, Router,
   };
   ```

4. Run and commit:
   ```
   ~/.cargo/bin/cargo test -p arctis-audio
   ```
   Expected: new `routing::tests` (4 added) pass; full crate suite green; no daemon touched. Then:
   ```
   git add crates/audio && git commit -m "audio: Router — live pw-metadata move + persistent WirePlumber fragment (G3)"
   ```

---

## Task 6 — `asm-cli` subcommands + OWNER-RUN hardware E2E

**Files**
- modify `crates/cli/src/main.rs` (add `channels`, `route`, and `channel output` subcommand trees)
- (no Cargo change — `arctis-audio` is already a dependency of `crates/cli`)

**Interfaces**
- Consumes `arctis_audio::{ChannelManager, ChannelSetConfig, Router, RouteRule, AppMatch, EqModel, RealRunner}`.
- Produces CLI:
  - `asm-cli channels up [--target <hw_sink>]` — create Game/Chat/Media (each pinned to `--target` if given).
  - `asm-cli channels down` — remove all configured channels.
  - `asm-cli route set <app> <channel> [--by-name]` — live move + persistent rule (`<app>` = binary by default, or `application.name` with `--by-name`; `<channel>` = a channel id `game|chat|media`, resolved to its sink `node.name`).
  - `asm-cli route list` — print persistent rules.
  - `asm-cli channel output set <channel> <device>` — enforced retarget (`<device>` = hardware sink `node.name`; pass `default` to clear the pin).

**Design notes**
- `route set` resolves `<channel>` to a sink `node.name` via `ChannelSetConfig::default_sonar(None).find(<channel>)`. It performs `Router::apply_live` (instant) then `Router::set_rule` + `write_persistent` (durable). The mapping channel-id → sink-name is the single source already in `channels.rs` (G1).
- `route list` reads back the persistent fragment is out of scope; for v1 `route list` re-reads nothing and instead prints the in-memory rules after re-deriving them is not possible across processes — so `route list` prints the **current fragment file contents** if present (simple, honest), else "no persistent routes". (Reading the fragment back into typed rules is a LATER parser; not needed now.)

**Steps**

1. Extend `crates/cli/src/main.rs`. Add imports near the existing `use arctis_audio::…` line:
   ```rust
   use arctis_audio::{
       AppMatch, ChannelManager, ChannelSetConfig, RouteRule, Router,
   };
   ```
   Add variants to `enum Command` (after `Eq`):
   ```rust
       /// Manage the full set of submix channels (Game / Chat / Media).
       Channels {
           #[command(subcommand)]
           action: ChannelsAction,
       },
       /// Per-application routing (live + persistent).
       Route {
           #[command(subcommand)]
           action: RouteAction,
       },
       /// Per-channel output device control.
       Channel {
           #[command(subcommand)]
           action: ChannelCmd,
       },
   ```
   Add these enums after the existing `EqAction` enum:
   ```rust
   #[derive(Subcommand, Debug)]
   enum ChannelsAction {
       /// Create all configured channels (idempotent).
       Up {
           /// Hardware sink node.name every channel feeds; omit to follow default.
           #[arg(long)]
           target: Option<String>,
       },
       /// Remove all configured channels (idempotent).
       Down,
   }

   #[derive(Subcommand, Debug)]
   enum RouteAction {
       /// Route an app to a channel: live move + persistent WirePlumber rule.
       Set {
           /// Application matcher (application.process.binary by default).
           app: String,
           /// Channel id: game | chat | media.
           channel: String,
           /// Match application.name instead of process.binary.
           #[arg(long)]
           by_name: bool,
       },
       /// Print the persistent routing fragment.
       List,
   }

   #[derive(Subcommand, Debug)]
   enum ChannelCmd {
       /// Set a channel's output device (enforced rebuild).
       Output {
           #[command(subcommand)]
           action: ChannelOutputAction,
       },
   }

   #[derive(Subcommand, Debug)]
   enum ChannelOutputAction {
       /// Retarget a channel to a hardware sink (`default` clears the pin).
       Set {
           /// Channel id: game | chat | media.
           channel: String,
           /// Hardware sink node.name, or `default` to follow the default sink.
           device: String,
       },
   }
   ```
   Add match arms inside `match cli.command { … }` (after the `Eq` arm):
   ```rust
       Command::Channels { action } => {
           let target = match &action {
               ChannelsAction::Up { target } => target.clone(),
               ChannelsAction::Down => None,
           };
           let cfg = ChannelSetConfig::default_sonar(target.as_deref());
           let mut mgr = ChannelManager::new(RealRunner, cfg);
           match action {
               ChannelsAction::Up { .. } => match mgr.up(&EqModel::default_10band()) {
                   Ok(handles) => {
                       println!("channels up: {} sinks ready", handles.len());
                       ExitCode::SUCCESS
                   }
                   Err(e) => {
                       eprintln!("error bringing channels up: {e}");
                       ExitCode::FAILURE
                   }
               },
               ChannelsAction::Down => match mgr.down() {
                   Ok(()) => {
                       println!("channels down");
                       ExitCode::SUCCESS
                   }
                   Err(e) => {
                       eprintln!("error bringing channels down: {e}");
                       ExitCode::FAILURE
                   }
               },
           }
       }
       Command::Route { action } => match action {
           RouteAction::Set { app, channel, by_name } => {
               let cfg = ChannelSetConfig::default_sonar(None);
               let sink = match cfg.find(&channel) {
                   Some(c) => c.node_name.clone(),
                   None => {
                       eprintln!("error: unknown channel '{channel}' (use game|chat|media)");
                       return ExitCode::FAILURE;
                   }
               };
               let matcher = if by_name {
                   AppMatch::Name(app.clone())
               } else {
                   AppMatch::Binary(app.clone())
               };
               let mut router = Router::new(RealRunner);
               // Live move first (instant), then persist.
               match router.apply_live(&matcher, &sink) {
                   Ok(id) => println!("live: moved stream {id} ({app}) → {sink}"),
                   Err(e) => {
                       eprintln!("warning: live move failed (is the app playing?): {e}");
                       // Still persist the rule so it applies next launch.
                   }
               }
               router.set_rule(RouteRule::new(&app, &sink));
               match router.write_persistent() {
                   Ok(path) => {
                       println!("persistent: rule written to {}", path.display());
                       println!("note: run `systemctl --user restart wireplumber` to load it now");
                       ExitCode::SUCCESS
                   }
                   Err(e) => {
                       eprintln!("error writing persistent rule: {e}");
                       ExitCode::FAILURE
                   }
               }
           }
           RouteAction::List => {
               let path = arctis_audio::wireplumber_fragment_path();
               match std::fs::read_to_string(&path) {
                   Ok(body) => {
                       println!("# {}", path.display());
                       print!("{body}");
                       ExitCode::SUCCESS
                   }
                   Err(_) => {
                       println!("no persistent routes ({} absent)", path.display());
                       ExitCode::SUCCESS
                   }
               }
           }
       },
       Command::Channel { action } => match action {
           ChannelCmd::Output { action } => match action {
               ChannelOutputAction::Set { channel, device } => {
                   let cfg = ChannelSetConfig::default_sonar(None);
                   let mut mgr = ChannelManager::new(RealRunner, cfg);
                   let dev = if device == "default" { None } else { Some(device.clone()) };
                   match mgr.set_output(&channel, dev, &EqModel::default_10band()) {
                       Ok(h) => {
                           println!(
                               "channel '{channel}' output set to {device} (conf {})",
                               h.conf_path.display()
                           );
                           ExitCode::SUCCESS
                       }
                       Err(e) => {
                           eprintln!("error setting channel output: {e}");
                           ExitCode::FAILURE
                       }
                   }
               }
           },
       },
   ```

2. Add CLI parse-only tests to `crates/cli/src/main.rs` `#[cfg(test)] mod tests` (these do NOT touch PipeWire):
   ```rust
   #[test]
   fn channels_up_with_target() {
       let cmd = parse(&["channels", "up", "--target", "alsa_output.arctis"])
           .expect("channels up --target should parse");
       match cmd {
           super::Command::Channels {
               action: super::ChannelsAction::Up { target: Some(t) },
           } => assert_eq!(t, "alsa_output.arctis"),
           other => panic!("unexpected: {other:?}"),
       }
   }

   #[test]
   fn channels_down() {
       let cmd = parse(&["channels", "down"]).expect("channels down should parse");
       assert!(matches!(
           cmd,
           super::Command::Channels { action: super::ChannelsAction::Down }
       ));
   }

   #[test]
   fn route_set_binary_default() {
       let cmd = parse(&["route", "set", "firefox", "media"]).expect("route set should parse");
       match cmd {
           super::Command::Route {
               action: super::RouteAction::Set { app, channel, by_name },
           } => {
               assert_eq!(app, "firefox");
               assert_eq!(channel, "media");
               assert!(!by_name);
           }
           other => panic!("unexpected: {other:?}"),
       }
   }

   #[test]
   fn route_set_by_name() {
       let cmd = parse(&["route", "set", "Firefox", "media", "--by-name"])
           .expect("route set --by-name should parse");
       match cmd {
           super::Command::Route {
               action: super::RouteAction::Set { by_name, .. },
           } => assert!(by_name),
           other => panic!("unexpected: {other:?}"),
       }
   }

   #[test]
   fn route_list() {
       let cmd = parse(&["route", "list"]).expect("route list should parse");
       assert!(matches!(
           cmd,
           super::Command::Route { action: super::RouteAction::List }
       ));
   }

   #[test]
   fn channel_output_set() {
       let cmd = parse(&["channel", "output", "set", "media", "alsa_output.speakers"])
           .expect("channel output set should parse");
       match cmd {
           super::Command::Channel {
               action: super::ChannelCmd::Output {
                   action: super::ChannelOutputAction::Set { channel, device },
               },
           } => {
               assert_eq!(channel, "media");
               assert_eq!(device, "alsa_output.speakers");
           }
           other => panic!("unexpected: {other:?}"),
       }
   }
   ```

3. Build and run the full default gate (compile + unit tests; no daemon):
   ```
   ~/.cargo/bin/cargo build --workspace
   ~/.cargo/bin/cargo test --workspace
   ```
   Expected: clean build; green suite; **no test spawns PipeWire or WirePlumber** (all audio/routing tests use `MockRunner`/fixtures; the one HOME-mutating test writes only to a temp dir). Commit:
   ```
   git add crates/cli && git commit -m "cli: channels up/down, route set/list, channel output set over arctis-audio"
   ```

4. **OWNER-RUN (manual, on real PipeWire 1.4 / WirePlumber 0.5 — out of CI, G8 / spec §14).** Validate the full vertical and pin the items not confirmed by docs. (A headset is ideal for the per-channel-output step but any two output devices work; a browser is the routed app.)

   **A. Discover the hardware sink name.**
   ```
   wpctl status            # note the Arctis (or any) sink
   pw-cli ls Node | grep -i 'node.name'   # copy the exact hardware sink node.name
   ```

   **B. Bring channels up and verify three sinks + mixing (Research basis d).**
   ```
   ~/.cargo/bin/cargo run -p arctis-cli -- channels up --target <hardware_sink_node.name>
   wpctl status            # expect Arctis_Game / Arctis_Chat / Arctis_Media as output sinks
   pw-cli ls Node | grep -E 'Arctis_(Game|Chat|Media)'
   ```
   Play audio into two of them at once (e.g. set a browser to `Arctis_Media` and a media player to `Arctis_Game` in `pavucontrol`) and **confirm both are audible at the hardware sink simultaneously** (this validates that multiple filter-chain sinks mix at one hardware sink — if not, the LATER Master-node plan is needed; record the result). Confirm **48 kHz** end-to-end: `pw-dump <hardware_sink_id> | grep -i rate` shows 48000 and no resampler is inserted.

   **C. Per-app LIVE routing — capture a real `pw-dump`, pin the parser/value form.**
   ```
   # Start playing audio in a browser, then:
   pw-dump > /tmp/asm_pwdump.json
   grep -n 'application.process.binary' /tmp/asm_pwdump.json   # find your browser's binary + nearby "id"
   ```
   - **If the JSON field layout differs from `crates/audio/tests/fixtures/pw_dump_streams.json`**, update that fixture to match a real object and re-run `~/.cargo/bin/cargo test -p arctis-audio parse_stream_id` until green (correct the parser if needed — it is one function).
   - Now route live:
   ```
   ~/.cargo/bin/cargo run -p arctis-cli -- route set <browser_binary> media
   pw-dump <browser_stream_id> | grep -i target        # expect target.object = Arctis_Media
   wpctl status                                          # browser stream now under Arctis_Media
   ```
   Listen: the browser should now play through Media. **If the move does not take with the sink `node.name`, edit `move_stream_argv` to use the sink's `object.serial` instead (one-line change), re-run its unit test, and re-validate.** Record which form worked.

   **D. Persistent rule + restore-stream (pin item c).**
   ```
   cat ~/.config/wireplumber/wireplumber.conf.d/90-asm-routing.conf   # written by route set
   systemctl --user restart wireplumber
   # Close and reopen the browser; confirm it auto-routes to Arctis_Media WITHOUT a live move.
   ```
   - If `restore-stream` overrides the rule (the app snaps back to a remembered target), consult the well-known settings (`restore.stream`, the `default` metadata) and add the minimal setting that lets the `node.rules` target win. **Record the exact setting key/value you applied** in the PR description (this is the doc-unconfirmed item c). Verify a manually-pinned stream (one you set by hand in `pavucontrol`) is **not** overridden by our rule.

   **E. Per-channel OUTPUT retarget — THE BUG FIX.**
   ```
   ~/.cargo/bin/cargo run -p arctis-cli -- channel output set media <other_device_node.name>
   pw-dump | grep -A3 'Arctis_Media.output'    # expect target.object = <other_device>
   ```
   With the browser still routed to Media, audio should now come out of the **other device** (e.g. speakers) while Game/Chat stay on the headset. Then clear:
   ```
   ~/.cargo/bin/cargo run -p arctis-cli -- channel output set media default
   ```
   Confirm Media returns to the default/hardware sink. This is the headline §5 enforcement (old selector did nothing; ours rebuilds the channel).

   **F. Tear down.**
   ```
   ~/.cargo/bin/cargo run -p arctis-cli -- channels down
   pw-cli ls Node | grep -E 'Arctis_(Game|Chat|Media)'   # expect nothing
   rm -f ~/.config/wireplumber/wireplumber.conf.d/90-asm-routing.conf   # if reverting persistent routing
   systemctl --user restart wireplumber
   # Stop any leftover dedicated `pipewire -c` instances (v1 best-effort pkill; manual confirm):
   pgrep -af 'pipewire -c .*Arctis_'
   ```
   Record in the PR description: (1) mixing confirmed (B), (2) `pw-dump` field layout / fixture correction (C), (3) `node.name` vs `object.serial` for the live move (C), (4) the restore-stream setting applied (D), (5) per-channel output retarget confirmed audible (E).

---

## Self-Review

**Spec coverage**
- §5 multiple named submixes (Game/Chat/Media, Sonar-mirroring names) — Task 1 `ChannelSetConfig::default_sonar` + `ChannelManager::{up,down}` reusing `AudioBackend` (G1).
- §5/§6 **per-channel output override, enforced** (the bug fix) — Task 4 `AudioBackend::recreate` + `ChannelManager::set_output` regenerate the conf with the new `playback.props.target.object` and re-spawn; OWNER-RUN E (audible retarget) validates. Constraint "enforced, not stored" met.
- §6 per-app routing **live** (`pw-metadata <id> target.object <sink>`) + **persistent** (WirePlumber 0.5 `node.rules` SPA-JSON in `wireplumber.conf.d/`) — Task 2 (fragment generator), Task 3 (move argv + stream-id parser), Task 5 (`Router::{apply_live,set_rule,write_persistent}`).
- §6 respect manual pins / manage restore-stream — persistent fragment is the durable record; restore-stream setting pinned in OWNER-RUN D (manual-pin non-override verified there).
- §6 mixing at hardware sink without an explicit Master — justified in Research basis (d) with citations; verified OWNER-RUN B; Master node explicitly deferred.
- Decision on Master node (requested in scope item 2): **not needed for this increment** — each channel's `playback.props.target.object = <hardware sink>` lets PipeWire's adapter mixer sum them; a Master is only worthwhile for a single post-mix stage (deferred), with rationale + citations recorded.
- §3 48 kHz — inherited unchanged from `render_filter_chain_conf`; OWNER-RUN B confirms no resampler.
- §14 / G8 testing split — every generator/orchestrator unit-tested with `MockRunner`/fixtures; daemon path is OWNER-RUN, out of CI (default gate `cargo test --workspace` touches no daemon).
- Guardrails: G1 (one `AudioBackend` driven N times via the `&mut R` runner seam; channel-id→sink-name mapping single-sourced; argv/SPA-JSON are single generators), G3 (live + idempotent + 48 kHz; enforced retarget), G6 (no `tauri`; new code split across `channels.rs` + `routing.rs`, files stay focused), G7 (all paths return `AudioError`; no `unwrap`/`expect` on runtime paths).

**Placeholder scan** — no `TBD`, no "similar to above", no `todo!()`/`unimplemented!()`. Every code/fixture/command step is complete and compilable. The genuinely doc-unconfirmed details are isolated and **verified-then-pinned** in OWNER-RUN Task 6 (per the founded-in-research rule), each to a single edit point: `pw-dump` field layout → `pw_dump_streams.json` + `parse_stream_id`; live-move value form (`node.name` vs `object.serial`) → `move_stream_argv`; restore-stream override → a recorded WirePlumber setting; multi-sink mixing / Master need → OWNER-RUN B result.

**Type consistency** — `AudioError` remains the single crate error type; `CommandRunner`/`CmdOutput`/`MockRunner` flow unchanged, now also borrowed via `&mut R` (one new blanket impl, Task 1). `ChannelDef.sink_spec()` produces the *existing* `SinkSpec`, so all foundation generators apply verbatim. `RouteRule`/`AppMatch`/`Router` are self-contained in `routing.rs`. Channel-id → sink `node.name` mapping has one source (`ChannelSetConfig`), shared by `ChannelManager` and the CLI `route`/`channel` arms. The `target.object` value (sink `node.name`) is produced in exactly one place (`move_stream_argv`) and one fixture key (`node_rules_fragment`).

**CI note** — the default gate is `~/.cargo/bin/cargo test --workspace` and never starts PipeWire or WirePlumber. Hardware/PipeWire E2E (Task 6 step 4, A–F) is **owner-run, manual, and out of CI** (G8, spec §14).

## Open questions (carry into execution / later plans)
1. **`pw-dump` field layout** — pinned against a real capture in OWNER-RUN 6C; the fixture is the contract for the parser.
2. **Live-move value form** — `node.name` (default) vs `object.serial`; confirmed in 6C, isolated to `move_stream_argv`.
3. **restore-stream override** — the exact WirePlumber 0.5 setting that lets a `node.rules` target win over a remembered target; pinned in 6D and recorded.
4. **Master node** — deferred; revisit when a single post-mix EQ/limiter or a one-shot "retarget all channels" is wanted (LATER plan), or if OWNER-RUN B shows mixing problems.
5. **`route list` round-trip** — v1 prints the fragment file verbatim; a typed SPA-JSON reader (fragment → `Vec<RouteRule>`) is a LATER refinement, naturally owned by the config/engine plan.
6. **Dedicated-instance lifecycle** — N channels mean N `pipewire -c` children; clean ownership/teardown beyond best-effort `pkill -f <conf>` is carried forward to the engine plan (G10).
