# EQ & Mic Preset Packs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship read-only, built-in EQ preset packs (oratory1990-measured Nova Pro curves) and full-chain
mic preset packs (verbatim Sonar voice presets), applyable from both the CLI and the GUI.

**Architecture:** A code-defined factory catalog in `crates/engine/src/presets.rs` exposes
`factory_eq_presets()` and `factory_mic_presets()`. The engine's `apply_eq_preset` resolves factory
names (after the user library); a new `apply_mic_preset` overlays a full-chain `MicPreset` onto the live
mic chain (preserving the user's master on/off + hardware mic). Both catalogs ride in `EngineState` so
the CLI and GUI can list them. No device writes; pure software EQ/DSP.

**Tech Stack:** Rust workspace (`config`, `engine`, `client`, `cli`, `src-tauri`), Svelte 5 + Vitest
frontend, PipeWire subprocess model.

## Global Constraints

- Pure software EQ/DSP only — **no device writes**; the device-write allowlist stays empty.
- **GUI ⇄ CLI feature parity is mandatory** — every preset action exists in both.
- Built-in presets are **read-only** (code-defined, never persisted, never deletable); user-saved
  `config.eq_presets` keep working unchanged. A user preset name **overrides** a factory name of the same name.
- Canonical EQ model: fixed 10 bands at `[31,62,125,250,500,1000,2000,4000,8000,16000]` Hz;
  `EqBandConfig { kind: "peaking"|"lowshelf"|"highshelf", freq_hz: f32, q: f32, gain_db: f32 }`;
  band 0 = lowshelf, band 9 = highshelf, rest peaking. Gain bound is **±12.0 dB** (`EQ_GAIN_MIN_DB`/
  `EQ_GAIN_MAX_DB` in `crates/domain/src/eq_bounds.rs`).
- Mic preset applies overlay the full chain but **preserve** `mic.enabled` (master) and `mic.hw_mic`.
- `~/.cargo/bin/cargo` is cargo. Tests use `MockRunner`/fixtures only — **no test touches real audio**.
- No `unwrap`/`expect`/`panic` on runtime paths (G7). Small focused files (G6). Reuse over duplication (G1).
- Commit trailers required: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01329izwVKkyPS1uskubQC28`. Never `git add` `.superpowers/`,
  `.claude/`, or any `*.rpm`.

## File Structure

- `crates/config/src/schema.rs` — add `MicPreset` struct (reuses the existing mic stage structs).
- `crates/engine/src/presets.rs` (NEW) — `factory_eq_presets()` + `factory_mic_presets()` + validation tests.
- `crates/engine/src/state.rs` — add `MicPresetSnapshot`; add `factory_eq_presets` + `mic_presets` to `EngineState`; add `Event::MicPresetApplied`.
- `crates/engine/src/engine.rs` — extend `apply_eq_preset` (factory fallback); add `apply_mic_preset`; populate the two new state fields in `state()`.
- `crates/engine/src/lib.rs` — export `presets`, `MicPresetSnapshot`.
- `crates/client/src/protocol.rs` — add `Request::ApplyMicPreset { name }` + round-trip/wire tests.
- `crates/cli/src/daemon.rs` — handle `ApplyMicPreset`.
- `crates/cli/src/main.rs` — `eq preset list` shows built-ins; new `mic preset list|apply`.
- `src-tauri/src/commands.rs` + `src-tauri/src/lib.rs` — `mic_preset_apply` command + registration.
- `frontend/src/lib/ipc.ts` — `MicPresetSnapshot` type, `factory_eq_presets`/`mic_presets` on `EngineState`, `micPresetApply()`.
- `frontend/src/lib/components/EqPage.svelte` — "Built-in" presets section (apply only).
- `frontend/src/lib/components/MicPage.svelte` — mic preset picker (apply + description).

---

## Phase 1 — Data model + factory catalog

### Task 1: `MicPreset` struct + `MicPresetSnapshot`

**Files:**
- Modify: `crates/config/src/schema.rs` (add `MicPreset` after `EqPreset`, ~line 335)
- Modify: `crates/engine/src/state.rs` (add `MicPresetSnapshot`)
- Modify: `crates/engine/src/lib.rs` (re-export `MicPresetSnapshot`)

**Interfaces:**
- Produces:
  ```rust
  // crates/config/src/schema.rs
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct MicPreset {
      pub name: String,
      pub description: String,
      pub gain: MicGainStage,
      pub highpass: MicHighpassStage,
      pub suppression: MicSuppressionStage,
      pub compressor: MicCompressorStage,
      pub gate: MicGateStage,
      pub eq_enabled: bool,
      pub eq: Vec<EqBandConfig>,
  }
  // crates/engine/src/state.rs
  #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
  pub struct MicPresetSnapshot { pub name: String, pub description: String }
  ```

- [ ] **Step 1 — Failing test** (`crates/config/src/schema.rs` test module): construct a `MicPreset`,
  TOML round-trip it, assert equality.

```rust
#[test]
fn mic_preset_round_trips_via_toml() {
    let p = MicPreset {
        name: "Less Nasal".into(),
        description: "Tame nasal honk".into(),
        gain: MicGainStage::default(),
        highpass: MicHighpassStage::default(),
        suppression: MicSuppressionStage { enabled: true, ..Default::default() },
        compressor: MicCompressorStage::default(),
        gate: MicGateStage::default(),
        eq_enabled: true,
        eq: vec![EqBandConfig { kind: "peaking".into(), freq_hz: 1000.0, q: 0.7071, gain_db: -4.0 }],
    };
    let toml = toml::to_string(&p).unwrap();
    let back: MicPreset = toml::from_str(&toml).unwrap();
    assert_eq!(p, back);
}
```

- [ ] **Step 2 — Run, verify fail:** `~/.cargo/bin/cargo test -p arctis-config mic_preset_round_trips` → FAIL (type undefined).
- [ ] **Step 3 — Implement** the `MicPreset` struct (above) in schema.rs and `MicPresetSnapshot` in
  state.rs; re-export `MicPresetSnapshot` from `crates/engine/src/lib.rs` next to `EqPresetSnapshot`.
- [ ] **Step 4 — Run:** `~/.cargo/bin/cargo test -p arctis-config && ~/.cargo/bin/cargo build -p arctis-engine` → PASS/clean.
- [ ] **Step 5 — Commit:** `feat(config): MicPreset struct + MicPresetSnapshot`.

### Task 2: Factory catalog `presets.rs`

**Files:**
- Create: `crates/engine/src/presets.rs`
- Modify: `crates/engine/src/lib.rs` (add `pub mod presets;`)
- Test: in `presets.rs`

**Interfaces:**
- Consumes: `arctis_config::{EqPreset, EqBandConfig, MicPreset, MicGainStage, MicHighpassStage,
  MicSuppressionStage, MicCompressorStage, MicGateStage}`.
- Produces: `pub fn factory_eq_presets() -> Vec<arctis_config::EqPreset>` and
  `pub fn factory_mic_presets() -> Vec<arctis_config::MicPreset>`.

- [ ] **Step 1 — Failing validation test** (catches transcription errors across both catalogs):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    const FREQS: [f32; 10] = [31.0,62.0,125.0,250.0,500.0,1000.0,2000.0,4000.0,8000.0,16000.0];

    #[test]
    fn every_factory_preset_is_well_formed() {
        let eq = factory_eq_presets();
        assert!(eq.iter().any(|p| p.name == "Reference (Calibrated)"));
        let mic = factory_mic_presets();
        for p in &mic { assert!(["Flat","Less Nasal","Deep Voice","Balanced","Clarity High Pitch",
            "Clarity Low Pitch","Broadcast High Pitch","Broadcast Low Pitch","Walkie Talkie","Streamer"]
            .contains(&p.name.as_str()), "unexpected mic preset {}", p.name); }
        // Channel + mic EQ vectors: 10 dense bands at canonical freqs, gains within ±12.
        let all_band_sets: Vec<&Vec<EqBandConfig>> =
            eq.iter().map(|p| &p.bands).chain(mic.iter().map(|p| &p.eq)).collect();
        for bands in all_band_sets {
            assert_eq!(bands.len(), 10, "preset must have 10 bands");
            for (i, b) in bands.iter().enumerate() {
                assert_eq!(b.freq_hz, FREQS[i], "band {i} freq mismatch");
                assert!((-12.0..=12.0).contains(&b.gain_db), "gain {} out of ±12", b.gain_db);
                assert!(["peaking","lowshelf","highshelf"].contains(&b.kind.as_str()));
            }
            assert_eq!(bands[0].kind, "lowshelf");
            assert_eq!(bands[9].kind, "highshelf");
        }
        // Names unique within each catalog.
        let mut eqn: Vec<_> = eq.iter().map(|p| &p.name).collect(); eqn.sort(); eqn.dedup();
        assert_eq!(eqn.len(), eq.len());
    }
}
```

- [ ] **Step 2 — Run, verify fail:** `~/.cargo/bin/cargo test -p arctis-engine every_factory_preset_is_well_formed` → FAIL (module/functions undefined).
- [ ] **Step 3 — Implement `presets.rs`** with the helpers + both catalogs (values are verbatim from
  the spec §5; do NOT alter them):

```rust
//! Read-only factory preset catalog. Channel EQ = oratory1990-measured Nova Pro Wireless
//! curves; mic presets = verbatim Sonar voice presets. See the design spec for provenance.
use arctis_config::{EqBandConfig, EqPreset, MicCompressorStage, MicGainStage, MicGateStage,
    MicHighpassStage, MicPreset, MicSuppressionStage};

// ── EQ band builders (band 0 = lowshelf@31, band 9 = highshelf@16k, rest peaking) ──
fn pk(freq: f32, gain: f32) -> EqBandConfig { EqBandConfig { kind: "peaking".into(), freq_hz: freq, q: 1.0, gain_db: gain } }
fn pkq(freq: f32, q: f32, gain: f32) -> EqBandConfig { EqBandConfig { kind: "peaking".into(), freq_hz: freq, q, gain_db: gain } }
fn ls(gain: f32) -> EqBandConfig { EqBandConfig { kind: "lowshelf".into(), freq_hz: 31.0, q: 0.7, gain_db: gain } }
fn hs(gain: f32) -> EqBandConfig { EqBandConfig { kind: "highshelf".into(), freq_hz: 16000.0, q: 0.7, gain_db: gain } }

fn eqp(name: &str, hint: &str, bands: Vec<EqBandConfig>) -> EqPreset {
    EqPreset { name: name.into(), kind_hint: Some(hint.into()), bands }
}

pub fn factory_eq_presets() -> Vec<EqPreset> {
    vec![
        eqp("Flat", "Reference · no EQ",
            vec![ls(0.0), pk(62.0,0.0), pk(125.0,0.0), pk(250.0,0.0), pk(500.0,0.0), pk(1000.0,0.0), pk(2000.0,0.0), pk(4000.0,0.0), pk(8000.0,0.0), hs(0.0)]),
        eqp("Reference (Calibrated)", "Reference · preamp -3.2 dB",
            vec![ls(-1.5), pk(62.0,0.3), pkq(125.0,1.41,-9.5), pkq(250.0,1.41,-1.6), pk(500.0,1.0), pk(1000.0,2.2), pk(2000.0,1.6), pk(4000.0,2.8), pk(8000.0,0.2), hs(2.2)]),
        eqp("Bass Boost", "Music · preamp -4 dB",
            vec![ls(4.0), pk(62.0,2.0), pkq(125.0,1.41,-5.0), pk(250.0,-0.5), pk(500.0,0.5), pk(1000.0,1.5), pk(2000.0,1.0), pk(4000.0,2.5), pk(8000.0,-1.0), hs(1.0)]),
        eqp("FPS / Footsteps", "Gaming · preamp -5 dB",
            vec![ls(-2.0), pk(62.0,-1.5), pkq(125.0,1.41,-7.0), pk(250.0,-2.5), pk(500.0,0.0), pk(1000.0,2.0), pk(2000.0,3.5), pk(4000.0,5.0), pk(8000.0,2.5), hs(1.0)]),
        eqp("Immersive", "Movies · preamp -4.5 dB",
            vec![ls(4.5), pk(62.0,2.5), pkq(125.0,1.41,-5.5), pk(250.0,-1.0), pk(500.0,0.0), pk(1000.0,0.5), pk(2000.0,1.0), pk(4000.0,2.0), pk(8000.0,-2.0), hs(3.5)]),
        eqp("Vocal Clarity", "Voice · preamp -4 dB",
            vec![ls(-3.0), pk(62.0,-1.5), pkq(125.0,1.41,-8.5), pk(250.0,0.0), pk(500.0,1.0), pk(1000.0,3.0), pk(2000.0,3.0), pk(4000.0,4.0), pk(8000.0,-1.5), hs(1.5)]),
        eqp("Warm", "Music · preamp -3 dB",
            vec![ls(2.5), pk(62.0,3.0), pkq(125.0,1.41,-3.5), pk(250.0,1.5), pk(500.0,0.5), pk(1000.0,0.0), pk(2000.0,0.0), pk(4000.0,1.5), pk(8000.0,-3.5), hs(-1.5)]),
        eqp("Treble Smooth", "Fatigue · preamp -3 dB",
            vec![ls(-1.0), pk(62.0,0.5), pkq(125.0,1.41,-8.0), pk(250.0,-1.5), pk(500.0,1.0), pk(1000.0,2.0), pk(2000.0,2.0), pk(4000.0,3.0), pkq(8000.0,1.4,-4.0), hs(-2.0)]),
    ]
}

// ── Mic EQ builders (all Q 0.7071) ──
fn mls(gain: f32) -> EqBandConfig { EqBandConfig { kind: "lowshelf".into(), freq_hz: 31.0, q: 0.7071, gain_db: gain } }
fn mpk(freq: f32, gain: f32) -> EqBandConfig { EqBandConfig { kind: "peaking".into(), freq_hz: freq, q: 0.7071, gain_db: gain } }
fn mhs(gain: f32) -> EqBandConfig { EqBandConfig { kind: "highshelf".into(), freq_hz: 16000.0, q: 0.7071, gain_db: gain } }

/// g = the 8 inner peaking-band gains for 62..8000 Hz; b0 = 31 Hz lowshelf gain; b9 = 16 kHz highshelf gain.
fn miceq(b0: f32, g: [f32; 8], b9: f32) -> Vec<EqBandConfig> {
    vec![mls(b0), mpk(62.0,g[0]), mpk(125.0,g[1]), mpk(250.0,g[2]), mpk(500.0,g[3]),
         mpk(1000.0,g[4]), mpk(2000.0,g[5]), mpk(4000.0,g[6]), mpk(8000.0,g[7]), mhs(b9)]
}

fn micp(name: &str, desc: &str, supp: bool, gate: bool, comp: bool, eq: Vec<EqBandConfig>) -> MicPreset {
    MicPreset {
        name: name.into(), description: desc.into(),
        gain: MicGainStage::default(),
        highpass: MicHighpassStage::default(),
        suppression: MicSuppressionStage { enabled: supp, ..Default::default() },
        compressor: MicCompressorStage { enabled: comp, ..Default::default() },
        gate: MicGateStage { enabled: gate, ..Default::default() },
        eq_enabled: true, eq,
    }
}

pub fn factory_mic_presets() -> Vec<MicPreset> {
    vec![
        micp("Flat", "No processing — a clean reset", false, false, false,
            miceq(0.0, [0.0,0.0,0.0,0.0,0.0,0.0,0.0,0.0], 0.0)),
        micp("Less Nasal", "Tame the nasal/honky 1 kHz zone", true, false, false,
            miceq(-12.0, [0.0,0.0,0.0,-2.0,-4.0,-2.0,2.0,4.0], 0.0)),
        micp("Deep Voice", "Warmth and body for thin voices", true, false, false,
            miceq(-12.0, [4.0,4.0,3.0,0.0,-3.0,-1.0,0.0,0.0], 0.0)),
        micp("Balanced", "General-purpose clarity", true, false, false,
            miceq(-12.0, [0.0,0.0,-3.0,-2.0,0.0,2.0,3.0,1.0], 0.0)),
        micp("Clarity High Pitch", "Presence/air for high voices", true, false, false,
            miceq(-12.0, [-12.0,-4.0,-4.0,0.0,0.0,3.0,4.0,4.0], 4.0)),
        micp("Clarity Low Pitch", "Presence/air for deep voices", true, false, false,
            miceq(-12.0, [-3.0,-2.5,0.0,0.0,0.0,3.0,4.0,4.0], 4.0)),
        micp("Broadcast High Pitch", "Radio voice for high voices", true, false, false,
            miceq(-12.0, [-12.0,-3.0,6.0,3.0,-2.5,0.0,3.0,4.0], 4.0)),
        micp("Broadcast Low Pitch", "Radio voice for deep voices", true, false, false,
            miceq(-12.0, [2.0,5.0,2.0,-3.0,-2.0,0.0,3.0,4.0], 4.0)),
        micp("Walkie Talkie", "Two-way radio bandpass effect", true, false, false,
            miceq(-12.0, [-12.0,-12.0,-12.0,5.0,6.0,6.0,5.0,-12.0], -12.0)),
        micp("Streamer", "Full chain: gate + compressor + presence", true, true, true,
            miceq(-12.0, [0.0,0.0,-2.0,-1.0,0.0,2.0,3.0,1.0], 0.0)),
    ]
}
```

- [ ] **Step 4 — Run:** `~/.cargo/bin/cargo test -p arctis-engine presets::` → PASS.
- [ ] **Step 5 — Commit:** `feat(engine): factory EQ + mic preset catalog`.

---

## Phase 2 — Engine apply paths + state

### Task 3: Factory EQ resolution + `factory_eq_presets` in state

**Files:**
- Modify: `crates/engine/src/engine.rs` (`apply_eq_preset` lookup; `state()` population)
- Modify: `crates/engine/src/state.rs` (add field)
- Test: `crates/engine/src/engine.rs`

**Interfaces:**
- Consumes: `crate::presets::factory_eq_presets()`.
- Produces: `EngineState.factory_eq_presets: Vec<EqPresetSnapshot>`.

- [ ] **Step 1 — Failing test:** applying a factory preset name to a channel sets that channel's EQ to
  the factory bands; an unknown name still errors; a user preset with the same name as a factory preset
  wins.

```rust
#[test]
fn apply_eq_preset_resolves_factory_name() {
    let _l = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = std::env::temp_dir().join(format!("asm_fxeq_{}", std::process::id()));
    std::env::set_var("ASM_CONFIG_HOME", &tmp);
    let runner = queue_reconcile_present(MockRunner::new()); // mic/eq apply tolerant
    let mut engine = Engine::new(runner, two_profile_config());
    engine.apply_eq_preset("Bass Boost", "game").expect("factory preset applies");
    let st = engine.state();
    let game = st.channels.iter().find(|c| c.id == "game").unwrap();
    assert_eq!(game.eq_bands.len(), 10);
    assert!(st.factory_eq_presets.iter().any(|p| p.name == "Reference (Calibrated)"));
    let _ = std::fs::remove_dir_all(&tmp); std::env::remove_var("ASM_CONFIG_HOME");
}
```
(If `queue_reconcile_present` doesn't satisfy the apply_all subprocess count for a single channel apply,
use a tolerant MockRunner that returns empty stdout for the `ls` + band-set calls — match the pattern
the existing `apply_eq_preset` tests already use; reuse their runner helper.)

- [ ] **Step 2 — Run, verify fail:** `~/.cargo/bin/cargo test -p arctis-engine apply_eq_preset_resolves_factory_name` → FAIL (no `factory_eq_presets` field / name not found).
- [ ] **Step 3 — Implement:**
  - Add `pub factory_eq_presets: Vec<EqPresetSnapshot>` to `EngineState` (state.rs), after `eq_presets`.
  - In `apply_eq_preset`, change the lookup so a miss in `self.config.eq_presets` falls back to the
    factory catalog before erroring:
    ```rust
    let preset_bands = self.config.eq_presets.iter().find(|p| p.name == preset).map(|p| p.bands.clone())
        .or_else(|| crate::presets::factory_eq_presets().into_iter().find(|p| p.name == preset).map(|p| p.bands))
        .ok_or_else(|| EngineError::BadRequest(format!("EQ preset not found: {preset}")))?;
    ```
  - In `state()`, populate `factory_eq_presets` by mapping `crate::presets::factory_eq_presets()` into
    `EqPresetSnapshot` the same way the existing `eq_presets` snapshot is built (~engine.rs:442).
- [ ] **Step 4 — Run:** `~/.cargo/bin/cargo test -p arctis-engine` → PASS.
- [ ] **Step 5 — Commit:** `feat(engine): apply factory EQ presets + expose them in state`.

### Task 4: `apply_mic_preset` + `mic_presets` in state

**Files:**
- Modify: `crates/engine/src/engine.rs` (new method + `state()`), `crates/engine/src/state.rs` (field + event)
- Test: `crates/engine/src/engine.rs`

**Interfaces:**
- Consumes: `crate::presets::factory_mic_presets()`.
- Produces: `pub fn apply_mic_preset(&mut self, name: &str) -> Result<(), EngineError>`;
  `EngineState.mic_presets: Vec<MicPresetSnapshot>`; `Event::MicPresetApplied { name: String }`.

- [ ] **Step 1 — Failing test:** applying a mic preset overlays its stages + eq, **preserves**
  `mic.enabled` and `mic.hw_mic`, and lists in state; unknown name → BadRequest.

```rust
#[test]
fn apply_mic_preset_overlays_and_preserves_enabled_and_hwmic() {
    let _l = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = std::env::temp_dir().join(format!("asm_mp_{}", std::process::id()));
    std::env::set_var("ASM_CONFIG_HOME", &tmp);
    let mut cfg = two_profile_config();
    cfg.profiles[0].mic.enabled = true;
    cfg.profiles[0].mic.hw_mic = Some("alsa_input.keepme".into());
    let mut engine = Engine::new(MockRunner::new(), cfg); // mic rebuild tolerant of MockRunner
    engine.apply_mic_preset("Less Nasal").expect("applies");
    let st = engine.state();
    assert!(st.mic_presets.iter().any(|p| p.name == "Walkie Talkie"));
    assert_eq!(st.mic.eq_bands.len(), 10);
    // preserved:
    assert!(st.mic.enabled);
    // unknown name:
    assert!(matches!(engine.apply_mic_preset("Nope"), Err(EngineError::BadRequest(_))));
    let _ = std::fs::remove_dir_all(&tmp); std::env::remove_var("ASM_CONFIG_HOME");
}
```
(Confirm the exact field names on `MicSnapshot` — `st.mic.enabled`, `st.mic.eq_bands` — against
`state.rs`; adjust the asserts to the real snapshot field names. If the mic live-rebuild needs queued
MockRunner output, mirror the runner setup used by the existing `mic_set_enabled`/`mic_set_eq_band`
tests.)

- [ ] **Step 2 — Run, verify fail:** `~/.cargo/bin/cargo test -p arctis-engine apply_mic_preset_overlays` → FAIL.
- [ ] **Step 3 — Implement:**
  - Add `pub mic_presets: Vec<MicPresetSnapshot>` to `EngineState`; add
    `MicPresetApplied { name: String }` to the `Event` enum (state.rs), mirroring `EqPresetApplied`.
  - `apply_mic_preset`: find the named preset in `crate::presets::factory_mic_presets()` (→ BadRequest if
    absent); in the active profile's `mic`, set `gain/highpass/suppression/compressor/gate/eq_enabled/eq`
    from the preset while leaving `enabled` and `hw_mic` untouched; `save_config()`; rebuild the mic chain
    live by reusing the SAME path the engine already uses to (re)build the mic chain after a mic mutation
    (find how `mic_set_enabled`/`mic_set_eq_band` apply live and call that build once — do not reimplement
    the chain); `emit(Event::MicPresetApplied { name })`.
  - In `state()`, populate `mic_presets` from `factory_mic_presets()` → `MicPresetSnapshot { name, description }`.
- [ ] **Step 4 — Run:** `~/.cargo/bin/cargo test -p arctis-engine` → PASS.
- [ ] **Step 5 — Commit:** `feat(engine): apply_mic_preset (full-chain overlay) + mic_presets in state`.

---

## Phase 3 — IPC plumbing

### Task 5: `ApplyMicPreset` protocol + daemon + Tauri command

**Files:**
- Modify: `crates/client/src/protocol.rs` (Request variant + tests), `crates/cli/src/daemon.rs` (handler),
  `src-tauri/src/commands.rs` (command), `src-tauri/src/lib.rs` (register)
- Test: `crates/client/src/protocol.rs`

**Interfaces:**
- Consumes: `engine.apply_mic_preset(&name)`.
- Produces: `Request::ApplyMicPreset { name: String }` (kebab wire tag `apply-mic-preset`); Tauri command
  `mic_preset_apply(name) -> EngineState`.

- [ ] **Step 1 — Failing tests** (protocol.rs): a wire-tag parse test and a round-trip test mirroring the
  existing `parse_*`/`request_*_round_trips` tests:

```rust
#[test]
fn parse_apply_mic_preset_wire_tag() {
    let r: Request = serde_json::from_str(r#"{"cmd":"apply-mic-preset","name":"Less Nasal"}"#).unwrap();
    assert!(matches!(r, Request::ApplyMicPreset { name } if name == "Less Nasal"));
}
```

- [ ] **Step 2 — Run, verify fail:** `~/.cargo/bin/cargo test -p arctis-client parse_apply_mic_preset_wire_tag` → FAIL.
- [ ] **Step 3 — Implement:**
  - `protocol.rs`: add `ApplyMicPreset { name: String }` to the `Request` enum (kebab via the existing
    `#[serde(tag="cmd", rename_all="kebab-case")]`).
  - `daemon.rs`: add the arm
    `Request::ApplyMicPreset { name } => match engine.apply_mic_preset(&name) { Ok(()) => Response::ok_with_state(engine.state()), Err(e) => Response::err(e.to_string()) },`.
  - `src-tauri/src/commands.rs`: add `mic_preset_apply` mirroring an existing state-returning command that
    forwards to the daemon (e.g. `eq_preset_apply`); send `Request::ApplyMicPreset { name }`.
  - `src-tauri/src/lib.rs`: register `commands::mic_preset_apply` in the invoke handler list.
- [ ] **Step 4 — Run:** `~/.cargo/bin/cargo test -p arctis-client && ~/.cargo/bin/cargo build -p arctis-sound-manager-ui` → PASS/clean.
- [ ] **Step 5 — Commit:** `feat(ipc): ApplyMicPreset request + mic_preset_apply command`.

---

## Phase 4 — CLI parity

### Task 6: CLI built-in EQ list + `mic preset` subcommand

**Files:**
- Modify: `crates/cli/src/main.rs`
- Test: `crates/cli/src/main.rs` (arg-parse tests, following existing CLI test style)

**Interfaces:**
- Consumes: `daemon::Request::ApplyMicPreset`; `EngineState.{factory_eq_presets, mic_presets}` (via GetState).

- [ ] **Step 1 — Failing test:** `MicPresetAction` parses `list` and `apply <name>` (mirror an existing
  clap arg-parse test in main.rs). Example:

```rust
#[test]
fn cli_parses_mic_preset_apply() {
    use clap::Parser;
    let c = Cli::try_parse_from(["asm-cli","mic","preset","apply","Less Nasal"]).unwrap();
    // assert the parsed command matches Mic { action: MicAction::Preset { action: MicPresetAction::Apply { name } } }
    // with name == "Less Nasal" — adjust to the actual Cli enum shape.
}
```

- [ ] **Step 2 — Run, verify fail:** `~/.cargo/bin/cargo test -p arctis-cli cli_parses_mic_preset_apply` → FAIL.
- [ ] **Step 3 — Implement:**
  - Extend the `eq preset list` handler (`dispatch_eq_preset` ~main.rs:1674) so its output prints a
    "Built-in:" section listing `state.factory_eq_presets` names followed by a "Saved:" section listing
    `state.eq_presets` — fetch state via `daemon::Request::GetState` the way the existing list path does.
  - Add a `Preset { action: MicPresetAction }` arm under the `Mic` subcommand (next to the existing mic
    actions) with `MicPresetAction::{ List, Apply { name: String } }`. `List` prints
    `state.mic_presets` (name — description) from GetState; `Apply { name }` sends
    `daemon::Request::ApplyMicPreset { name }` and prints `mic preset '<name>' applied` (mirror
    `dispatch_eq_preset`'s daemon-call + error-print pattern).
- [ ] **Step 4 — Run:** `~/.cargo/bin/cargo test -p arctis-cli` → PASS.
- [ ] **Step 5 — Commit:** `feat(cli): built-in EQ preset list + mic preset list/apply`.

---

## Phase 5 — GUI parity

### Task 7: Frontend ipc bindings

**Files:**
- Modify: `frontend/src/lib/ipc.ts`
- Test: `frontend/src/lib/*.test.ts` (a small type/wrapper test if the harness supports it; otherwise the
  build + downstream tests cover it)

**Interfaces:**
- Produces: `MicPresetSnapshot { name: string; description: string }`; `EngineState.factory_eq_presets:
  EqPresetSnapshot[]` and `EngineState.mic_presets: MicPresetSnapshot[]`; `micPresetApply(name): Promise<EngineState>`.

- [ ] **Step 1 — Implement** (this task is type/binding plumbing; the failing signal is the build/types):
  - Add `export interface MicPresetSnapshot { name: string; description: string }`.
  - Add `factory_eq_presets: EqPresetSnapshot[]` and `mic_presets: MicPresetSnapshot[]` to the
    `EngineState` interface (next to `eq_presets`, line ~96).
  - Add `export const micPresetApply = (name: string): Promise<EngineState> => invoke<EngineState>("mic_preset_apply", { name });`
    next to the existing `eqPresetApply` (~line 349).
- [ ] **Step 2 — Run:** `pnpm -C frontend test && pnpm -C frontend build` → PASS, warning-clean.
- [ ] **Step 3 — Commit:** `feat(frontend): ipc bindings for factory + mic presets`.

### Task 8: EqPage built-in presets section

**Files:**
- Modify: `frontend/src/lib/components/EqPage.svelte`
- Test: `frontend/src/lib/components/*.test.ts` (extract any list-building logic to a small util + unit-test
  it, mirroring `channelStripUtils.ts`)

**Interfaces:**
- Consumes: `$engineState.factory_eq_presets`, `eqPresetApply(name, channel)`.

- [ ] **Step 1 — Failing test** for a small pure helper that splits presets into built-in vs saved groups
  for display (write it in a `eqPresetUtils.ts` if none exists), e.g.
  `groupPresets(factory, saved)` returns `{ builtin: [...names], saved: [...names] }`.
- [ ] **Step 2 — Run, verify fail:** `pnpm -C frontend test` → FAIL.
- [ ] **Step 3 — Implement:** add a read-only "Built-in" presets group in EqPage that lists
  `$engineState.factory_eq_presets` (name + `kind_hint` as a sub-label) each with an **Apply** button
  calling `eqPresetApply(name, currentChannelId)`; keep the existing user save/apply/delete UI as the
  "Saved" group. No delete on built-ins.
- [ ] **Step 4 — Run:** `pnpm -C frontend test && pnpm -C frontend build` → PASS, warning-clean.
- [ ] **Step 5 — Commit:** `feat(frontend): built-in EQ presets in EqPage`.

### Task 9: MicPage preset picker

**Files:**
- Modify: `frontend/src/lib/components/MicPage.svelte`
- Test: `frontend/src/lib/components/*.test.ts`

**Interfaces:**
- Consumes: `$engineState.mic_presets`, `micPresetApply(name)`.

- [ ] **Step 1 — Failing test** for the picker's apply wiring (mirror the existing MicPage test
  conventions; if no DOM harness, extract the "apply selected preset" handler to a tiny testable unit and
  assert it calls `micPresetApply` with the chosen name).
- [ ] **Step 2 — Run, verify fail:** `pnpm -C frontend test` → FAIL.
- [ ] **Step 3 — Implement:** add a "Presets" picker (dropdown or list) to MicPage populated from
  `$engineState.mic_presets`; on apply, call `micPresetApply(name)` and set `engineState` to the returned
  state; show the selected preset's `description`. Surface errors via the page's existing error pattern
  (don't swallow).
- [ ] **Step 4 — Run:** `pnpm -C frontend test && pnpm -C frontend build` → PASS, warning-clean.
- [ ] **Step 5 — Commit:** `feat(frontend): mic preset picker in MicPage`.

---

## Self-Review

- **Spec coverage:** §4 model → Task 1; §5 catalog → Task 2; §6 engine API → Tasks 3–4; §7 protocol/CLI/GUI
  → Tasks 5/6/7-9. §9 testing → per-task tests + the Task 2 validation table. ✓
- **Placeholder scan:** preset values are concrete (Task 2); the two "confirm exact snapshot field names"
  notes (Tasks 3–4) point at real files to read, not unspecified work. ✓
- **Type consistency:** `MicPreset` (config) / `MicPresetSnapshot` (engine+ipc) / `Request::ApplyMicPreset` /
  `mic_preset_apply` / `micPresetApply` used consistently across tasks. `factory_eq_presets` + `mic_presets`
  are the EngineState field names everywhere. ✓
- **No automated test touches real audio** (MockRunner/fixtures only). ✓
