> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to execute this plan task-by-task. Each task below is independently testable; dispatch one subagent per task, in order, and review at each checkpoint. Tasks marked **OWNER-RUN** are manual hardware/PipeWire validation steps the repo owner runs on their real machine — they are NOT part of CI and NOT to be auto-executed by a subagent.

# Audio Engine Foundation — PipeWire Virtual EQ Sink (thin vertical)

**Date:** 2026-06-20
**Spec:** `docs/superpowers/specs/2026-06-20-arctis-sound-manager-design.md` (esp. §6 Audio engine, §14 Testing)
**Guardrails:** `ARCHITECTURE.md` G1, G3, G6, G7, G8

## Goal

Stand up a new `arctis-audio` crate that can (a) create and remove a **PipeWire virtual EQ sink** — a `libpipewire-module-filter-chain` `Audio/Sink` whose graph is a chain of named parametric biquad bands feeding the current hardware output — and (b) apply **live per-band parametric EQ changes** (Freq/Q/Gain) to that sink **without restarting the audio stack**. Wire it into `asm-cli` with `sink create|remove` and `eq set|show` subcommands so the owner can end-to-end test on their real machine **with no headset attached** (any default output works). The pure logic (config/argv/Props generation) is fully unit-tested without a live PipeWire daemon; the daemon-touching path is exercised only by owner-run hardware tasks.

This is a deliberately **thin, E2E-testable vertical slice** of spec §6, not the whole audio engine.

## Architecture

- New crate `crates/audio` (package `arctis-audio`, lib name `arctis_audio`), depending only on `arctis-domain` (crate dependency rule, ARCHITECTURE §2). It does **not** depend on `tauri`, `engine`, `device`, or `config`.
- **Subprocess approach for v1.** All PipeWire interaction goes through external commands (`pipewire -c <conf>` for the dedicated filter-chain instance, `pw-cli` for live Props updates and listing, `pw-dump` for verification). Native `pipewire-rs` monitoring is explicitly a LATER plan (it is `!Send`, callback-heavy, and needs a dedicated loop thread — out of scope here).
- All subprocess invocation sits behind a small **`CommandRunner`** trait (run argv → captured output), mirroring the device crate's `Transport`/`MockTransport` shape (G1). A real impl wraps `std::process::Command`; a `MockRunner` records argv and returns canned output for tests.
- The TDD core is a set of **pure generators** inside an `AudioBackend`: given an EQ model, produce (1) the filter-chain config string, (2) the exact argv for create/remove, (3) the exact `pw-cli` Props payload for a live band update. These are unit-tested against fixtures with **no daemon**. The backend then executes via `CommandRunner`; nothing in the default test gate spawns PipeWire.
- One biquad-band builder and one config generator — no duplication (G1).

Data/lifecycle, end to end:

```
EqModel (domain types: bands, defaults, validation)
   │  pure generators (AudioBackend, no I/O)
   ├─ render_filter_chain_conf()  ──► conf string written to a temp file
   ├─ create_argv() / remove_argv() ─► argv for the dedicated `pipewire -c` instance + teardown
   └─ set_band_props_argv(band, f, q, g) ─► argv for `pw-cli s <node-id> Props {…}`
                                            (live, in-place; NO conf rewrite, NO restart — G3)
        │
        ▼  CommandRunner (real: std::process::Command  |  test: MockRunner)
   actual PipeWire daemon  (touched ONLY by OWNER-RUN tasks, out of CI — §14 / G8)
```

## Tech Stack

- Rust 2021, workspace edition/lints already configured in root `Cargo.toml`.
- Deps via `[workspace.dependencies]` only: `arctis-domain`, `thiserror`, `serde`/`serde_json` (already present). New deps: none required beyond these.
- `cargo` lives at `~/.cargo/bin/cargo` and is **not on PATH by default** — every command in this plan invokes it as `~/.cargo/bin/cargo`.
- Target runtime: PipeWire **1.4.x** / WirePlumber **0.5.x**, **48000 Hz only**, on the owner's Nobara machine. (System validated in spec §3.)

## Research basis (founded-in-research; cite, don't guess)

Confirmed against current PipeWire docs (June 2026):

- **Builtin biquad nodes** in a filter-chain graph use `type = builtin` with `label = bq_peaking` / `bq_lowshelf` / `bq_highshelf`, and **every biquad exposes named controls `"Freq"`, `"Q"`, `"Gain"`** set in a `control = { … }` block. — https://docs.pipewire.org/page_module_filter_chain.html
- **Virtual sink** is produced by `capture.props = { media.class = Audio/Sink }`; the chain's output is wired to hardware via `playback.props` with `node.passive = true`. The module is `libpipewire-module-filter-chain`. — https://docs.pipewire.org/page_module_filter_chain.html
- **`pw-cli` set-param** form is `set-param <object-id> <param-id> <param-json>` (alias `s <id> Props <json>`); param-id may be the short name (`Props`); `list-objects` / `ls Node` enumerates node ids; `enum-params <id> Props` / alias `e` reads current params. — https://docs.pipewire.org/page_man_pw-cli_1.html
- General module config and `/usr/share/pipewire/filter-chain/` examples confirmed via ArchWiki. — https://wiki.archlinux.org/title/PipeWire
- Reference project on **this exact machine** already proved a dedicated `pipewire -c filter-chain.conf` instance hosting `libpipewire-module-filter-chain` works (spec §3, §10).

**NOT fully nailed by official docs — verify on-machine, then pin (do not assert a guess):**
- The exact JSON shape of the **live Props payload** that names a *specific band's* control. The expected/widely-used form is the controls being addressed as `"<node.name>:Freq"` / `"<node.name>:Q"` / `"<node.name>:Gain"` inside a `params` array, e.g. `pw-cli s <node-id> Props '{ params = [ "eq_band_3:Freq" 1200.0 "eq_band_3:Q" 1.0 "eq_band_3:Gain" -4.5 ] }'`. **Task 4 codes the generator to this form behind a single constant, and Task 6 (OWNER-RUN) verifies the literal string against the live daemon via `enum-params … Props` / `pw-dump` and corrects the constant if the daemon reports a different key convention.** The generator design isolates this to one place so a correction is a one-line fix re-validated by the Task-4 unit test fixture.
- Whether each band's `control` keys must be globally unique (per-band `name = eq_band_N`) for the `name:Control` addressing to resolve. We assume **yes** (give every band node a unique `name`), and Task 6 confirms.

> **Live-EQ persistence note (record in code comment + spec alignment):** runtime `Props` set via `pw-cli` are **not persisted** by PipeWire across daemon/session restarts. The future engine must **re-apply all band values on startup** after (re)creating the sink. This plan only creates the sink and applies live edits; re-apply-on-startup orchestration is a LATER plan (engine crate).

## Non-goals (explicitly DEFERRED to later plans)

- Multi-channel submixes (Game / Chat / Media) and the Master mix graph.
- Per-app routing (`node.rules`, `pw-metadata target.object`, `restore-stream`).
- HRIR / convolver virtual surround.
- The microphone chain.
- The engine orchestrator, reconciler, event stream, and re-apply-on-startup.
- Config / profiles / persistence.
- Any UI (`src-tauri`, `ui/`).
- Native `pipewire-rs` monitoring / live node-id discovery via the API (subprocess only here).
- Per-band filter-type selection beyond peaking/lowshelf/highshelf, and the 3-band simplified mode.

## Global Constraints

