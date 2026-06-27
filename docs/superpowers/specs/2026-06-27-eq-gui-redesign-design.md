# EQ GUI Redesign — Design Spec

**Date:** 2026-06-27
**Status:** Approved design → ready for implementation planning
**Scope:** The parametric-EQ editing experience — the GUI (`EqPage` + canvas) **and** the engine's
EQ band data model that the GUI synchronises against. Reused by both the channel EQ and the Mic EQ.

## 1. Problem

The current parametric-EQ GUI is glitchy and unreliable. The headline symptom — **"all my dots
disappear"** — is structural, not a rendering hiccup. A deep diagnosis (with file:line evidence)
found three confirmed root causes:

1. **Disappearing dots = data-model impedance mismatch.** The engine stores a *sparse,
   variable-length* per-channel band vector where **empty means "flat / no override"**
   (`crates/config/src/schema.rs`). `state()` returns that raw vector
   (`crates/engine/src/engine.rs` ~line 249), and `set_eq_band` **pads it with `1000 Hz`
   placeholders** up to the edited index (`engine.rs` ~lines 502–511). So editing dot #3 first makes
   the engine report **4 bands**; the GUI's 2-second background `state-changed` poll then overwrites
   the UI's 10-band array with that 4-element array, and the canvas draws only `bands.length` dots →
   **6 dots vanish**. Editing a high band first collapses the lower dots onto 1000 Hz.
2. **Jumpiness = the 2 s poll clobbering edits.** `src-tauri/src/lib.rs` emits `state-changed`
   unconditionally every 2 s; `EqPage`'s reconcile effect rewrites the band array from
   `channel.eq_bands`. The only guard (`if (get(dragging)) return`) covers **pointer drags only** —
   not wheel-Q or arrow-key edits — and there is a race right after `pointerup`. The canvas also
   **discards** the authoritative `EngineState` the mutation returns and depends entirely on the
   laggy poll.
3. **Fragility = four-way band bookkeeping.** Band data lives in `EqPage.bands`,
   `EqPage.bandsByChannel`, `EqCanvas.bands` (a `$bindable` passed one-way), and the engine — with
   competing writers.

(Canvas resize/redraw and Tauri arg-key casing were investigated and **ruled out** as causes.)

## 2. Decisions (locked)

1. **GUI + engine dense model.** Fix the engine to return a dense, fixed-length band array AND
   rebuild the GUI. (Fixes the bug at its source; benefits the CLI; cleans the config.)
2. **Fixed 10-band set.** Always 10 bands at the canonical default frequencies; no Sonar-style
   add/delete. Stable slots are the core of the fix.
3. **Per-band controls:** a **filter-type selector** and **inline numeric entry** (freq/gain/Q) per
   band. **No** per-band bypass.
4. **Filter types:** **Peaking, Low-shelf, High-shelf** only — the three gain-based kinds the audio
   engine already supports (`bq_peaking`/`bq_lowshelf`/`bq_highshelf`). **No** High-pass/Low-pass
   (gain-less; their differing handle semantics would undercut the reliability goal). Documented as a
   possible future follow-up.
5. **Rendering: SVG**, replacing the imperative `<canvas>`.

## 3. Research grounding (validated, not assumed)

- **Build, don't adopt.** No maintained, framework-agnostic, extractable EQ-curve-editor component
  exists (closest: `weq8`, ISC, unmaintained since 2022, coupled to a Web Audio runtime we don't
  have — our DSP is PipeWire `bq_*` biquads). Reuse the *ideas/UX conventions*, not the code.
- **SVG over canvas (high confidence).** A `<path>` curve + focusable `<circle role="slider">`
  handles are DOM nodes the framework owns and diffs, so handles **cannot desync or disappear**;
  hit-testing and accessibility are native; vector is DPR-crisp. Canvas is only warranted for an
  optional non-interactive spectrum layer later. This single change eliminates the whole
  disappearing-dots *rendering* class; the dense-engine model eliminates the *data* cause.
- **Curve math (RBJ Audio EQ Cookbook).** The frontend already implements correct biquad magnitude
  for peaking/low-shelf/high-shelf and sums in dB across series bands at 48 kHz (`eq.ts`). No new DSP
  is required for the chosen filter set.
- **State pattern.** Single fixed-length source of truth, `$derived` curve/handles, optimistic local
  edits during interaction, **ignore backend echoes while editing** with a last-writer-wins reconcile
  when idle. Avoid the `$state`/prop-name collision class (we hit one on the mixer branch).
- **Sonar parity (for look & feel).** Curve-first parametric EQ on a dark grid; colored numbered
  dots; drag X=freq / Y=gain; per-band filter-type dropdown (Peaking default); **inline numeric
  freq/gain/Q entry**; per-channel EQ; custom presets. (Sonar's 5→10 dynamic add/delete and HPF/LPF
  are intentionally out of scope per the decisions above.)

