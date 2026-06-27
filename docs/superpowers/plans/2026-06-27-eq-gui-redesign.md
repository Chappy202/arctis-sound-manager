# EQ GUI Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the parametric-EQ editor reliable (no disappearing dots, no value jumps) and Sonar-like, by fixing the engine to return a dense fixed-10-band model and rebuilding the GUI as an SVG editor with a single source of truth.

**Architecture:** Engine-first. Phase 1 makes the engine always emit exactly 10 bands per channel (and per mic) seeded with canonical default frequencies — no sparse/`1000 Hz`-padded vectors. Phase 2/3 replace the imperative `<canvas>` editor with a controlled SVG component (`EqGraph`) driven by a single `$state` band array, with per-band type + numeric controls and an "ignore engine echoes while editing" reconcile. Protocol is unchanged (`SetEqBand`/`MicEqBand`).

**Tech Stack:** Rust workspace (`arctis-config`, `arctis-audio`, `arctis-engine`); Svelte 5 runes + TypeScript + Vite + Vitest (`frontend/`); Tauri v2 (`src-tauri`).

## Global Constraints

- **Filter kinds are exactly `peaking` | `lowshelf` | `highshelf`** (the engine's `bq_peaking`/`bq_lowshelf`/`bq_highshelf`). No high-pass/low-pass, no per-band bypass, no add/delete bands.
- **Fixed 10 bands** (`MAX_BANDS = 10`, `arctis_audio::MAX_BANDS`). Canonical default centers = `EqModel::default_10band()` = `[31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000]` Hz, all peaking, Q 1.0, gain 0.0.
- **Ranges:** freq 20–20000 Hz (log axis), gain −12..+12 dB, Q 0.3..10 (`FREQ_MIN/FREQ_MAX/GAIN_MIN/GAIN_MAX/Q_MIN/Q_MAX` in `frontend/src/lib/eq.ts`).
- **No protocol changes.** `SetEqBand`/`MicEqBand` stay as-is; CLI/Tauri/ipc signatures unchanged.
- **Rendering: SVG** (`<path>` curve + focusable `<circle role="slider">` handles). No imperative canvas for the editor.
- **Single source of truth for editing bands** in the page; child components are controlled (read-only `bands` prop + `onBandChange` callback). No `$bindable` band copies. **Avoid prop names that collide with the `$state` rune** (don't name a prop `state`).
- **G7:** typed errors, no `unwrap`/`expect` on runtime paths; surface failures.
- **Curve math:** existing RBJ biquad magnitude in `eq.ts` (peaking/lowshelf/highshelf), summed in dB at 48 kHz. No new DSP.
- **Commit after every task** with the exact message shown. Run `cargo test --workspace` (Rust tasks) or `pnpm --dir frontend test` + `pnpm --dir frontend build` (frontend tasks) before committing.

---

## File Structure

**New files:**
- `frontend/src/lib/components/EqGraph.svelte` — controlled SVG editor (grid + curve + 10 focusable handles). Replaces `EqCanvas.svelte`.
- `frontend/src/lib/components/EqBandPanel.svelte` — selected-band detail: type dropdown + numeric freq/gain/Q + reset-band.
- `frontend/src/lib/stores/eqEditing.ts` — `eqEditing` writable + edit-session helpers. Replaces `eqDragging.ts`.

**Modified files:**
- `crates/engine/src/convert.rs` — `default_eq_band_configs()` + `dense_eq_bands()` helpers.
- `crates/engine/src/engine.rs` — `state()` channel + mic `eq_bands` use `dense_eq_bands`; `set_eq_band` seeds dense defaults + band-index bounds guard.
- `frontend/src/lib/eq.ts` — align `DEFAULT_BAND_FREQS` to the engine; add `reconcileBands()`.
- `frontend/src/lib/eq.test.ts` — tests for `reconcileBands`.
- `frontend/src/lib/components/EqPage.svelte` — rewrite to single-source state + EqGraph + EqBandPanel + BandList + Flatten.
- `frontend/src/lib/components/BandList.svelte` — repurpose as a compact selectable 10-row table.
- `frontend/src/lib/components/MicPage.svelte` — swap `EqCanvas` → `EqGraph`.

**Deleted files:**
- `frontend/src/lib/components/EqCanvas.svelte`
- `frontend/src/lib/stores/eqDragging.ts`

---

# PHASE 1 — Engine dense band model (headless)

## Task 1: Dense channel `eq_bands` + `set_eq_band` seeding

**Files:**
- Modify: `crates/engine/src/convert.rs` (add two helpers)
- Modify: `crates/engine/src/engine.rs` (`state()` channel build ~lines 247-258; `set_eq_band` ~lines 500-512)
- Test: inline `#[cfg(test)]` in `crates/engine/src/convert.rs` and `crates/engine/src/engine.rs`

**Interfaces:**
- Produces: `pub fn default_eq_band_configs() -> Vec<arctis_config::EqBandConfig>` (10 canonical defaults) and `pub fn dense_eq_bands(channel: &arctis_config::ChannelConfig) -> Vec<arctis_config::EqBandConfig>` (exactly 10, overrides overlaid) in `convert.rs`.
- Consumes: `arctis_audio::{EqModel, MAX_BANDS}`, `arctis_config::{ChannelConfig, EqBandConfig}`.

- [ ] **Step 1: Write the failing helper tests (convert.rs)**

Add to the `#[cfg(test)] mod tests` in `crates/engine/src/convert.rs`:

```rust
#[test]
fn default_eq_band_configs_is_ten_canonical_flat_bands() {
    let v = default_eq_band_configs();
    assert_eq!(v.len(), 10);
    let freqs: Vec<f32> = v.iter().map(|b| b.freq_hz).collect();
    assert_eq!(
        freqs,
        vec![31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0]
    );
    assert!(v.iter().all(|b| b.kind == "peaking" && b.gain_db == 0.0 && b.q == 1.0));
}

#[test]
fn dense_eq_bands_overlays_overrides_on_defaults() {
    let mut ch = arctis_config::ChannelConfig {
        id: "game".into(), node_name: "Arctis_Game".into(), description: "g".into(),
        output_device: None, eq: vec![], volume_db: 0.0, muted: false,
    };
    // Sparse override: only band index 2 set (a +3 dB highshelf at 300 Hz).
    ch.eq = vec![
        arctis_config::EqBandConfig { kind: "peaking".into(), freq_hz: 31.0, q: 1.0, gain_db: 0.0 },
        arctis_config::EqBandConfig { kind: "peaking".into(), freq_hz: 62.0, q: 1.0, gain_db: 0.0 },
        arctis_config::EqBandConfig { kind: "highshelf".into(), freq_hz: 300.0, q: 1.0, gain_db: 3.0 },
    ];
    let dense = dense_eq_bands(&ch);
    assert_eq!(dense.len(), 10);
    assert_eq!(dense[2].kind, "highshelf");
    assert_eq!(dense[2].freq_hz, 300.0);
    assert_eq!(dense[2].gain_db, 3.0);
    // Untouched slots keep canonical defaults.
    assert_eq!(dense[9].freq_hz, 16000.0);
    assert_eq!(dense[9].gain_db, 0.0);
}

#[test]
fn dense_eq_bands_empty_config_is_ten_defaults() {
    let ch = arctis_config::ChannelConfig {
        id: "chat".into(), node_name: "Arctis_Chat".into(), description: "c".into(),
        output_device: None, eq: vec![], volume_db: 0.0, muted: false,
    };
    assert_eq!(dense_eq_bands(&ch), default_eq_band_configs());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p arctis-engine dense_eq_bands 2>&1 | tail -20`
Expected: FAIL — `default_eq_band_configs`/`dense_eq_bands` not found.

- [ ] **Step 3: Implement the helpers (convert.rs)**

Add near `eq_model_for` in `crates/engine/src/convert.rs` (uses `EqModel::default_10band()` as the single source of the canonical frequencies — note its bands are all `BandKind::Peaking`, so the config-string kind is always `"peaking"`):

```rust
/// The canonical dense default 10-band set, in config form (kind strings).
/// Frequencies come from `EqModel::default_10band()` (single source of truth).
pub fn default_eq_band_configs() -> Vec<arctis_config::EqBandConfig> {
    arctis_audio::EqModel::default_10band()
        .bands
        .iter()
        .map(|b| arctis_config::EqBandConfig {
            kind: "peaking".to_string(),
            freq_hz: b.freq_hz,
            q: b.q,
            gain_db: b.gain_db,
        })
        .collect()
}

/// A dense, fixed-length (10) band vector for a channel: canonical defaults with
/// any stored overrides overlaid by index. Never empty, never `1000 Hz` padding.
pub fn dense_eq_bands(channel: &arctis_config::ChannelConfig) -> Vec<arctis_config::EqBandConfig> {
    let mut dense = default_eq_band_configs();
    for (i, b) in channel.eq.iter().enumerate().take(dense.len()) {
        dense[i] = b.clone();
    }
    dense
}
```

> If `EqModel.bands` is not a public field, use whatever public accessor `arctis_audio::EqModel` exposes for its bands (check `crates/audio/src/eq.rs`); the field is `pub bands: Vec<EqBand>` per the current code.

Also make `eq_model_for` return the **dense 10-band model** so the live filter chain always has 10 bands (otherwise editing a band that a <10-band preset didn't include would target a band that doesn't exist in the live chain). Replace the body of `eq_model_for` with:

```rust
/// Build an `EqModel` for a channel config — ALWAYS the dense 10-band model
/// (canonical defaults overlaid with stored overrides), so the live filter
/// chain has a stable 10 biquads and `set_eq_band` can always target any band.
pub fn eq_model_for(channel: &ChannelConfig) -> Result<EqModel, EngineError> {
    let bands = dense_eq_bands(channel)
        .iter()
        .map(eq_band_from_cfg)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(EqModel { bands })
}
```

> This changes `eq_model_for` for a *non-empty but partial* config from "map exactly the stored bands" to "10 dense bands." Update the existing test `eq_model_for` non-empty assertion accordingly: a config with 1 override band now yields an `EqModel` of **10** bands (the override at its index + canonical defaults elsewhere), not 1. The `eq_model_for_empty_eq_gives_default_10band` test still passes (dense of empty == `default_10band()`).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p arctis-engine dense_eq_bands 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 5: Use `dense_eq_bands` in `state()`**

In `crates/engine/src/engine.rs` `state()`, replace the channel `eq_bands` build (currently `ch.eq.iter().map(...)`) with the dense version:

```rust
                        eq_bands: convert::dense_eq_bands(ch)
                            .iter()
                            .map(|b| EqBandSnapshot {
                                kind: b.kind.clone(),
                                freq_hz: b.freq_hz,
                                q: b.q,
                                gain_db: b.gain_db,
                            })
                            .collect(),
```

- [ ] **Step 6: Write the failing `state()` + `set_eq_band` tests (engine.rs)**

Add to the engine test module (mirror existing test helpers — `make_config_no_eq_no_routes()`, `arctis_audio::MockRunner`, `Engine::new`):

```rust
#[test]
fn state_returns_ten_dense_bands_for_flat_channel() {
    let engine = Engine::new(arctis_audio::MockRunner::new(), make_config_no_eq_no_routes());
    let st = engine.state();
    let game = st.channels.iter().find(|c| c.id == "game").unwrap();
    assert_eq!(game.eq_bands.len(), 10, "flat channel must report 10 dense bands");
    assert_eq!(game.eq_bands[0].freq_hz, 31.0);
    assert_eq!(game.eq_bands[9].freq_hz, 16000.0);
    assert!(game.eq_bands.iter().all(|b| b.gain_db == 0.0 && b.kind == "peaking"));
}

#[test]
fn set_eq_band_seeds_dense_defaults_no_1000hz_padding() {
    // Editing band index 3 first must NOT create 1000 Hz placeholders at 0..2.
    let cfg = make_config_no_eq_no_routes();
    // MockRunner: set_eq_band persists then applies one band live (find_node_id + props).
    // Queue generous empty successes so apply_band never starves.
    let runner = arctis_audio::MockRunner::new()
        .with_output(0, "", "").with_output(0, "", "")
        .with_output(0, "", "").with_output(0, "", "");
    let mut engine = Engine::new(runner, cfg);
    let band = arctis_config::EqBandConfig {
        kind: "peaking".into(), freq_hz: 250.0, q: 1.4, gain_db: 3.0,
    };
    engine.set_eq_band("game", 3, band).unwrap();
    let st = engine.state();
    let game = st.channels.iter().find(|c| c.id == "game").unwrap();
    assert_eq!(game.eq_bands.len(), 10);
    // Band 3 is the edit; bands 0-2 are canonical defaults (NOT 1000 Hz).
    assert_eq!(game.eq_bands[3].freq_hz, 250.0);
    assert_eq!(game.eq_bands[3].gain_db, 3.0);
    assert_eq!(game.eq_bands[0].freq_hz, 31.0, "band 0 must be canonical default, not 1000 Hz");
    assert_eq!(game.eq_bands[1].freq_hz, 62.0);
    assert_eq!(game.eq_bands[2].freq_hz, 125.0);
}

#[test]
fn set_eq_band_rejects_out_of_range_index() {
    let mut engine = Engine::new(arctis_audio::MockRunner::new(), make_config_no_eq_no_routes());
    let band = arctis_config::EqBandConfig { kind: "peaking".into(), freq_hz: 1000.0, q: 1.0, gain_db: 0.0 };
    assert!(engine.set_eq_band("game", 10, band).is_err());
}
```

> If `make_config_no_eq_no_routes` builds channels WITHOUT `aux`, the assertions on `game` still hold. Match the exact MockRunner call sequence `apply_band` needs (run the test; if it starves, add `.with_output(0,"","")` entries and note the observed sequence in the report).

- [ ] **Step 7: Run to verify failure**

Run: `cargo test -p arctis-engine 'state_returns_ten_dense\|set_eq_band_seeds\|set_eq_band_rejects' 2>&1 | tail -25`
Expected: `state_returns_ten_dense...` PASSES (Step 5 already done); the two `set_eq_band_*` FAIL (padding still 1000 Hz; no bounds guard).

- [ ] **Step 8: Fix `set_eq_band` seeding + bounds guard**

In `crates/engine/src/engine.rs` `set_eq_band`, replace the `while channel.eq.len() <= band { push 1000Hz }` block with a bounds guard + dense seeding:

```rust
            if band >= arctis_audio::MAX_BANDS {
                return Err(EngineError::BadRequest(format!(
                    "band index {band} out of range (0..{})",
                    arctis_audio::MAX_BANDS
                )));
            }
            // Seed the dense canonical defaults (correct freqs, NOT 1000 Hz)
            // while preserving any existing overrides, so unedited lower bands
            // keep their real default frequencies.
            if channel.eq.len() < arctis_audio::MAX_BANDS {
                channel.eq = convert::dense_eq_bands(channel);
            }
            channel.eq[band] = cfg.clone();
```

> Place the bounds guard BEFORE mutating. `convert::dense_eq_bands(channel)` reads the current (sparse) `channel.eq` and returns the dense overlay; assigning it back makes the config dense. Then index `band` is always < 10 and exists.

- [ ] **Step 9: Run to verify pass + full engine suite**

Run: `cargo test -p arctis-engine 2>&1 | tail -15`
Expected: PASS (new + existing). Fix any existing test that asserted empty/short `eq_bands`.

- [ ] **Step 10: Commit**

```bash
git add crates/engine/src/convert.rs crates/engine/src/engine.rs
git commit -m "feat(engine): dense fixed-10 channel eq_bands + seed correct default freqs (no 1000Hz padding)"
```

---

## Task 2: Dense mic `eq_bands` in `state()`

**Files:**
- Modify: `crates/engine/src/engine.rs` (the mic snapshot build in `state()` — search for where `MicSnapshot { ... eq_bands: ... }` is constructed)
- Test: inline `#[cfg(test)]` in `crates/engine/src/engine.rs`

**Interfaces:**
- Consumes: `convert::dense_eq_bands` (Task 1). NOTE the mic EQ stores its bands on the mic chain config, not a `ChannelConfig`. If the mic eq band vector is a `Vec<EqBandConfig>` on the mic config, reuse the same overlay logic; if it's a different type, write a small local dense overlay mirroring `dense_eq_bands` (defaults from `default_eq_band_configs()`, overlay by index, length 10).

- [ ] **Step 1: Locate the mic `eq_bands` build**

Run: `grep -n "MicSnapshot\|eq_bands" crates/engine/src/engine.rs | head` and read the surrounding `state()` code that builds the `mic` value (the `MicSnapshot { ... eq_bands: ... }`). Identify the source vector (the mic chain config's EQ stage bands).

- [ ] **Step 2: Write the failing test**

```rust
#[test]
fn state_returns_ten_dense_mic_eq_bands() {
    let engine = Engine::new(arctis_audio::MockRunner::new(), make_config_no_eq_no_routes());
    let st = engine.state();
    assert_eq!(st.mic.eq_bands.len(), 10, "mic EQ must report 10 dense bands");
    assert_eq!(st.mic.eq_bands[0].freq_hz, 31.0);
    assert_eq!(st.mic.eq_bands[9].freq_hz, 16000.0);
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p arctis-engine state_returns_ten_dense_mic 2>&1 | tail -15`
Expected: FAIL (mic eq_bands is currently the raw/sparse vector — length ≠ 10).

- [ ] **Step 4: Make the mic eq build dense**

In the mic snapshot construction in `state()`, build `eq_bands` densely. If the mic EQ bands are stored as `Vec<EqBandConfig>` (call it `mic_eq_cfg`), produce the dense overlay the same way:

```rust
        // Dense 10-band mic EQ: canonical defaults overlaid with stored mic bands.
        let mic_eq_dense: Vec<EqBandSnapshot> = {
            let mut dense = convert::default_eq_band_configs();
            for (i, b) in mic_eq_cfg.iter().enumerate().take(dense.len()) {
                dense[i] = b.clone();
            }
            dense.iter().map(|b| EqBandSnapshot {
                kind: b.kind.clone(), freq_hz: b.freq_hz, q: b.q, gain_db: b.gain_db,
            }).collect()
        };
```

Use `mic_eq_dense` for the `MicSnapshot.eq_bands` field. Adapt the source-vector name (`mic_eq_cfg`) to whatever the real mic config exposes. If the mic EQ band element type differs from `EqBandConfig`, map it to `EqBandConfig` first (kind string + freq/q/gain).

> If the mic EQ already reports a fixed 10 (some chains seed defaults at apply time), this task is a verify — the test should pass once you confirm and, if needed, route through the dense overlay so it's guaranteed.

- [ ] **Step 5: Run to verify pass + suite**

Run: `cargo test -p arctis-engine 2>&1 | tail -12`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/engine/src/engine.rs
git commit -m "feat(engine): dense fixed-10 mic eq_bands in state()"
```

---

## Task 3: Phase 1 workspace gate

- [ ] **Step 1: Full build + test**

Run: `cargo test --workspace 2>&1 | grep -E "test result: FAILED|^error|FAILED" || echo WORKSPACE_CLEAN`
Expected: `WORKSPACE_CLEAN`. Fix any CLI test asserting empty/short `eq_bands` (e.g. `eq show` expectations) to expect 10 dense bands.

- [ ] **Step 2: Commit (only if fixes were needed)**

```bash
git add -A && git commit -m "test: green workspace after dense EQ band model"
```

---

# PHASE 2 — GUI core (SVG editor + single source of truth)

## Task 4: `eq.ts` — align defaults + `reconcileBands` reducer

**Files:**
- Modify: `frontend/src/lib/eq.ts`
- Test: `frontend/src/lib/eq.test.ts`

**Interfaces:**
- Produces: `export function reconcileBands(local: Band[], incoming: Band[], editing: boolean): Band[]`.
- Consumes: `Band` type (existing).

- [ ] **Step 1: Write failing tests (eq.test.ts)**

Add to `frontend/src/lib/eq.test.ts` (import `reconcileBands` in the existing import block):

```ts
describe("reconcileBands", () => {
  const mk = (gainDb: number): Band => ({ kind: "peaking", freqHz: 1000, q: 1, gainDb });
  it("keeps local bands while editing", () => {
    const local = [mk(3), mk(0)];
    const incoming = [mk(-6), mk(0)];
    expect(reconcileBands(local, incoming, true)).toBe(local);
  });
  it("adopts incoming bands when idle", () => {
    const local = [mk(3), mk(0)];
    const incoming = [mk(-6), mk(0)];
    expect(reconcileBands(local, incoming, false)).toEqual(incoming);
  });
  it("never returns a shorter array than incoming when idle", () => {
    const local = [mk(0)];
    const incoming = [mk(1), mk(2), mk(3)];
    expect(reconcileBands(local, incoming, false).length).toBe(3);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm --dir frontend test eq.test 2>&1 | tail -20`
Expected: FAIL — `reconcileBands` not exported.

- [ ] **Step 3: Implement + align default freqs**

In `frontend/src/lib/eq.ts`:

(a) Change `DEFAULT_BAND_FREQS` to match the engine's `default_10band()`:

```ts
export const DEFAULT_BAND_FREQS: readonly number[] = [
  31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000,
];
```

(b) Add the reducer at the end of the file:

```ts
/**
 * Reconcile the locally-edited band array against an incoming engine snapshot.
 * While the user is editing (any modality), keep the local array unchanged so a
 * background state refresh can never clobber an in-progress edit. When idle,
 * adopt the incoming array. Pure — unit-testable.
 */
export function reconcileBands(local: Band[], incoming: Band[], editing: boolean): Band[] {
  if (editing) return local;
  return incoming;
}
```

- [ ] **Step 4: Run to verify pass**

Run: `pnpm --dir frontend test eq.test 2>&1 | tail -15`
Expected: PASS (existing + 3 new). Then `pnpm --dir frontend build 2>&1 | tail -3` (no type errors).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/eq.ts frontend/src/lib/eq.test.ts
git commit -m "feat(frontend): align EQ default freqs to engine + reconcileBands reducer"
```

---

## Task 5: `eqEditing` store (replaces `eqDragging`)

**Files:**
- Create: `frontend/src/lib/stores/eqEditing.ts`

**Interfaces:**
- Produces: `eqEditing: Writable<boolean>`, `beginEditing(): void`, `endEditing(settleMs?: number): void`, `pulseEditing(settleMs?: number): void`.

- [ ] **Step 1: Create the store**

`frontend/src/lib/stores/eqEditing.ts`:

```ts
/**
 * eqEditing.ts — Tracks whether an EQ edit is in progress, across ALL input
 * modalities (pointer drag, scroll-Q, keyboard, numeric-field focus). The EQ
 * page reads this before applying a background state refresh, so no refresh
 * clobbers an in-progress edit. Replaces the old pointer-only `eqDragging`.
 */
import { writable } from "svelte/store";

export const eqEditing = writable(false);

let settleTimer: ReturnType<typeof setTimeout> | null = null;

/** Begin a held edit session (e.g. pointerdown, numeric-field focus). */
export function beginEditing(): void {
  if (settleTimer) { clearTimeout(settleTimer); settleTimer = null; }
  eqEditing.set(true);
}

/** End a held edit session after a short settle window (e.g. pointerup, blur). */
export function endEditing(settleMs = 300): void {
  if (settleTimer) clearTimeout(settleTimer);
  settleTimer = setTimeout(() => { eqEditing.set(false); settleTimer = null; }, settleMs);
}

/** One discrete edit (wheel, key, numeric commit): hold then auto-release. */
export function pulseEditing(settleMs = 300): void {
  beginEditing();
  endEditing(settleMs);
}
```

- [ ] **Step 2: Verify it typechecks**

Run: `pnpm --dir frontend build 2>&1 | tail -5`
Expected: builds (the file is unused so far; this just confirms no syntax/type error).

- [ ] **Step 3: Commit**

```bash
git add frontend/src/lib/stores/eqEditing.ts
git commit -m "feat(frontend): eqEditing store (all-modality edit guard)"
```

---

## Task 6: `EqGraph.svelte` — SVG editor (controlled)

**Files:**
- Create: `frontend/src/lib/components/EqGraph.svelte`

**Interfaces:**
- Produces a component with props: `{ bands: Band[]; selectedIndex: number; onBandChange: (index: number, band: Band) => void; onSelect: (index: number) => void; onFlush?: (index: number, band: Band) => void; channelId?: string }`. Controlled (no internal band state). Calls `setEqBand(channelId, ...)` when `onFlush` is absent.
- Consumes: `eq.ts` (`freqToX`,`gainToY`,`xToFreq`,`yToGain`,`logFreqAxis`,`summedCurveDb`,`clampBand`,`FREQ_MIN`,`FREQ_MAX`,`GAIN_MIN`,`GAIN_MAX`,`type Band`); `ipc.ts` (`setEqBand`); `stores/eqEditing.ts`.

- [ ] **Step 1: Create the component**

`frontend/src/lib/components/EqGraph.svelte`:

```svelte
<script lang="ts">
  /**
   * EqGraph.svelte — Controlled SVG parametric-EQ editor.
   * Renders a log-frequency grid, the summed response curve, and one focusable
   * <circle role="slider"> handle per band. Holds NO band state — the parent
   * owns the single source of truth and receives changes via onBandChange.
   */
  import {
    freqToX, gainToY, xToFreq, yToGain, logFreqAxis, summedCurveDb, clampBand,
    FREQ_MIN, FREQ_MAX, GAIN_MIN, GAIN_MAX, type Band,
  } from "../eq.js";
  import { setEqBand } from "../ipc.js";
  import { beginEditing, endEditing, pulseEditing } from "../stores/eqEditing.js";

  interface Props {
    bands: Band[];
    selectedIndex: number;
    onBandChange: (index: number, band: Band) => void;
    onSelect: (index: number) => void;
    onFlush?: (index: number, band: Band) => void;
    channelId?: string;
  }
  let { bands, selectedIndex, onBandChange, onSelect, onFlush, channelId = "" }: Props = $props();

  // Internal viewBox coordinate space (CSS scales the SVG to fit).
  const VW = 1000;
  const VH = 360;

  const BAND_COLORS = [
    "#FF5200", "#0091D1", "#41A930", "#754BD3", "#FFBE00",
    "#2A7199", "#B24736", "#356E74", "#6F3969", "#50648C",
  ];
  const FREQ_LABELS = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
  const GAIN_LABELS = [-12, -6, 0, 6, 12];
  const CURVE_SAMPLES = 240;
  const freqAxis = logFreqAxis(CURVE_SAMPLES);

  // Throttled flush during drag.
  let throttleTimer: ReturnType<typeof setTimeout> | null = null;
  const THROTTLE_MS = 50;

  function flush(index: number, band: Band) {
    if (onFlush) { onFlush(index, band); return; }
    setEqBand(channelId, index, band.kind, band.freqHz, band.q, band.gainDb)
      .catch((e) => console.warn("[EqGraph] setEqBand failed:", e));
  }
  function throttledFlush(index: number, band: Band) {
    if (throttleTimer !== null) return;
    throttleTimer = setTimeout(() => { throttleTimer = null; flush(index, bands[index]); }, THROTTLE_MS);
  }

  // ── Derived geometry ──────────────────────────────────────────────────────
  const curvePath = $derived.by(() => {
    const dbs = summedCurveDb(bands, freqAxis);
    let d = "";
    for (let i = 0; i < freqAxis.length; i++) {
      const x = freqToX(freqAxis[i], VW);
      const y = gainToY(dbs[i], VH);
      d += (i === 0 ? "M" : "L") + x.toFixed(2) + " " + y.toFixed(2) + " ";
    }
    return d.trim();
  });
  const fillPath = $derived(
    `${curvePath} L ${freqToX(FREQ_MAX, VW).toFixed(2)} ${gainToY(0, VH).toFixed(2)} ` +
    `L ${freqToX(FREQ_MIN, VW).toFixed(2)} ${gainToY(0, VH).toFixed(2)} Z`
  );

  function handleX(b: Band) { return freqToX(b.freqHz, VW); }
  function handleY(b: Band) { return gainToY(b.gainDb, VH); }

  // ── Pointer drag (per-handle) ─────────────────────────────────────────────
  let dragIndex = -1;
  let svgEl: SVGSVGElement | undefined = $state();

  function toViewBox(e: PointerEvent): [number, number] {
    if (!svgEl) return [0, 0];
    const r = svgEl.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width) * VW, ((e.clientY - r.top) / r.height) * VH];
  }

  function onHandleDown(e: PointerEvent, i: number) {
    e.preventDefault();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    dragIndex = i;
    beginEditing();
    if (selectedIndex !== i) onSelect(i);
  }
  function onHandleMove(e: PointerEvent, i: number) {
    if (dragIndex !== i) return;
    e.preventDefault();
    const [vx, vy] = toViewBox(e);
    const next = clampBand({ ...bands[i], freqHz: xToFreq(vx, VW), gainDb: yToGain(vy, VH) });
    onBandChange(i, next);
    throttledFlush(i, next);
  }
  function onHandleUp(e: PointerEvent, i: number) {
    if (dragIndex !== i) return;
    e.preventDefault();
    if (throttleTimer !== null) { clearTimeout(throttleTimer); throttleTimer = null; }
    flush(i, bands[i]);
    dragIndex = -1;
    endEditing();
  }

  // ── Scroll = Q ────────────────────────────────────────────────────────────
  function onHandleWheel(e: WheelEvent, i: number) {
    e.preventDefault();
    const factor = e.deltaY > 0 ? 0.85 : 1.18;
    const next = clampBand({ ...bands[i], q: bands[i].q * factor });
    onSelect(i);
    onBandChange(i, next);
    flush(i, next);
    pulseEditing();
  }

  // ── Keyboard (APG slider) ─────────────────────────────────────────────────
  function onHandleKey(e: KeyboardEvent, i: number) {
    const b = bands[i];
    const coarse = e.shiftKey;
    const gainStep = coarse ? 1 : 0.25;
    const qStep = coarse ? 0.5 : 0.1;
    let next: Band | null = null;
    switch (e.key) {
      case "ArrowUp":
        next = e.altKey ? { ...b, q: b.q + qStep } : { ...b, gainDb: b.gainDb + gainStep }; break;
      case "ArrowDown":
        next = e.altKey ? { ...b, q: b.q - qStep } : { ...b, gainDb: b.gainDb - gainStep }; break;
      case "ArrowLeft": {
        const x = freqToX(b.freqHz, VW); next = { ...b, freqHz: xToFreq(x - VW * (coarse ? 0.05 : 0.02), VW) }; break;
      }
      case "ArrowRight": {
        const x = freqToX(b.freqHz, VW); next = { ...b, freqHz: xToFreq(x + VW * (coarse ? 0.05 : 0.02), VW) }; break;
      }
      case "Home": next = { ...b, gainDb: GAIN_MIN }; break;
      case "End": next = { ...b, gainDb: GAIN_MAX }; break;
      default: return;
    }
    e.preventDefault();
    const clamped = clampBand(next);
    onSelect(i);
    onBandChange(i, clamped);
    flush(i, clamped);
    pulseEditing();
  }

  function onHandleDblClick(i: number) {
    const next = clampBand({ ...bands[i], gainDb: 0 });
    onBandChange(i, next);
    flush(i, next);
    pulseEditing();
  }

  function fmt(b: Band) {
    const f = b.freqHz >= 1000 ? `${(b.freqHz / 1000).toFixed(1)}k` : `${Math.round(b.freqHz)}`;
    return `Band ${"?"}, ${b.kind}, ${f} Hz, ${b.gainDb >= 0 ? "+" : ""}${b.gainDb.toFixed(1)} dB, Q ${b.q.toFixed(2)}`;
  }
</script>

<div class="eq-graph-wrap">
  <svg bind:this={svgEl} viewBox="0 0 {VW} {VH}" preserveAspectRatio="none"
       class="eq-graph" role="group" aria-label="Parametric EQ frequency response editor">
    <!-- grid -->
    {#each FREQ_LABELS as f}
      <line x1={freqToX(f, VW)} y1="0" x2={freqToX(f, VW)} y2={VH} class="grid" />
      <text x={freqToX(f, VW)} y={VH - 4} class="axis-x">{f >= 1000 ? `${f / 1000}k` : f}</text>
    {/each}
    {#each GAIN_LABELS as g}
      <line x1="0" y1={gainToY(g, VH)} x2={VW} y2={gainToY(g, VH)} class:zero={g === 0} class="grid" />
      {#if g !== 0}<text x="6" y={gainToY(g, VH) - 3} class="axis-y">{g > 0 ? `+${g}` : g}</text>{/if}
    {/each}

    <!-- curve -->
    <path d={fillPath} class="curve-fill" />
    <path d={curvePath} class="curve-line" />

    <!-- handles -->
    {#each bands as b, i (i)}
      <g class="handle" class:selected={i === selectedIndex}>
        <!-- larger transparent hit/focus target -->
        <circle cx={handleX(b)} cy={handleY(b)} r="18" class="hit"
          role="slider" tabindex="0"
          aria-label={fmt(b).replace("Band ?", `Band ${i + 1}`)}
          aria-valuemin={GAIN_MIN} aria-valuemax={GAIN_MAX} aria-valuenow={b.gainDb}
          aria-valuetext={fmt(b).replace("Band ?", `Band ${i + 1}`)}
          onpointerdown={(e) => onHandleDown(e, i)}
          onpointermove={(e) => onHandleMove(e, i)}
          onpointerup={(e) => onHandleUp(e, i)}
          onwheel={(e) => onHandleWheel(e, i)}
          onkeydown={(e) => onHandleKey(e, i)}
          ondblclick={() => onHandleDblClick(i)}
          onfocus={() => onSelect(i)}
        />
        <circle cx={handleX(b)} cy={handleY(b)} r={i === selectedIndex ? 9 : 6}
          fill={BAND_COLORS[i % BAND_COLORS.length]} class="dot" />
        <text x={handleX(b)} y={handleY(b) + 3} class="dot-num">{i + 1}</text>
      </g>
    {/each}
  </svg>
</div>

<style>
  .eq-graph-wrap { width: 100%; height: 100%; background: var(--ss-surface-2); border-radius: var(--ss-radius-md); overflow: hidden; }
  .eq-graph { display: block; width: 100%; height: 100%; touch-action: none; }
  .grid { stroke: rgba(255,255,255,0.06); stroke-width: 1; }
  .grid.zero { stroke: rgba(255,255,255,0.18); stroke-width: 1.5; }
  .axis-x { fill: rgba(122,124,128,0.9); font: 10px var(--ss-font-mono, monospace); text-anchor: middle; }
  .axis-y { fill: rgba(122,124,128,0.6); font: 10px var(--ss-font-mono, monospace); }
  .curve-line { fill: none; stroke: var(--ss-accent, #FF5200); stroke-width: 2.5; stroke-linejoin: round; vector-effect: non-scaling-stroke; }
  .curve-fill { fill: rgba(255,82,0,0.10); stroke: none; }
  .hit { fill: transparent; cursor: grab; }
  .hit:active { cursor: grabbing; }
  .hit:focus-visible { outline: none; }
  .handle:focus-within .dot, .handle.selected .dot { stroke: #fff; stroke-width: 2; }
  .dot { stroke: rgba(255,255,255,0.6); stroke-width: 1.5; }
  .dot-num { fill: #fff; font: bold 9px var(--ss-font-mono, monospace); text-anchor: middle; pointer-events: none; }
</style>
```

- [ ] **Step 2: Verify build/typecheck**

Run: `pnpm --dir frontend build 2>&1 | tail -15`
Expected: builds (component unused so far). Fix any type errors against the real `eq.ts`/`ipc.ts` exports.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/lib/components/EqGraph.svelte
git commit -m "feat(frontend): SVG EqGraph editor (controlled, focusable handles)"
```

---

## Task 7: `EqBandPanel.svelte` — type + numeric controls

**Files:**
- Create: `frontend/src/lib/components/EqBandPanel.svelte`

**Interfaces:**
- Produces a component with props: `{ band: Band | null; index: number; onBandChange: (index: number, band: Band) => void; onFlush: (index: number, band: Band) => void }`. Edits route through `onBandChange` + `onFlush`.
- Consumes: `eq.ts` (`clampBand`, `FREQ_MIN`,`FREQ_MAX`,`GAIN_MIN`,`GAIN_MAX`,`Q_MIN`,`Q_MAX`,`type Band`); `stores/eqEditing.ts` (`beginEditing`,`endEditing`).

- [ ] **Step 1: Create the component**

`frontend/src/lib/components/EqBandPanel.svelte`:

```svelte
<script lang="ts">
  import { clampBand, FREQ_MIN, FREQ_MAX, GAIN_MIN, GAIN_MAX, Q_MIN, Q_MAX, type Band } from "../eq.js";
  import { beginEditing, endEditing } from "../stores/eqEditing.js";

  interface Props {
    band: Band | null;
    index: number;
    onBandChange: (index: number, band: Band) => void;
    onFlush: (index: number, band: Band) => void;
  }
  let { band, index, onBandChange, onFlush }: Props = $props();

  const KINDS: Band["kind"][] = ["peaking", "lowshelf", "highshelf"];
  const KIND_LABELS: Record<Band["kind"], string> = {
    peaking: "Peaking", lowshelf: "Low shelf", highshelf: "High shelf",
  };

  function commit(patch: Partial<Band>) {
    if (!band) return;
    const next = clampBand({ ...band, ...patch });
    onBandChange(index, next);
    onFlush(index, next);
  }
  function onNum(field: "freqHz" | "gainDb" | "q", e: Event) {
    const v = Number((e.target as HTMLInputElement).value);
    if (Number.isNaN(v)) return; // reject; field reverts on blur via reactive value
    commit({ [field]: v } as Partial<Band>);
  }
  function resetBand() {
    if (!band) return;
    commit({ kind: "peaking", q: 1, gainDb: 0 });
  }
</script>

<div class="band-panel">
  {#if band}
    <div class="panel-head">SELECTED BAND <span class="b-num">{index + 1}</span></div>
    <label class="field">
      <span>Type</span>
      <select value={band.kind} onchange={(e) => commit({ kind: (e.target as HTMLSelectElement).value as Band["kind"] })}>
        {#each KINDS as k}<option value={k}>{KIND_LABELS[k]}</option>{/each}
      </select>
    </label>
    <label class="field">
      <span>Freq (Hz)</span>
      <input type="number" min={FREQ_MIN} max={FREQ_MAX} step="1" value={Math.round(band.freqHz)}
        onfocus={beginEditing} onblur={() => endEditing()} oninput={(e) => onNum("freqHz", e)} />
    </label>
    <label class="field">
      <span>Gain (dB)</span>
      <input type="number" min={GAIN_MIN} max={GAIN_MAX} step="0.5" value={band.gainDb}
        onfocus={beginEditing} onblur={() => endEditing()} oninput={(e) => onNum("gainDb", e)} />
    </label>
    <label class="field">
      <span>Q</span>
      <input type="number" min={Q_MIN} max={Q_MAX} step="0.1" value={band.q}
        onfocus={beginEditing} onblur={() => endEditing()} oninput={(e) => onNum("q", e)} />
    </label>
    <button class="reset-btn" onclick={resetBand}>Reset band</button>
  {:else}
    <p class="empty">Select a band to edit its values.</p>
  {/if}
</div>

<style>
  .band-panel { display: flex; flex-direction: column; gap: var(--ss-space-2); }
  .panel-head { font-family: var(--ss-font-display); text-transform: uppercase; font-size: var(--ss-type-h2-size); color: var(--ss-text-primary); }
  .b-num { color: var(--ss-accent); }
  .field { display: grid; grid-template-columns: 84px 1fr; align-items: center; gap: var(--ss-space-2); font-size: var(--ss-type-caption-size); color: var(--ss-text-secondary); }
  .field select, .field input { height: var(--ss-control-h-sm); background: var(--ss-surface-input); border: 1px solid var(--ss-border-strong); border-radius: var(--ss-radius-xs); color: var(--ss-text-primary); padding: 0 var(--ss-space-2); font-family: var(--ss-font-mono); }
  .field input:focus, .field select:focus { outline: none; border-color: var(--ss-accent-border); }
  .reset-btn { align-self: flex-start; height: 24px; padding: 0 var(--ss-space-3); background: transparent; border: 1px solid var(--ss-border-strong); border-radius: var(--ss-radius-xs); color: var(--ss-text-tertiary); cursor: pointer; }
  .reset-btn:hover { color: var(--ss-accent); border-color: var(--ss-accent-border); background: var(--ss-accent-soft); }
  .empty { color: var(--ss-text-tertiary); font-style: italic; font-size: var(--ss-type-caption-size); }
</style>
```

- [ ] **Step 2: Verify build**

Run: `pnpm --dir frontend build 2>&1 | tail -10`
Expected: builds.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/lib/components/EqBandPanel.svelte
git commit -m "feat(frontend): EqBandPanel (type selector + numeric freq/gain/Q + reset)"
```

---

## Task 8: `BandList.svelte` — compact selectable table

**Files:**
- Modify: `frontend/src/lib/components/BandList.svelte` (replace its body)

**Interfaces:**
- Produces a component with props: `{ bands: Band[]; selectedIndex: number; onSelectBand: (index: number) => void }` (read-only display + selection). Remove any band-editing/IPC logic from this component — editing now lives in `EqBandPanel` and `EqGraph`.
- Consumes: `eq.ts` (`type Band`).

- [ ] **Step 1: Replace the component body**

Read the current `BandList.svelte` to preserve its `<style>` conventions, then replace its script + markup with a read-only selectable table:

```svelte
<script lang="ts">
  import { type Band } from "../eq.js";
  interface Props {
    bands: Band[];
    selectedIndex: number;
    onSelectBand: (index: number) => void;
  }
  let { bands, selectedIndex, onSelectBand }: Props = $props();
  const KIND_SHORT: Record<Band["kind"], string> = { peaking: "PK", lowshelf: "LS", highshelf: "HS" };
  function fmtFreq(f: number) { return f >= 1000 ? `${(f / 1000).toFixed(f >= 10000 ? 1 : 2)}k` : `${Math.round(f)}`; }
</script>

<ul class="band-list" role="listbox" aria-label="EQ bands">
  {#each bands as b, i (i)}
    <li>
      <button class="band-row" class:selected={i === selectedIndex}
        role="option" aria-selected={i === selectedIndex} onclick={() => onSelectBand(i)}>
        <span class="bn">{i + 1}</span>
        <span class="bk">{KIND_SHORT[b.kind]}</span>
        <span class="bf">{fmtFreq(b.freqHz)}Hz</span>
        <span class="bg">{b.gainDb >= 0 ? "+" : ""}{b.gainDb.toFixed(1)}dB</span>
        <span class="bq">Q{b.q.toFixed(1)}</span>
      </button>
    </li>
  {/each}
</ul>

<style>
  .band-list { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(2, 1fr); gap: 2px; }
  .band-row { display: flex; align-items: center; gap: var(--ss-space-2); width: 100%; padding: var(--ss-space-1) var(--ss-space-2); background: var(--ss-surface-2); border: 1px solid transparent; border-radius: var(--ss-radius-xs); cursor: pointer; font-family: var(--ss-font-mono); font-size: var(--ss-type-caption-size); color: var(--ss-text-secondary); }
  .band-row:hover { background: var(--ss-surface-3); }
  .band-row.selected { border-color: var(--ss-accent-border); color: var(--ss-text-primary); }
  .bn { color: var(--ss-accent); width: 16px; }
  .bf { margin-left: auto; }
</style>
```

- [ ] **Step 2: Verify build**

Run: `pnpm --dir frontend build 2>&1 | tail -10`
Expected: builds (callers updated in Task 9; if a stale caller passes removed props, that's fixed there).

> If `BandList.svelte` has a co-located test asserting the old editing API, update it to the new read-only props or remove the obsolete assertions (don't gut meaningful coverage — selection behavior can be asserted).

- [ ] **Step 3: Commit**

```bash
git add frontend/src/lib/components/BandList.svelte
git commit -m "feat(frontend): BandList as compact selectable read-only table"
```

---

## Task 9: `EqPage.svelte` — single-source rewrite

**Files:**
- Modify: `frontend/src/lib/components/EqPage.svelte`

**Interfaces:**
- Consumes: `EqGraph`, `EqBandPanel`, `BandList`, `engineState` store, `stores/eqEditing.ts` (`eqEditing`), `eq.ts` (`reconcileBands`, `type Band`), `ipc.ts` (`setEqBand`, `eqPresetSave/Apply/Delete`).

- [ ] **Step 1: Rewrite the script to a single source of truth**

Replace the `EqPage.svelte` `<script>` band-state logic. Keep the channel-tabs, presets, header, and CSS. Core changes:

```ts
  import { get } from "svelte/store";
  import { engineState } from "../stores.js";
  import { eqEditing } from "../stores/eqEditing.js";
  import { reconcileBands, type Band } from "../eq.js";
  import { setEqBand, eqPresetSave, eqPresetApply, eqPresetDelete } from "../ipc.js";
  import EqGraph from "./EqGraph.svelte";
  import EqBandPanel from "./EqBandPanel.svelte";
  import BandList from "./BandList.svelte";

  let channelId = $state<string>("");
  let bands = $state<Band[]>([]);            // SINGLE source of truth for the active channel
  let selectedBandIndex = $state(0);

  function snapshotToBands(id: string): Band[] {
    const ch = $engineState?.channels.find((c) => c.id === id);
    return (ch?.eq_bands ?? []).map((b) => ({
      kind: b.kind as Band["kind"], freqHz: b.freq_hz, q: b.q, gainDb: b.gain_db,
    }));
  }

  function selectChannel(id: string) {
    channelId = id;
    bands = snapshotToBands(id);            // engine is dense-10; no fabrication
    selectedBandIndex = 0;
  }

  // Init once state is available.
  $effect(() => {
    if (!channelId && $engineState?.channels.length) {
      selectChannel($engineState.channels[0].id);
    }
  });

  // Reconcile from engine ONLY when idle (covers all edit modalities via eqEditing).
  $effect(() => {
    const st = $engineState;            // dependency
    if (!channelId || !st) return;
    const incoming = snapshotToBands(channelId);
    if (incoming.length === 0) return;
    bands = reconcileBands(bands, incoming, get(eqEditing));
  });

  // Single writer: all child edits land here.
  function handleBandChange(index: number, band: Band) {
    bands = bands.map((b, i) => (i === index ? band : b));
  }
  function handleSelect(index: number) { selectedBandIndex = index; }
  // Flush helper passed to the band panel (graph flushes internally via setEqBand).
  function flushBand(index: number, band: Band) {
    setEqBand(channelId, index, band.kind, band.freqHz, band.q, band.gainDb)
      .catch((e) => console.warn("[EqPage] setEqBand failed:", e));
  }
  async function flattenAll() {
    const flat = bands.map((b) => ({ ...b, gainDb: 0 }));
    bands = flat;
    for (let i = 0; i < flat.length; i++) {
      try { await setEqBand(channelId, i, flat[i].kind, flat[i].freqHz, flat[i].q, 0); }
      catch (e) { console.warn("[EqPage] flatten band failed:", e); }
    }
  }
```

- [ ] **Step 2: Wire the markup**

In the EqPage markup, replace the old `EqCanvas` + the `showingDefaults` notice + `BandList` block with:

```svelte
  <!-- hero graph -->
  <div class="eq-canvas-card">
    <div class="canvas-area">
      <EqGraph {channelId} {bands} selectedIndex={selectedBandIndex}
        onBandChange={handleBandChange} onSelect={handleSelect} />
    </div>
    <div class="gesture-hint" aria-hidden="true">
      <span>Drag = freq / gain</span><span class="hint-sep">·</span>
      <span>Scroll = Q</span><span class="hint-sep">·</span>
      <span>Dbl-click = flatten band</span><span class="hint-sep">·</span>
      <span>Arrows = nudge · Alt+↑↓ = Q</span>
    </div>
  </div>

  <div class="eq-detail-row">
    <div class="band-list-card">
      <div class="card-header"><h2 class="card-title">BANDS</h2>
        <span class="band-count">{bands.length}</span>
        <button class="flatten-btn" onclick={flattenAll}>Flatten</button>
      </div>
      <BandList {bands} selectedIndex={selectedBandIndex} onSelectBand={handleSelect} />
    </div>
    <div class="band-list-card">
      <EqBandPanel band={bands[selectedBandIndex] ?? null} index={selectedBandIndex}
        onBandChange={handleBandChange} onFlush={flushBand} />
    </div>
  </div>
```

Remove `getOrInitBands`, `bandsByChannel`, `showingDefaults`, `defaultBands` import, and the old `onMount` fabrication. Keep the presets card and channel tabs unchanged. Add minimal CSS for `.eq-detail-row` (`display:flex; gap; flex-wrap`) and `.flatten-btn` (mirror `.preset-btn`).

- [ ] **Step 3: Build + frontend tests**

Run: `pnpm --dir frontend build 2>&1 | tail -15 && pnpm --dir frontend test 2>&1 | tail -8`
Expected: builds with NO reactivity warnings; tests green. Grep the build output: `pnpm --dir frontend build 2>&1 | grep -iE "warn|conflict" || echo NO_WARNINGS`.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/lib/components/EqPage.svelte
git commit -m "feat(frontend): EqPage single-source rewrite (EqGraph + panel + list + flatten)"
```

---

## Task 10: `MicPage.svelte` reuse + delete old files

**Files:**
- Modify: `frontend/src/lib/components/MicPage.svelte` (swap `EqCanvas` → `EqGraph`)
- Delete: `frontend/src/lib/components/EqCanvas.svelte`, `frontend/src/lib/stores/eqDragging.ts`

**Interfaces:**
- Consumes: `EqGraph` (Task 6). MicPage already holds `micEqBands` ($state) + a `flushBand`-style `onFlush` calling `micEqBand`.

- [ ] **Step 1: Swap the component in MicPage**

In `frontend/src/lib/components/MicPage.svelte`:
- Change the import `import EqCanvas from "./EqCanvas.svelte";` → `import EqGraph from "./EqGraph.svelte";`.
- Find the `<EqCanvas ... onFlush={...} />` usage and replace with `EqGraph`, passing the controlled props: `bands={micEqBands}`, `selectedIndex`, `onBandChange`, `onSelect`, and the existing `onFlush` (mic uses `micEqBand`). If MicPage tracked `selectedBandIndex` differently, add a `let selectedBandIndex = $state(0)` and an `onSelect` handler. Ensure MicPage's `onBandChange` updates `micEqBands` as the single source (it already does: `micEqBands = micEqBands.map(...)`).
- Replace any `import { dragging } from "../stores/eqDragging.js"` usage with the `eqEditing` store (`import { eqEditing } from "../stores/eqEditing.js"`) and use `get(eqEditing)` in MicPage's reconcile guard (mirror EqPage Task 9 Step 1).

- [ ] **Step 2: Delete the obsolete files**

```bash
git rm frontend/src/lib/components/EqCanvas.svelte frontend/src/lib/stores/eqDragging.ts
```

Then grep for any remaining importers and fix them:
Run: `grep -rn "EqCanvas\|eqDragging" frontend/src` — expected: no matches.

- [ ] **Step 3: Build + tests**

Run: `pnpm --dir frontend build 2>&1 | tail -15 && pnpm --dir frontend test 2>&1 | tail -8`
Expected: builds (no missing-import errors), tests green. `pnpm --dir frontend build 2>&1 | grep -iE "warn|conflict" || echo NO_WARNINGS`.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(frontend): mic EQ reuses EqGraph; remove EqCanvas + eqDragging"
```

---

## Task 11: Final gate

- [ ] **Step 1: Rust workspace**

Run: `cargo test --workspace 2>&1 | grep -E "test result: FAILED|^error|FAILED" || echo RUST_CLEAN`
Expected: `RUST_CLEAN`.

- [ ] **Step 2: Frontend**

Run: `pnpm --dir frontend test 2>&1 | tail -6 && pnpm --dir frontend build 2>&1 | tail -6 && pnpm --dir frontend build 2>&1 | grep -iE "warn|conflict" || echo NO_WARNINGS`
Expected: tests green, builds, `NO_WARNINGS`.

- [ ] **Step 3: GUI link build (catches Tauri/type integration)**

Run: `cargo build -p arctis-sound-manager-ui 2>&1 | tail -5`
Expected: compiles.

- [ ] **Step 4: Commit (if fixes needed)**

```bash
git add -A && git commit -m "test: green workspace + frontend after EQ GUI redesign"
```

---

## Owner-run validation (after implementation)

These need the running GUI:
1. Open EQ, drag dot #8 first → the other 9 dots **stay put** (no disappear/collapse). Drag dot #3 → only #3 moves.
2. Edit via scroll-Q and arrow keys, wait >2 s → values **do not jump back** (the poll no longer clobbers).
3. Numeric entry for freq/gain/Q updates the curve + audio; type selector switches peaking/low-shelf/high-shelf and the curve shape changes correctly.
4. Flatten resets all bands to 0 dB; presets save/apply/delete still work.
5. Mic EQ page behaves identically (reused EqGraph).
6. `~/.cargo/bin/cargo run -p arctis-cli -- eq show <channel>` prints 10 dense bands.

---

## Self-Review Notes (author)

- **Spec coverage:** dense engine model (T1 channel, T2 mic), CLI parity via dense state (T1/T3), SVG editor (T6), single-source state + ignore-echo (T9 + reconcileBands T4 + eqEditing T5), type selector + numeric entry (T7), BandList table (T8), mic reuse + canvas removal (T10), a11y APG handles (T6), flatten (T9), tests + gates (T3, T11). Filter set limited to peaking/lowshelf/highshelf (Global Constraints). No protocol changes.
- **Reuse:** `default_10band()` is the single source of default freqs (engine + helper); `EqGraph` reused by EqPage + MicPage; existing RBJ math reused.
- **Open verification flags surfaced inline:** mic eq band source-vector name (T2), MockRunner apply_band call sequence (T1), `EqModel.bands` accessor (T1), stale BandList test (T8).