- **Sample rate:** 48000 Hz only, end-to-end; never emit a resample or a non-48k rate in any conf/argv (G3, spec §3).
- **Live EQ via Props, no restart:** band edits MUST be applied with `pw-cli s <id> Props …` in place; never rewrite the `.conf` or restart the daemon to change a parameter (G3, headline requirement).
- **Subprocess approach:** all PipeWire interaction is external-command driven through `CommandRunner`; no `pipewire-rs` linkage in this plan.
- **Crate naming:** package `arctis-audio`, library `arctis_audio`, directory `crates/audio`.
- **Deps via workspace:** every dependency is referenced as `{ workspace = true }`; declarations live in root `[workspace.dependencies]`.
- **Typed errors:** all fallible paths return `thiserror`-derived `AudioError`; **no `unwrap()`/`expect()`** on runtime/fallible paths (G7).
- **Reuse, not duplicate:** exactly one biquad-band builder and one config generator; `CommandRunner`/`MockRunner` mirror `Transport`/`MockTransport` (G1).
- **Idempotent create/remove:** stable `node.name`; reconcile against an existing sink instead of blindly recreating (G3).
- **Testability:** pure generators are unit-tested with no daemon; the daemon path is OWNER-RUN and out of the CI gate (G8, spec §14).

---

## Task 1 — Crate scaffold + workspace wiring + `CommandRunner`/`MockRunner`

**Files**
- create `crates/audio/Cargo.toml`
- create `crates/audio/src/lib.rs`
- create `crates/audio/src/error.rs`
- create `crates/audio/src/runner.rs`
- modify `Cargo.toml` (root: add member + workspace dependency)

**Interfaces**
- Produces `trait CommandRunner { fn run(&mut self, program: &str, args: &[&str]) -> Result<CmdOutput, AudioError>; }`
- Produces `struct CmdOutput { pub status: i32, pub stdout: String, pub stderr: String }`
- Produces `struct RealRunner;` (impl over `std::process::Command`)
- Produces `struct MockRunner { pub calls: Vec<Vec<String>>, … }` with `with_output(status, stdout, stderr)` queueing, mirroring `MockTransport`
- Produces `enum AudioError` (thiserror)

**Steps**

1. Modify the root `Cargo.toml` to register the crate and its workspace dependency. Apply both edits:

   In `[workspace] members`, change:
   ```toml
   members = ["crates/domain", "crates/device", "crates/cli"]
   ```
   to:
   ```toml
   members = ["crates/domain", "crates/device", "crates/audio", "crates/cli"]
   ```
   In `[workspace.dependencies]`, add this line directly after the `arctis-device` line:
   ```toml
   arctis-audio = { path = "crates/audio" }
   ```

2. Create `crates/audio/Cargo.toml`:
   ```toml
   [package]
   name = "arctis-audio"
   version = "0.1.0"
   edition.workspace = true
   license.workspace = true

   [dependencies]
   arctis-domain = { workspace = true }
   thiserror = { workspace = true }
   ```

3. Write a failing test for `AudioError`'s `Display`. Create `crates/audio/src/error.rs`:
   ```rust
   use thiserror::Error;

   /// Errors from the audio backend. All fallible paths return this; no
   /// `unwrap`/`expect` on runtime paths (ARCHITECTURE G7).
   #[derive(Debug, Error)]
   pub enum AudioError {
       /// A subprocess could not be spawned or its I/O failed.
       #[error("command `{program}` failed to run: {source_msg}")]
       Spawn { program: String, source_msg: String },
       /// A subprocess ran but exited non-zero.
       #[error("command `{program}` exited with status {status}: {stderr}")]
       NonZeroExit {
           program: String,
           status: i32,
           stderr: String,
       },
       /// A generated argument or config was invalid (programmer/data error).
       #[error("invalid audio request: {0}")]
       Invalid(String),
       /// Expected output (e.g. a node id) could not be parsed.
       #[error("could not parse `{what}` from output: {detail}")]
       Parse { what: String, detail: String },
   }

   #[cfg(test)]
   mod tests {
       use super::*;

       #[test]
       fn nonzero_exit_displays_program_and_stderr() {
           let e = AudioError::NonZeroExit {
               program: "pw-cli".into(),
               status: 1,
               stderr: "boom".into(),
           };
           assert_eq!(
               e.to_string(),
               "command `pw-cli` exited with status 1: boom"
           );
       }
   }
   ```

4. Create `crates/audio/src/runner.rs` with the trait, output type, real impl, and mock:
   ```rust
   use crate::error::AudioError;
   use std::process::Command;

   /// Captured result of a subprocess invocation.
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub struct CmdOutput {
       pub status: i32,
       pub stdout: String,
       pub stderr: String,
   }

   /// Runs an argv and captures its output. Mirrors the device crate's
   /// `Transport` seam so the backend is testable with no live daemon (G1, G8).
   pub trait CommandRunner {
       fn run(&mut self, program: &str, args: &[&str]) -> Result<CmdOutput, AudioError>;
   }

   /// Real runner over `std::process::Command`.
   #[derive(Default)]
   pub struct RealRunner;

   impl CommandRunner for RealRunner {
       fn run(&mut self, program: &str, args: &[&str]) -> Result<CmdOutput, AudioError> {
           let out = Command::new(program)
               .args(args)
               .output()
               .map_err(|e| AudioError::Spawn {
                   program: program.to_string(),
                   source_msg: e.to_string(),
               })?;
           Ok(CmdOutput {
               status: out.status.code().unwrap_or(-1),
               stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
               stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
           })
       }
   }

   /// In-memory runner for tests: records every argv, replays queued outputs.
   /// Mirrors `MockTransport` (G1).
   #[derive(Default)]
   pub struct MockRunner {
       /// Each recorded call is `[program, arg0, arg1, …]`.
       pub calls: Vec<Vec<String>>,
       queued: std::collections::VecDeque<CmdOutput>,
   }

   impl MockRunner {
       pub fn new() -> Self {
           Self::default()
       }

       /// Queue an output for the next `run`.
       pub fn with_output(mut self, status: i32, stdout: &str, stderr: &str) -> Self {
           self.queued.push_back(CmdOutput {
               status,
               stdout: stdout.to_string(),
               stderr: stderr.to_string(),
           });
           self
       }

       /// The most recent recorded call, as `program` plus its args.
       pub fn last_call(&self) -> Option<&Vec<String>> {
           self.calls.last()
       }
   }

   impl CommandRunner for MockRunner {
       fn run(&mut self, program: &str, args: &[&str]) -> Result<CmdOutput, AudioError> {
           let mut call = Vec::with_capacity(args.len() + 1);
           call.push(program.to_string());
           call.extend(args.iter().map(|a| a.to_string()));
           self.calls.push(call);
           Ok(self.queued.pop_front().unwrap_or(CmdOutput {
               status: 0,
               stdout: String::new(),
               stderr: String::new(),
           }))
       }
   }

   #[cfg(test)]
   mod tests {
       use super::*;

       #[test]
       fn mock_records_argv_and_replays_output() {
           let mut r = MockRunner::new().with_output(0, "node-id 42", "");
           let out = r.run("pw-cli", &["ls", "Node"]).expect("mock never errors");
           assert_eq!(out.stdout, "node-id 42");
           assert_eq!(r.last_call().unwrap(), &vec!["pw-cli", "ls", "Node"]);
       }

       #[test]
       fn mock_defaults_to_success_when_queue_empty() {
           let mut r = MockRunner::new();
           let out = r.run("true", &[]).expect("mock never errors");
           assert_eq!(out.status, 0);
       }
   }
   ```