## 4. Architecture

### 4.1 Engine — dense, fixed-length band model

The fix that kills the data cause. The protocol is **unchanged** (`SetEqBand` / `MicEqBand`); only
the engine's read/seed behaviour changes.

- **`state()` always materializes exactly 10 bands** per channel. Build `eq_bands` from
  `convert::eq_model_for(channel)` — which already returns `EqModel::default_10band()` for an empty
  config and maps overrides otherwise — then map its 10 `EqBand`s to `EqBandSnapshot`. A flat channel
  now reports 10 real bands at the canonical ISO centers, gain 0.0 — never `[]` or a short array.
- **`set_eq_band` seeds the dense default before writing.** If `channel.eq` is shorter than 10, seed
  it from the canonical default 10 bands (correct frequencies from `default_10band()`, **not** 1000
  Hz) and then write the edited index. The persisted config becomes dense with correct defaults.
- **Presets** operate on the dense 10: `EqPresetSave` saves the current 10 bands; `EqPresetApply`
  replaces all 10. Preset `band_count` is 10.
- **Canonical default frequencies.** `EqModel::default_10band()` is the single source of the default
  band frequencies. The frontend renders whatever the engine returns and no longer fabricates
  defaults; any frontend default constant must match `default_10band()`.
- **Mic EQ** receives the same dense-10 treatment so the rebuilt `EqGraph` works identically there.
- **CLI** `eq show` now prints the dense 10 bands — a free parity/usability win.

### 4.2 GUI — SVG editor + single source of truth

- **`EqGraph.svelte` (new, replaces `EqCanvas.svelte`):** an `<svg>` with a log-frequency grid
  (labels 20 Hz–20 kHz), gain grid (±12 dB, 0 dB emphasized), a summed-response `<path>` filled to
  the 0 dB line, and **ten `<circle role="slider">` handles** (colored, numbered, with a larger
  transparent hit/focus target) whose `cx={freqToX(band.freqHz)}` / `cy={gainToY(band.gainDb)}` are
  bound to the band model. A floating readout chip shows freq/gain/Q for the active band. The
  component is **controlled**: it receives `bands` (read-only) + an `onBandChange(index, band)`
  callback and an optional `onFlush` override (for Mic reuse). It keeps **no** band state of its own.
- **`EqBandPanel.svelte` (new):** the selected band's detail — a **type dropdown**
  (Peaking / Low-shelf / High-shelf), **numeric inputs** for frequency, gain, and Q (clamped to
  `FREQ_MIN..FREQ_MAX`, `GAIN_MIN..GAIN_MAX`, `Q_MIN..Q_MAX`; NaN rejected), and a **Reset band**
  action (gain→0, type→peaking, Q→1).
- **`BandList.svelte` (repurposed):** a compact 10-row table (freq / gain / Q / type) for overview
  and click-to-select.
- **`EqPage.svelte` (rewrite):** holds the single `bands: Band[]` state for the active channel,
  channel tabs, the `EqGraph`, the band panel + list, a **Flatten** (reset-all-to-0 dB) action, the
  retained presets card, and gesture hints. Removes `bandsByChannel`, the `$bindable` band prop, and
  the canvas-local copy.
- **`stores/eqEditing.ts` (replaces `eqDragging.ts`):** an `eqEditing` writable boolean plus a helper
  that sets it true and auto-clears after a short settle window. Set on **every** edit modality —
  pointer drag, wheel-Q, keyboard, and numeric-field focus.

### 4.3 Interaction model

- **Drag a handle** → frequency (X, log) + gain (Y, linear ±12 dB).
- **Scroll on a handle** → Q (multiplicative steps, clamped).
- **Double-click a handle** → reset that band's **gain to 0 dB** (quick flatten of one band; leaves
  type/freq/Q). Distinct from the panel's full **Reset band** (gain→0, type→peaking, Q→1).
- **Keyboard (WAI-ARIA APG slider):** Arrow Up/Down = gain; Arrow Left/Right = frequency; a modifier
  (e.g. Alt+Arrow) = Q; Home/End = axis extremes; Shift = coarse. `aria-valuetext` announces the
  freq/gain/Q triple.
- **Numeric panel** → precise, accessible entry for freq/gain/Q + type; the authoritative non-drag
  path.
- All edits flow up through `onBandChange`; `EqPage` updates the single `bands` array and flushes.

## 5. State / reactivity model (reliability core)

- **Single owner:** `EqPage.bands` (`$state`) is the only source of truth for the active channel;
  child components are controlled. One writer.
- **Invariant length:** engine returns dense-10 and the UI never shrinks → `bands.length === 10`;
  handles render via `{#each bands as b, i (i)}`. Dots cannot vanish.