5. Create `crates/audio/src/lib.rs`:
   ```rust
   //! Subprocess-driven PipeWire audio backend: virtual EQ sink lifecycle and
   //! live parametric-EQ control. Pure generators are unit-tested with no daemon;
   //! the daemon-touching path runs only under owner hardware tests (G8).
   pub mod error;
   pub mod runner;

   pub use error::AudioError;
   pub use runner::{CmdOutput, CommandRunner, MockRunner, RealRunner};
   ```

6. Run the gate and commit:
   ```
   ~/.cargo/bin/cargo test -p arctis-audio
   ```
   Expected: compiles; `error::tests::nonzero_exit_displays_program_and_stderr`, `runner::tests::mock_records_argv_and_replays_output`, and `runner::tests::mock_defaults_to_success_when_queue_empty` all pass (3 passed). Then:
   ```
   git add crates/audio Cargo.toml && git commit -m "audio: scaffold arctis-audio crate with CommandRunner seam"
   ```

---

## Task 2 — EQ band domain model: biquad params, defaults, validation (pure, TDD)

**Files**
- create `crates/audio/src/eq.rs`
- modify `crates/audio/src/lib.rs` (add `pub mod eq;` + re-exports)

**Interfaces**
- Produces `enum BandKind { Peaking, LowShelf, HighShelf }` with `fn label(&self) -> &'static str` → `"bq_peaking"`/`"bq_lowshelf"`/`"bq_highshelf"`
- Produces `struct EqBand { pub kind: BandKind, pub freq_hz: f32, pub q: f32, pub gain_db: f32 }`
- Produces `struct EqModel { pub bands: Vec<EqBand> }`
- Produces consts: `MAX_BANDS = 10`, `GAIN_MIN_DB = -12.0`, `GAIN_MAX_DB = 12.0`, `Q_MIN = 0.3`, `Q_MAX = 10.0`, `FREQ_MIN_HZ = 20.0`, `FREQ_MAX_HZ = 20_000.0`, `SAMPLE_RATE_HZ = 48_000`
- Produces `EqModel::default_10band() -> EqModel`, `EqBand::validate(&self) -> Result<(), AudioError>`, `EqModel::validate(&self) -> Result<(), AudioError>`

**Steps**

1. Write `crates/audio/src/eq.rs` with the model, defaults, validation, and failing tests:
   ```rust
   use crate::error::AudioError;

   /// Biquad band type. Labels are the PipeWire builtin filter labels.
   /// Confirmed: https://docs.pipewire.org/page_module_filter_chain.html
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum BandKind {
       Peaking,
       LowShelf,
       HighShelf,
   }

   impl BandKind {
       /// The PipeWire builtin node `label` for this band type.
       pub fn label(&self) -> &'static str {
           match self {
               BandKind::Peaking => "bq_peaking",
               BandKind::LowShelf => "bq_lowshelf",
               BandKind::HighShelf => "bq_highshelf",
           }
       }
   }

   /// Engine-wide audio constants (ARCHITECTURE G3 / spec §3).
   pub const SAMPLE_RATE_HZ: u32 = 48_000;
   pub const MAX_BANDS: usize = 10;
   pub const GAIN_MIN_DB: f32 = -12.0;
   pub const GAIN_MAX_DB: f32 = 12.0;
   pub const Q_MIN: f32 = 0.3;
   pub const Q_MAX: f32 = 10.0;
   pub const FREQ_MIN_HZ: f32 = 20.0;
   pub const FREQ_MAX_HZ: f32 = 20_000.0;

   /// One parametric EQ band.
   #[derive(Debug, Clone, Copy, PartialEq)]
   pub struct EqBand {
       pub kind: BandKind,
       pub freq_hz: f32,
       pub q: f32,
       pub gain_db: f32,
   }

   impl EqBand {
       pub fn new(kind: BandKind, freq_hz: f32, q: f32, gain_db: f32) -> Self {
           Self { kind, freq_hz, q, gain_db }
       }

       /// Validate ranges. Our chosen defaults (spec §6 — SteelSeries' exact
       /// ranges are unpublished).
       pub fn validate(&self) -> Result<(), AudioError> {
           if !(FREQ_MIN_HZ..=FREQ_MAX_HZ).contains(&self.freq_hz) {
               return Err(AudioError::Invalid(format!(
                   "freq {} Hz out of range {}..={}",
                   self.freq_hz, FREQ_MIN_HZ, FREQ_MAX_HZ
               )));
           }
           if !(Q_MIN..=Q_MAX).contains(&self.q) {
               return Err(AudioError::Invalid(format!(
                   "Q {} out of range {}..={}",
                   self.q, Q_MIN, Q_MAX
               )));
           }
           if !(GAIN_MIN_DB..=GAIN_MAX_DB).contains(&self.gain_db) {
               return Err(AudioError::Invalid(format!(
                   "gain {} dB out of range {}..={}",
                   self.gain_db, GAIN_MIN_DB, GAIN_MAX_DB
               )));
           }
           Ok(())
       }
   }

   /// A full per-sink EQ: an ordered list of bands.
   #[derive(Debug, Clone, PartialEq)]
   pub struct EqModel {
       pub bands: Vec<EqBand>,
   }

   impl EqModel {
       /// 10 flat peaking bands at standard ISO-ish centers; gain 0 dB, Q 1.0.
       pub fn default_10band() -> Self {
           const CENTERS: [f32; MAX_BANDS] = [
               31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
           ];
           let bands = CENTERS
               .iter()
               .map(|&f| EqBand::new(BandKind::Peaking, f, 1.0, 0.0))
               .collect();
           Self { bands }
       }

       pub fn validate(&self) -> Result<(), AudioError> {
           if self.bands.is_empty() {
               return Err(AudioError::Invalid("EQ has no bands".into()));
           }
           if self.bands.len() > MAX_BANDS {
               return Err(AudioError::Invalid(format!(
                   "{} bands exceeds max {}",
                   self.bands.len(),
                   MAX_BANDS
               )));
           }
           for (i, b) in self.bands.iter().enumerate() {
               b.validate().map_err(|e| {
                   AudioError::Invalid(format!("band {i}: {e}"))
               })?;
           }
           Ok(())
       }
   }

   #[cfg(test)]
   mod tests {
       use super::*;

       #[test]
       fn labels_match_pipewire_builtins() {
           assert_eq!(BandKind::Peaking.label(), "bq_peaking");
           assert_eq!(BandKind::LowShelf.label(), "bq_lowshelf");
           assert_eq!(BandKind::HighShelf.label(), "bq_highshelf");
       }

       #[test]
       fn default_is_ten_flat_bands_and_validates() {
           let m = EqModel::default_10band();
           assert_eq!(m.bands.len(), 10);
           assert!(m.bands.iter().all(|b| b.gain_db == 0.0 && b.q == 1.0));
           assert!(m.validate().is_ok());
       }

       #[test]
       fn rejects_out_of_range_gain() {
           let b = EqBand::new(BandKind::Peaking, 1000.0, 1.0, 99.0);
           assert!(b.validate().is_err());
       }

       #[test]
       fn rejects_out_of_range_freq_and_q() {
           assert!(EqBand::new(BandKind::Peaking, 5.0, 1.0, 0.0).validate().is_err());
           assert!(EqBand::new(BandKind::Peaking, 1000.0, 0.01, 0.0).validate().is_err());
       }

       #[test]
       fn rejects_too_many_bands() {
           let m = EqModel {
               bands: vec![EqBand::new(BandKind::Peaking, 1000.0, 1.0, 0.0); MAX_BANDS + 1],
           };
           assert!(m.validate().is_err());
       }
   }
   ```

2. Add to `crates/audio/src/lib.rs` after `pub mod error;`:
   ```rust
   pub mod eq;
   ```
   and after the existing `pub use` block:
   ```rust
   pub use eq::{
       BandKind, EqBand, EqModel, FREQ_MAX_HZ, FREQ_MIN_HZ, GAIN_MAX_DB, GAIN_MIN_DB, MAX_BANDS,
       Q_MAX, Q_MIN, SAMPLE_RATE_HZ,
   };
   ```

3. Run and commit:
   ```
   ~/.cargo/bin/cargo test -p arctis-audio
   ```
   Expected: all `eq::tests` pass (5 new tests; suite green). Then:
   ```
   git add crates/audio && git commit -m "audio: EQ band domain model, defaults, range validation"
   ```

---

## Task 3 — Filter-chain config generator (pure, TDD against an exact conf fixture)

**Files**
- create `crates/audio/src/config.rs`
- create `crates/audio/tests/fixtures/eq_sink_3band.conf` (expected output fixture)
- modify `crates/audio/src/lib.rs`

**Interfaces**
- Produces `struct SinkSpec { pub node_name: String, pub description: String, pub playback_target: Option<String> }`
- Produces `fn band_node_name(index: usize) -> String` → `"eq_band_0"`, `"eq_band_1"`, … (stable, the addressing key root used by live Props in Task 4)
- Produces `fn render_filter_chain_conf(spec: &SinkSpec, eq: &EqModel) -> Result<String, AudioError>`

**Steps**

1. Create the expected fixture `crates/audio/tests/fixtures/eq_sink_3band.conf` (a 3-band model: LowShelf 100/0.7/3.0, Peaking 1000/1.0/0.0, HighShelf 8000/0.7/-2.0; node_name `arctis_eq`, description `Arctis EQ Sink`, target `alsa_output.hw0`). This is the literal string the generator must produce:
   ```
   context.properties = {
       default.clock.rate = 48000
       default.clock.allowed-rates = [ 48000 ]
   }
   context.modules = [
       {   name = libpipewire-module-filter-chain
           args = {
               node.description = "Arctis EQ Sink"
               media.name       = "Arctis EQ Sink"
               filter.graph = {
                   nodes = [
                       {   type = builtin  name = "eq_band_0"  label = bq_lowshelf
                           control = { "Freq" = 100  "Q" = 0.7  "Gain" = 3 }
                       }
                       {   type = builtin  name = "eq_band_1"  label = bq_peaking
                           control = { "Freq" = 1000  "Q" = 1  "Gain" = 0 }
                       }
                       {   type = builtin  name = "eq_band_2"  label = bq_highshelf
                           control = { "Freq" = 8000  "Q" = 0.7  "Gain" = -2 }
                       }
                   ]
                   links = [
                       { output = "eq_band_0:Out"  input = "eq_band_1:In" }
                       { output = "eq_band_1:Out"  input = "eq_band_2:In" }
                   ]
                   inputs  = [ "eq_band_0:In" ]
                   outputs = [ "eq_band_2:Out" ]
               }
               audio.rate     = 48000
               audio.channels = 2
               audio.position = [ FL FR ]
               capture.props = {
                   node.name   = "arctis_eq"
                   media.class = Audio/Sink
               }
               playback.props = {
                   node.name    = "arctis_eq.output"
                   node.passive = true
                   target.object = "alsa_output.hw0"
               }
           }
       }
   ]
   ```
   > Note for the generator: number formatting is "shortest" — `3.0` → `3`, `0.7` → `0.7`, `-2.0` → `-2`, `100.0` → `100`. Implement a small `fmt_num` helper that drops a trailing `.0`. When `playback_target` is `None`, omit the `target.object` line entirely (the chain then follows the default sink).

2. Create `crates/audio/src/config.rs` implementing the generator to match the fixture exactly, plus the failing test:
   ```rust
   use crate::eq::{EqModel, SAMPLE_RATE_HZ};
   use crate::error::AudioError;

   /// Identity + routing for one virtual EQ sink. `node_name` is stable so
   /// create/remove are idempotent (G3).
   #[derive(Debug, Clone)]
   pub struct SinkSpec {
       pub node_name: String,
       pub description: String,
       /// `Some(hardware_sink_node_name)` to pin the tail; `None` follows default.
       pub playback_target: Option<String>,
   }

   /// Stable per-band node name. This is the addressing root the live-EQ Props
   /// generator (Task 4) uses as `"<band_node_name>:Freq"` etc.
   pub fn band_node_name(index: usize) -> String {
       format!("eq_band_{index}")
   }

   /// Format a value the way the conf expects: drop a trailing `.0`.
   fn fmt_num(v: f32) -> String {
       if v.fract() == 0.0 {
           format!("{}", v as i64)
       } else {
           // Trim to a stable short form (no scientific notation for our ranges).
           let s = format!("{v}");
           s
       }
   }

   /// Render the full `pipewire -c` conf for a filter-chain virtual EQ sink.
   pub fn render_filter_chain_conf(
       spec: &SinkSpec,
       eq: &EqModel,
   ) -> Result<String, AudioError> {
       eq.validate()?;

       let mut nodes = String::new();
       for (i, b) in eq.bands.iter().enumerate() {
           nodes.push_str(&format!(
               "                    {{   type = builtin  name = \"{name}\"  label = {label}\n\
                \                        control = {{ \"Freq\" = {f}  \"Q\" = {q}  \"Gain\" = {g} }}\n\
                \                    }}\n",
               name = band_node_name(i),
               label = b.kind.label(),
               f = fmt_num(b.freq_hz),
               q = fmt_num(b.q),
               g = fmt_num(b.gain_db),
           ));
       }

       let mut links = String::new();
       for i in 1..eq.bands.len() {
           links.push_str(&format!(
               "                    {{ output = \"{}:Out\"  input = \"{}:In\" }}\n",
               band_node_name(i - 1),
               band_node_name(i),
           ));
       }

       let first_in = format!("{}:In", band_node_name(0));
       let last_out = format!("{}:Out", band_node_name(eq.bands.len() - 1));

       let target_line = match &spec.playback_target {
           Some(t) => format!("                target.object = \"{t}\"\n"),
           None => String::new(),
       };

       let conf = format!(
   "context.properties = {{
       default.clock.rate = {rate}
       default.clock.allowed-rates = [ {rate} ]
   }}
   context.modules = [
       {{   name = libpipewire-module-filter-chain
           args = {{
               node.description = \"{desc}\"
               media.name       = \"{desc}\"
               filter.graph = {{
                   nodes = [
   {nodes}                ]
                   links = [
   {links}                ]
                   inputs  = [ \"{first_in}\" ]
                   outputs = [ \"{last_out}\" ]
               }}
               audio.rate     = {rate}
               audio.channels = 2
               audio.position = [ FL FR ]
               capture.props = {{
                   node.name   = \"{name}\"
                   media.class = Audio/Sink
               }}
               playback.props = {{
                   node.name    = \"{name}.output\"
                   node.passive = true
   {target_line}            }}
           }}
       }}
   ]
   ",
           rate = SAMPLE_RATE_HZ,
           desc = spec.description,
           nodes = nodes,
           links = links,
           first_in = first_in,
           last_out = last_out,
           name = spec.node_name,
       );
       Ok(conf)
   }

   #[cfg(test)]
   mod tests {
       use super::*;
       use crate::eq::{BandKind, EqBand};

       fn three_band() -> EqModel {
           EqModel {
               bands: vec![
                   EqBand::new(BandKind::LowShelf, 100.0, 0.7, 3.0),
                   EqBand::new(BandKind::Peaking, 1000.0, 1.0, 0.0),
                   EqBand::new(BandKind::HighShelf, 8000.0, 0.7, -2.0),
               ],
           }
       }

       #[test]
       fn renders_exact_fixture() {
           let spec = SinkSpec {
               node_name: "arctis_eq".into(),
               description: "Arctis EQ Sink".into(),
               playback_target: Some("alsa_output.hw0".into()),
           };
           let got = render_filter_chain_conf(&spec, &three_band()).unwrap();
           let want = include_str!("../tests/fixtures/eq_sink_3band.conf");
           assert_eq!(got, want);
       }

       #[test]
       fn omits_target_when_none() {
           let spec = SinkSpec {
               node_name: "arctis_eq".into(),
               description: "Arctis EQ Sink".into(),
               playback_target: None,
           };
           let got = render_filter_chain_conf(&spec, &three_band()).unwrap();
           assert!(!got.contains("target.object"));
       }

       #[test]
       fn band_node_names_are_stable() {
           assert_eq!(band_node_name(0), "eq_band_0");
           assert_eq!(band_node_name(7), "eq_band_7");
       }
   }
   ```
   > **Implementation note for the executor:** the exact whitespace must equal the fixture. If `renders_exact_fixture` fails, diff `got` vs `want` and adjust the format-string indentation (NOT the fixture) until equal — the fixture is the contract. Use `cargo test -p arctis-audio renders_exact_fixture -- --nocapture` and print both on mismatch.