- **Optimistic + ignore-echo:** on any edit, `EqPage` updates `bands` locally immediately and sets
  `eqEditing` (with settle timer). The `engineState` reconcile effect **skips while `eqEditing`** (so
  no modality clobbers an in-progress edit), and uses the **EngineState returned by the flush** rather
  than waiting on the 2 s poll. A lightweight per-channel edit version drops a stale reconcile
  (last-writer-wins). Because the band count is invariant, even a mistimed reconcile cannot remove
  dots.
- **Channel switch:** load `bands` from `engineState.channels[id].eq_bands` (always 10). No
  fabrication.
- **Flush cadence:** throttled (~50 ms trailing) during drag; immediate on pointerup, wheel, key
  commit, and numeric-field commit. The final value is always flushed.
- **`reconcileBands(local, incoming, editing)`** is a pure reducer (unit-testable): returns `local`
  unchanged while `editing`; otherwise returns `incoming`; never changes array length.

## 6. Curve math

Reuse the existing pure RBJ implementation (`eq.ts`): per-band biquad magnitude for peaking,
low-shelf, and high-shelf at `DEFAULT_SAMPLE_RATE = 48000`, summed in dB across the 10 series bands,
sampled on a log-frequency axis into an SVG path `d` string. No new DSP. (`Band.kind` stays
`"peaking" | "lowshelf" | "highshelf"`.)

## 7. Error handling

- **Flush failure:** non-fatal — the local band value holds; the next idle reconcile corrects it if
  the engine rejected the change (errors logged, never swallowed silently in a way that loses user
  feedback).
- **Numeric input:** clamp to legal ranges; reject NaN/empty; revert the field on invalid commit.
- **Engine-down / no state yet:** graceful loading/empty state; no default fabrication path.
- All audio/subprocess errors continue to surface per G7.

## 8. Accessibility (WAI-ARIA APG slider/multi-thumb)

Each handle is a focusable `role="slider"` with `aria-valuemin/max/now` on gain and `aria-valuetext`
for the full freq/gain/Q triple; fixed tab order; full keyboard control (§4.3); visible focus ring
(not color-only); larger-than-visual hit/focus target; `prefers-reduced-motion` respected; numeric
fields and the type dropdown are standard accessible controls.

## 9. Testing (TDD)

- **Engine (Rust):** `state()` returns exactly 10 dense bands for a flat channel (canonical freqs,
  gain 0) and for a partially-overridden channel (overrides applied, defaults elsewhere);
  `set_eq_band` on a fresh channel seeds dense defaults (no 1000 Hz placeholders) then writes the
  index; `EqPresetApply`/`EqPresetSave` round-trip 10 bands.
- **Frontend (vitest, pure):** existing `bandMagnitudeDb` / `summedCurveDb` tests retained; new tests
  for `reconcileBands` (returns local while editing; returns incoming when idle; never changes
  length), numeric clamping, and `freqToX`/`xToFreq` round-trip.
- **Build/typecheck:** `pnpm --dir frontend build` clean, **no reactivity warnings** (guard against
  the `$state`/prop-name collision class).

## 10. Build order (engine-first)

1. **Phase 1 — Engine dense model (headless):** dense `state()`, `set_eq_band` seeding, preset
   density, mic-EQ density; CLI-verifiable; Rust tests. No protocol change.
2. **Phase 2 — GUI core:** `eq.ts` `reconcileBands` reducer; `eqEditing` store; `EqGraph.svelte`
   (SVG, controlled); `EqBandPanel.svelte`; `BandList.svelte` repurpose; `EqPage.svelte` rewrite to a
   single source of truth; Mic-page reuse; tests.
3. **Phase 3 — Polish:** visual fidelity (dark grid, colored numbered dots, curve fill, selected-band
   emphasis), APG keyboard + `aria-valuetext`, readout chip, Flatten action, gesture hints.

## 11. Risks & mitigations

- **Engine band-model change ripples to CLI/tests.** Dense `state()` changes `eq_bands` output;
  update CLI `eq show` expectations and any engine tests asserting empty/short `eq_bands`. Mitigation:
  Phase 1 is headless and fully tested before any UI work.
- **Frontend default freqs must match `default_10band()`.** Otherwise the displayed pre-state default
  diverges from the engine. Mitigation: the UI renders engine-supplied bands; any local default
  constant is asserted equal to `default_10band()`.
- **Reactivity-warning regressions.** Mitigation: controlled components (no `$bindable` band copies),
  explicit non-colliding prop names, build gate fails on warnings.
- **Mic EQ parity.** The rebuilt `EqGraph` must work for the mic EQ via `onFlush`; ensure the mic EQ
  snapshot is also dense-10. Mitigation: covered in Phase 1 (engine) + Phase 2 (reuse) with a check.
- **Preset band-count drift.** A saved preset must be 10 bands so applying never changes the dot
  count. Mitigation: presets are saved/applied against the dense model; tested.