3. Add to `crates/audio/src/lib.rs`:
   ```rust
   pub mod config;
   ```
   and re-export:
   ```rust
   pub use config::{band_node_name, render_filter_chain_conf, SinkSpec};
   ```

4. Run and commit:
   ```
   ~/.cargo/bin/cargo test -p arctis-audio
   ```
   Expected: `config::tests::renders_exact_fixture`, `omits_target_when_none`, `band_node_names_are_stable` pass; full suite green. Then:
   ```
   git add crates/audio && git commit -m "audio: filter-chain conf generator with exact-fixture test"
   ```

---

## Task 4 — Live-EQ Props payload generator (pure, TDD)

**Files**
- create `crates/audio/src/props.rs`
- modify `crates/audio/src/lib.rs`

**Interfaces**
- Produces `fn band_props_json(band_index: usize, band: &EqBand) -> Result<String, AudioError>` → the `<param-json>` third arg for `pw-cli s <id> Props <json>`
- Produces `fn set_band_props_argv(node_id: &str, band_index: usize, band: &EqBand) -> Result<Vec<String>, AudioError>` → full argv after `pw-cli`

**Design decision (isolated for on-machine verification — see Research basis):** the live control key is `format!("{}:{}", band_node_name(i), control)`, e.g. `eq_band_3:Freq`. Keep the three control names in one place so a Task-6 correction is a one-liner.

**Steps**

1. Create `crates/audio/src/props.rs`:
   ```rust
   use crate::config::band_node_name;
   use crate::eq::EqBand;
   use crate::error::AudioError;

   /// Control names exposed by every builtin biquad node.
   /// Confirmed: https://docs.pipewire.org/page_module_filter_chain.html
   /// (The `<node-name>:<control>` addressing form is verified on-machine in
   /// Task 6; if the daemon reports a different convention, change ONLY these
   /// and the key format below.)
   const CTL_FREQ: &str = "Freq";
   const CTL_Q: &str = "Q";
   const CTL_GAIN: &str = "Gain";

   fn fmt_f(v: f32) -> String {
       // Always emit a decimal so the value is unambiguously a float to SPA.
       if v.fract() == 0.0 {
           format!("{v:.1}")
       } else {
           format!("{v}")
       }
   }

   /// The `<param-json>` for `pw-cli s <id> Props <json>` that updates one band's
   /// three controls in place (live; no conf rewrite, no restart — G3).
   pub fn band_props_json(band_index: usize, band: &EqBand) -> Result<String, AudioError> {
       band.validate()?;
       let n = band_node_name(band_index);
       Ok(format!(
           "{{ params = [ \"{n}:{CTL_FREQ}\" {f} \"{n}:{CTL_Q}\" {q} \"{n}:{CTL_GAIN}\" {g} ] }}",
           f = fmt_f(band.freq_hz),
           q = fmt_f(band.q),
           g = fmt_f(band.gain_db),
       ))
   }

   /// Full argv (after the `pw-cli` program) to apply one band live.
   pub fn set_band_props_argv(
       node_id: &str,
       band_index: usize,
       band: &EqBand,
   ) -> Result<Vec<String>, AudioError> {
       if node_id.trim().is_empty() {
           return Err(AudioError::Invalid("empty node id".into()));
       }
       Ok(vec![
           "s".to_string(),
           node_id.to_string(),
           "Props".to_string(),
           band_props_json(band_index, band)?,
       ])
   }

   #[cfg(test)]
   mod tests {
       use super::*;
       use crate::eq::BandKind;

       #[test]
       fn json_addresses_band_controls_by_node_name() {
           let b = EqBand::new(BandKind::Peaking, 1200.0, 1.0, -4.5);
           let json = band_props_json(3, &b).unwrap();
           assert_eq!(
               json,
               "{ params = [ \"eq_band_3:Freq\" 1200.0 \"eq_band_3:Q\" 1.0 \"eq_band_3:Gain\" -4.5 ] }"
           );
       }

       #[test]
       fn argv_is_s_id_props_json() {
           let b = EqBand::new(BandKind::Peaking, 1000.0, 1.0, 0.0);
           let argv = set_band_props_argv("57", 0, &b).unwrap();
           assert_eq!(argv[0], "s");
           assert_eq!(argv[1], "57");
           assert_eq!(argv[2], "Props");
           assert_eq!(
               argv[3],
               "{ params = [ \"eq_band_0:Freq\" 1000.0 \"eq_band_0:Q\" 1.0 \"eq_band_0:Gain\" 0.0 ] }"
           );
       }

       #[test]
       fn rejects_empty_node_id() {
           let b = EqBand::new(BandKind::Peaking, 1000.0, 1.0, 0.0);
           assert!(set_band_props_argv("  ", 0, &b).is_err());
       }

       #[test]
       fn rejects_invalid_band() {
           let b = EqBand::new(BandKind::Peaking, 1000.0, 1.0, 999.0);
           assert!(band_props_json(0, &b).is_err());
       }
   }
   ```

2. Add to `crates/audio/src/lib.rs`:
   ```rust
   pub mod props;
   ```
   and re-export:
   ```rust
   pub use props::{band_props_json, set_band_props_argv};
   ```

3. Run and commit:
   ```
   ~/.cargo/bin/cargo test -p arctis-audio
   ```
   Expected: `props::tests` (4 tests) pass; suite green. Then:
   ```
   git add crates/audio && git commit -m "audio: live-EQ Props payload generator (no-restart, G3)"
   ```

---

## Task 5 — `AudioBackend`: create / remove / apply over `CommandRunner` (TDD with `MockRunner`)

**Files**
- create `crates/audio/src/backend.rs`
- modify `crates/audio/src/lib.rs`

**Interfaces**
- Consumes `CommandRunner`, `SinkSpec`, `EqModel`, `EqBand`, generators from Tasks 3–4
- Produces `struct AudioBackend<R: CommandRunner> { … }` with:
  - `fn new(runner: R, spec: SinkSpec) -> Self`
  - `fn sink_exists(&mut self) -> Result<bool, AudioError>` (parses `pw-cli ls Node` stdout for the stable `node.name`)
  - `fn create(&mut self, eq: &EqModel) -> Result<ConfHandle, AudioError>` (idempotent: if `sink_exists`, returns existing without spawning a new instance)
  - `fn remove(&mut self) -> Result<(), AudioError>`
  - `fn find_node_id(&mut self) -> Result<String, AudioError>` (resolves the filter node id for live Props)
  - `fn apply_band(&mut self, band_index: usize, band: &EqBand) -> Result<(), AudioError>`
  - `fn apply_all(&mut self, eq: &EqModel) -> Result<(), AudioError>`
- Produces `struct ConfHandle { pub conf_path: PathBuf }` (path to the temp conf the dedicated instance reads)

**Design notes**
- `create` writes the rendered conf to a temp file (`std::env::temp_dir()` + stable name `arctis_eq.<node_name>.conf` so it's reconcilable) and spawns the dedicated instance via the runner: `pipewire` with args `["-c", "<conf_path>"]`. Real spawning is long-lived; the **OWNER-RUN** Task 6 covers actual lifecycle. Under `MockRunner` we assert the exact argv only.
- `remove` is idempotent: if `!sink_exists`, returns `Ok(())`. Teardown of the dedicated instance is by `node.name` reconciliation — for v1 we destroy the filter node via `pw-cli destroy <id>` when found, and the conf file is removed. (Killing the dedicated `pipewire -c` process is owner-managed in Task 6; record this limitation in a doc-comment as a LATER refinement.)
- `find_node_id` runs `pw-cli ls Node` and parses the `id:` line whose block contains `node.name = "<spec.node_name>"`. Parsing is tested with a canned `pw-cli ls Node` fixture in stdout.
- All errors typed; non-zero exits become `AudioError::NonZeroExit`. No `unwrap`/`expect` on runtime paths (G7).

**Steps**

1. Create `crates/audio/src/backend.rs`:
   ```rust
   use crate::config::{render_filter_chain_conf, SinkSpec};
   use crate::eq::{EqBand, EqModel};
   use crate::error::AudioError;
   use crate::props::set_band_props_argv;
   use crate::runner::{CmdOutput, CommandRunner};
   use std::path::PathBuf;

   /// Handle to the on-disk conf the dedicated `pipewire -c` instance reads.
   #[derive(Debug, Clone, PartialEq, Eq)]
   pub struct ConfHandle {
       pub conf_path: PathBuf,
   }

   pub struct AudioBackend<R: CommandRunner> {
       runner: R,
       spec: SinkSpec,
   }

   impl<R: CommandRunner> AudioBackend<R> {
       pub fn new(runner: R, spec: SinkSpec) -> Self {
           Self { runner, spec }
       }

       /// Expose the runner for assertions in tests.
       #[cfg(test)]
       pub fn runner(&self) -> &R {
           &self.runner
       }

       fn check(out: CmdOutput, program: &str) -> Result<CmdOutput, AudioError> {
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

       fn conf_path(&self) -> PathBuf {
           let mut p = std::env::temp_dir();
           p.push(format!("arctis_eq.{}.conf", self.spec.node_name));
           p
       }

       /// True if a node with our stable `node.name` is already present.
       pub fn sink_exists(&mut self) -> Result<bool, AudioError> {
           let out = self.runner.run("pw-cli", &["ls", "Node"])?;
           let out = Self::check(out, "pw-cli")?;
           Ok(out
               .stdout
               .contains(&format!("node.name = \"{}\"", self.spec.node_name)))
       }

       /// Create the sink idempotently (G3): if it already exists, reuse it.
       pub fn create(&mut self, eq: &EqModel) -> Result<ConfHandle, AudioError> {
           let path = self.conf_path();
           if self.sink_exists()? {
               return Ok(ConfHandle { conf_path: path });
           }
           let conf = render_filter_chain_conf(&self.spec, eq)?;
           std::fs::write(&path, conf).map_err(|e| AudioError::Spawn {
               program: "write-conf".to_string(),
               source_msg: e.to_string(),
           })?;
           let path_str = path.to_string_lossy().into_owned();
           let out = self.runner.run("pipewire", &["-c", &path_str])?;
           Self::check(out, "pipewire")?;
           Ok(ConfHandle { conf_path: path })
       }

       /// Resolve the filter node id for live Props. Parses `pw-cli ls Node`.
       pub fn find_node_id(&mut self) -> Result<String, AudioError> {
           let out = self.runner.run("pw-cli", &["ls", "Node"])?;
           let out = Self::check(out, "pw-cli")?;
           parse_node_id(&out.stdout, &self.spec.node_name)
       }

       /// Apply one band live via `pw-cli s <id> Props …` (no restart — G3).
       pub fn apply_band(&mut self, band_index: usize, band: &EqBand) -> Result<(), AudioError> {
           let id = self.find_node_id()?;
           let argv = set_band_props_argv(&id, band_index, band)?;
           let args: Vec<&str> = argv.iter().map(String::as_str).collect();
           let out = self.runner.run("pw-cli", &args)?;
           Self::check(out, "pw-cli")?;
           Ok(())
       }

       /// Apply every band live (used by future re-apply-on-startup; here for E2E).
       pub fn apply_all(&mut self, eq: &EqModel) -> Result<(), AudioError> {
           eq.validate()?;
           let id = self.find_node_id()?;
           for (i, b) in eq.bands.iter().enumerate() {
               let argv = set_band_props_argv(&id, i, b)?;
               let args: Vec<&str> = argv.iter().map(String::as_str).collect();
               let out = self.runner.run("pw-cli", &args)?;
               Self::check(out, "pw-cli")?;
           }
           Ok(())
       }

       /// Remove the sink idempotently: no-op if absent; else destroy the node
       /// and delete the conf. (Stopping the dedicated `pipewire -c` process is
       /// owner-managed for v1 — see Task 6; LATER: track and kill the child.)
       pub fn remove(&mut self) -> Result<(), AudioError> {
           if !self.sink_exists()? {
               let _ = std::fs::remove_file(self.conf_path());
               return Ok(());
           }
           let id = self.find_node_id()?;
           let out = self.runner.run("pw-cli", &["destroy", &id])?;
           Self::check(out, "pw-cli")?;
           let _ = std::fs::remove_file(self.conf_path());
           Ok(())
       }
   }

   /// Parse the numeric id of the node whose block declares `node.name = "<name>"`
   /// in `pw-cli ls Node` output.
   fn parse_node_id(stdout: &str, node_name: &str) -> Result<String, AudioError> {
       let needle = format!("node.name = \"{node_name}\"");
       let mut current_id: Option<String> = None;
       for line in stdout.lines() {
           let trimmed = line.trim_start();
           if let Some(rest) = trimmed.strip_prefix("id ") {
               // line form: `id 57, type PipeWire:Interface:Node/3`
               let id = rest
                   .split([',', ' '])
                   .next()
                   .unwrap_or("")
                   .trim()
                   .to_string();
               if !id.is_empty() {
                   current_id = Some(id);
               }
           }
           if trimmed.contains(&needle) {
               if let Some(id) = current_id.clone() {
                   return Ok(id);
               }
           }
       }
       Err(AudioError::Parse {
           what: "node id".to_string(),
           detail: format!("no node with node.name=\"{node_name}\""),
       })
   }

   #[cfg(test)]
   mod tests {
       use super::*;
       use crate::eq::{BandKind, EqBand, EqModel};
       use crate::runner::MockRunner;

       fn spec() -> SinkSpec {
           SinkSpec {
               node_name: "arctis_eq".into(),
               description: "Arctis EQ Sink".into(),
               playback_target: None,
           }
       }

       const LS_WITH_SINK: &str = "\
   id 40, type PipeWire:Interface:Node/3
       node.name = \"alsa_output.pci\"
   id 57, type PipeWire:Interface:Node/3
       node.name = \"arctis_eq\"
   id 58, type PipeWire:Interface:Node/3
       node.name = \"arctis_eq.output\"
   ";

       #[test]
       fn parses_node_id_for_stable_name() {
           assert_eq!(parse_node_id(LS_WITH_SINK, "arctis_eq").unwrap(), "57");
       }

       #[test]
       fn parse_errors_when_absent() {
           assert!(parse_node_id(LS_WITH_SINK, "nope").is_err());
       }

       #[test]
       fn create_is_idempotent_when_sink_exists() {
           let runner = MockRunner::new().with_output(0, LS_WITH_SINK, "");
           let mut be = AudioBackend::new(runner, spec());
           be.create(&EqModel::default_10band()).unwrap();
           // Only the `ls Node` existence check ran; no `pipewire -c` spawn.
           let calls = &be.runner().calls;
           assert_eq!(calls.len(), 1);
           assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
       }

       #[test]
       fn create_spawns_dedicated_instance_when_absent() {
           let runner = MockRunner::new()
               .with_output(0, "id 1, type PipeWire:Interface:Node/3\n    node.name = \"x\"\n", "")
               .with_output(0, "", ""); // pipewire -c
           let mut be = AudioBackend::new(runner, spec());
           be.create(&EqModel::default_10band()).unwrap();
           let calls = &be.runner().calls;
           assert_eq!(calls[0], vec!["pw-cli", "ls", "Node"]);
           assert_eq!(calls[1][0], "pipewire");
           assert_eq!(calls[1][1], "-c");
           assert!(calls[1][2].ends_with("arctis_eq.conf"));
       }

       #[test]
       fn apply_band_emits_exact_pw_cli_props_argv() {
           let runner = MockRunner::new()
               .with_output(0, LS_WITH_SINK, "") // find_node_id
               .with_output(0, "", ""); // the set
           let mut be = AudioBackend::new(runner, spec());
           let band = EqBand::new(BandKind::Peaking, 1200.0, 1.0, -4.5);
           be.apply_band(3, &band).unwrap();
           let last = be.runner().last_call().unwrap();
           assert_eq!(
               last,
               &vec![
                   "pw-cli".to_string(),
                   "s".to_string(),
                   "57".to_string(),
                   "Props".to_string(),
                   "{ params = [ \"eq_band_3:Freq\" 1200.0 \"eq_band_3:Q\" 1.0 \"eq_band_3:Gain\" -4.5 ] }".to_string(),
               ]
           );
       }

       #[test]
       fn nonzero_exit_is_typed_error() {
           let runner = MockRunner::new().with_output(1, "", "denied");
           let mut be = AudioBackend::new(runner, spec());
           let err = be.sink_exists().unwrap_err();
           assert!(matches!(err, AudioError::NonZeroExit { status: 1, .. }));
       }

       #[test]
       fn remove_is_noop_when_absent() {
           let runner = MockRunner::new().with_output(0, "id 1\n    node.name = \"other\"\n", "");
           let mut be = AudioBackend::new(runner, spec());
           be.remove().unwrap();
           assert_eq!(be.runner().calls.len(), 1); // only the existence check
       }
   }
   ```

2. Add to `crates/audio/src/lib.rs`:
   ```rust
   pub mod backend;
   ```
   and re-export:
   ```rust
   pub use backend::{AudioBackend, ConfHandle};
   ```

3. Run and commit:
   ```
   ~/.cargo/bin/cargo test -p arctis-audio
   ```
   Expected: all `backend::tests` (7 tests) pass; full crate suite green. Then:
   ```
   git add crates/audio && git commit -m "audio: AudioBackend create/remove/apply over CommandRunner"
   ```

---

## Task 6 — `asm-cli` subcommands + OWNER-RUN hardware E2E

**Files**
- modify `crates/cli/Cargo.toml` (add `arctis-audio` dep)
- modify `crates/cli/src/main.rs` (add `sink` and `eq` subcommand trees)

**Interfaces**
- Consumes `arctis_audio::{AudioBackend, RealRunner, SinkSpec, EqModel, EqBand, BandKind}`
- Produces CLI: `asm-cli sink create [--target <hw_sink>]`, `asm-cli sink remove`, `asm-cli eq set --band <n> --freq <hz> --q <q> --gain <db> [--kind peaking|lowshelf|highshelf]`, `asm-cli eq show`

**Steps**

1. Add to `crates/cli/Cargo.toml` `[dependencies]`:
   ```toml
   arctis-audio = { workspace = true }
   ```

2. Extend `crates/cli/src/main.rs`. Add imports near the top:
   ```rust
   use arctis_audio::{AudioBackend, BandKind, EqBand, EqModel, RealRunner, SinkSpec};
   ```
   Add variants to `enum Command` (after `Probe`):
   ```rust
       /// Manage the PipeWire virtual EQ sink.
       Sink {
           #[command(subcommand)]
           action: SinkAction,
       },
       /// Live parametric EQ control on the virtual sink.
       Eq {
           #[command(subcommand)]
           action: EqAction,
       },
   ```
   Add these enums after `enum Command`:
   ```rust
   #[derive(Subcommand)]
   enum SinkAction {
       /// Create the virtual EQ sink (idempotent) with 10 flat bands.
       Create {
           /// Hardware sink node.name to feed; omit to follow the default sink.
           #[arg(long)]
           target: Option<String>,
       },
       /// Remove the virtual EQ sink (idempotent).
       Remove,
   }

   #[derive(Subcommand)]
   enum EqAction {
       /// Set one band live (no restart).
       Set {
           #[arg(long)]
           band: usize,
           #[arg(long)]
           freq: f32,
           #[arg(long)]
           q: f32,
           #[arg(long)]
           gain: f32,
           #[arg(long, default_value = "peaking")]
           kind: String,
       },
       /// Show the resolved node id and confirm the sink is present.
       Show,
   }

   const SINK_NAME: &str = "arctis_eq";
   const SINK_DESC: &str = "Arctis EQ Sink";

   fn band_kind(s: &str) -> Result<BandKind, String> {
       match s {
           "peaking" => Ok(BandKind::Peaking),
           "lowshelf" => Ok(BandKind::LowShelf),
           "highshelf" => Ok(BandKind::HighShelf),
           other => Err(format!("unknown band kind: {other}")),
       }
   }
   ```
   Add match arms inside `match cli.command { … }` (after the `Probe` arm):
   ```rust
       Command::Sink { action } => {
           let target = match &action {
               SinkAction::Create { target } => target.clone(),
               SinkAction::Remove => None,
           };
           let spec = SinkSpec {
               node_name: SINK_NAME.to_string(),
               description: SINK_DESC.to_string(),
               playback_target: target,
           };
           let mut be = AudioBackend::new(RealRunner, spec);
           match action {
               SinkAction::Create { .. } => match be.create(&EqModel::default_10band()) {
                   Ok(h) => {
                       println!("sink ready: {SINK_NAME} (conf {})", h.conf_path.display());
                       ExitCode::SUCCESS
                   }
                   Err(e) => {
                       eprintln!("error creating sink: {e}");
                       ExitCode::FAILURE
                   }
               },
               SinkAction::Remove => match be.remove() {
                   Ok(()) => {
                       println!("sink removed: {SINK_NAME}");
                       ExitCode::SUCCESS
                   }
                   Err(e) => {
                       eprintln!("error removing sink: {e}");
                       ExitCode::FAILURE
                   }
               },
           }
       }
       Command::Eq { action } => {
           let spec = SinkSpec {
               node_name: SINK_NAME.to_string(),
               description: SINK_DESC.to_string(),
               playback_target: None,
           };
           let mut be = AudioBackend::new(RealRunner, spec);
           match action {
               EqAction::Set { band, freq, q, gain, kind } => {
                   let kind = match band_kind(&kind) {
                       Ok(k) => k,
                       Err(e) => {
                           eprintln!("error: {e}");
                           return ExitCode::FAILURE;
                       }
                   };
                   let b = EqBand::new(kind, freq, q, gain);
                   match be.apply_band(band, &b) {
                       Ok(()) => {
                           println!("band {band} set: {freq} Hz Q {q} {gain} dB");
                           ExitCode::SUCCESS
                       }
                       Err(e) => {
                           eprintln!("error setting band: {e}");
                           ExitCode::FAILURE
                       }
                   }
               }
               EqAction::Show => match be.find_node_id() {
                   Ok(id) => {
                       println!("{SINK_NAME} present, node id {id}");
                       ExitCode::SUCCESS
                   }
                   Err(e) => {
                       eprintln!("error: {e}");
                       ExitCode::FAILURE
                   }
               },
           }
       }
   ```

3. Build the workspace (compile gate only — no daemon touched):
   ```
   ~/.cargo/bin/cargo build --workspace
   ```
   Expected: clean build of `arctis-domain`, `arctis-device`, `arctis-audio`, `arctis-cli`. Then run the full default gate:
   ```
   ~/.cargo/bin/cargo test --workspace
   ```
   Expected: green; **no test spawns PipeWire** (all audio tests use `MockRunner` / pure generators). Commit:
   ```
   git add crates/cli Cargo.toml && git commit -m "cli: add sink and eq subcommands over arctis-audio"
   ```

4. **OWNER-RUN (manual, on real PipeWire — out of CI, G8 / spec §14, no headset required):** Validate the live path and **pin the live-Props key form** (the one detail not confirmed by docs).
   a. Create the sink: `~/.cargo/bin/cargo run -p arctis-cli -- sink create`
      Expect: `sink ready: arctis_eq (conf /tmp/arctis_eq.arctis_eq.conf)`.
   b. Confirm it appeared: `pw-cli ls Node | grep -A1 arctis_eq` (should list `node.name = "arctis_eq"`), and it shows in `pavucontrol` / `wpctl status` as an output sink. Note its node id.
   c. **Verify control addressing:** `pw-cli enum-params <id> Props` and/or `pw-dump <id>` — confirm the per-band controls are addressable as `eq_band_<n>:Freq` / `:Q` / `:Gain`. **If the daemon reports a different key convention, edit ONLY the constants/format in `crates/audio/src/props.rs`, update the Task-4 fixture expectation, and re-run `~/.cargo/bin/cargo test -p arctis-audio`.**
   d. Set a band live: `~/.cargo/bin/cargo run -p arctis-cli -- eq set --band 3 --freq 1200 --q 1.0 --gain -6 --kind peaking`
      Expect success line, **no audio glitch / no restart**.
   e. Confirm the change took effect: `pw-dump <id>` (or `pw-cli enum-params <id> Props`) shows `eq_band_3` Gain ≈ `-6`. This is the headline G3 validation.
   f. Tear down: `~/.cargo/bin/cargo run -p arctis-cli -- sink remove`, then `pw-cli ls Node | grep arctis_eq` returns nothing (and stop the dedicated `pipewire -c` process if it was started as a child — v1 limitation noted in Task 5).
   Record results (incl. any key-form correction) in the PR description.

---

## Self-Review

**Spec coverage**
- §6 virtual EQ sink via `libpipewire-module-filter-chain` `Audio/Sink` — Task 3 conf generator (`capture.props media.class = Audio/Sink`, `playback.props node.passive`).
- §6 live parametric EQ via Props with named `Freq/Q/Gain`, no restart — Tasks 4 (payload) + 5 (`apply_band`/`apply_all`), OWNER-RUN Task 6e validates G3.
- §6 idempotency / stable `node.name` — `band_node_name`, `sink_exists`, idempotent `create`/`remove` (Tasks 3, 5).
- §6 EQ defaults (≤10 bands, ±12 dB, Q 0.3–10, 20 Hz–20 kHz) — Task 2 consts + validation.
- §14 / G8 testing split — pure generators unit-tested with `MockRunner`; daemon path is OWNER-RUN and out of CI (every task's default gate is `cargo test`, no PipeWire).
- Persistence caveat (Props not persisted; re-apply on startup) — recorded in Research basis + `props.rs` doc-comment; orchestration deferred.
- Workspace wiring (member + `[workspace.dependencies]` + cli dep) — Task 1 + Task 6.
- Guardrails: G1 (single biquad builder/config generator; `CommandRunner`/`MockRunner` mirror `Transport`/`MockTransport`), G3 (live Props, 48 kHz, idempotent), G6 (no `tauri`; small focused files), G7 (typed `AudioError`, no `unwrap`/`expect` on runtime paths).

**Placeholder scan** — no `TBD`, no "similar to above", no `todo!()`/`unimplemented!()`. Every code step is complete Rust/TOML/conf. The one genuinely unconfirmed detail (live-Props key form) is isolated to three constants in `props.rs` and explicitly verified-then-pinned in OWNER-RUN Task 6c, per the founded-in-research rule (verify on-machine rather than assert a guess).

**Type consistency** — `AudioError` is the single error type across the crate; `CmdOutput`/`CommandRunner` consistent between real and mock; `SinkSpec`/`EqModel`/`EqBand` flow unchanged from generators into `AudioBackend` and the CLI; `band_node_name` is the single source for the per-band addressing root shared by config (Task 3) and Props (Task 4).

**CI note** — the default gate is `~/.cargo/bin/cargo test --workspace` and never starts a PipeWire daemon. Hardware/PipeWire E2E (Task 6 steps 4a–4f) is **owner-run, manual, and out of CI** (G8, spec §14).

## Open questions (carry into execution / later plans)
1. **Live-Props key form** — `eq_band_<n>:Freq` vs an alternative convention; pinned in Task 6c on the real daemon.
2. **Dedicated-instance lifecycle** — v1 leaves stopping the `pipewire -c` child to the owner; a LATER engine plan should track/own the child process (or move to a managed module load) for clean teardown (G10).
3. **Default playback target** — when `--target` is omitted the chain follows the default sink; whether to auto-resolve the current hardware sink name is an engine-orchestrator concern (deferred).
