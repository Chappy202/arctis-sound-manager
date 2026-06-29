# DayZ Competitive Surround Profile + Factory-Profile Catalog — Design

**Date:** 2026-06-29
**Status:** approved design, pending implementation plan
**Author:** brainstormed with the owner (JJ)

## 1. Goal

Turn the existing one-off `dayz` factory profile into an opinionated, research-backed
"DayZ competitive positioning" preset, **and** restructure factory profiles into a
**data-driven catalog** so future games and headphone-correction profiles are pure data,
not new code paths.

This rides on top of the **already-live discrete-7.1 → HRIR pipeline** — no new audio
architecture is introduced. The work is: pin the right per-profile settings, add a general
post-convolution correction stage, and generalize the factory-profile mechanism.

## 2. Background — current state (verified)

- **Discrete 7.1 is already live and correctly wired.** `Arctis_Game.output` exposes 8
  discrete ports (FL FR FC LFE RL RR SL SR), all linked into the 8-channel
  `effect_input.arctis_surround` → HRIR convolver → stereo → hardware. This is the
  research-recommended path; it is the piece previously noted as "shipped, pending owner
  live DayZ validation". That live validation remains the owner's to perform and is **not**
  blocked by this spec.
- **Existing `dayz` factory profile** — `crates/engine/src/factory_profiles.rs` (`dayz_profile`,
  ~lines 9-44). Clones the active profile, sets `name="DayZ"`, seeds the game channel with the
  `"FPS / Footsteps (Competitive)"` EQ preset, enables surround with `channels=["game"]`,
  preserves the active profile's `hw_sink`/`hrir`, sets `default_sink_channel="game"`.
  Registered via a hardcoded `"dayz" => dayz_profile()` arm in `create_factory_profile()`
  (`crates/engine/src/engine.rs` ~lines 1167-1185).
- **Surround model** — `SurroundConfig` (`crates/config/src/schema.rs` ~lines 338-381):
  `enabled`, `hrir: Option<String>` (bare stem), `channels: Vec<String>`, `hw_sink: Option<String>`,
  `mode: SurroundMode`, `crossfeed: u8`. `SurroundMode` (~lines 313-326): `Auto | Hrir71 | Hrir51 | StereoBypass`.
  `resolve_effective_mode()` (`engine.rs` ~120-135) maps `Auto` → `Hrir71` at 8ch.
- **Renderer** — `crates/audio/src/surround.rs`: `render_surround_conf_ex(&SurroundRender) -> Result<String, AudioError>`
  (~line 82). `SurroundRender` already carries `output_eq: Option<&EqModel>` (post-convolution
  EQ on the binaural output) — **currently never set by any profile**. Convolver `blocksize` is
  **not** parameterized (PipeWire default). `hw_sink` pins the playback target (~104-106).
- **HRIR catalog** — `crates/engine/src/hrir_catalog.rs` (`const CATALOG`, ~35-108), entries carry
  `stem, display, group, tonality: Tonality::{Dry,Neutral,Roomy}, license, origin`. Includes
  `04-gsx-sennheiser-gsx` (Dry, Proprietary, Import) and bundled `07-oal+++-openal-max` (Dry,
  Permissive, Bundled). HRIR WAVs live at `~/.local/share/pipewire/hrir_hesuvi/profiles/`;
  `04-gsx-sennheiser-gsx.wav` **is present on this machine**. Import infra in `hrir_import.rs`
  (`ensure_bundled`, `import_dir`, `BUNDLED_HRIR`).
- **EQ presets** — `crates/engine/src/presets.rs`: `factory_eq_presets() -> Vec<EqPreset>`,
  including `"FPS / Footsteps (Competitive)"` (~line 26). EQ band kinds map via
  `convert::band_kind_from_str` / `band_kind_to_str`.
- **UI** — `frontend/src/lib/components/SpatialPage.svelte` (enable, HRIR `Select`, channel
  routing, hw-sink dropdown); `ProfilesDropdown.svelte` (profile list + a hardcoded
  "Create DayZ profile" button calling `profileCreateFromFactory("DayZ")`).

## 3. Design decisions (owner-approved)

| Decision | Choice |
|---|---|
| Footstep EQ placement | **Split**: footstep boost stays pre-convolution on the game channel; a gentle Arctis-correction EQ is added **post-convolution** on the binaural output. |
| DayZ default HRIR | **Pin GSX** (`04-gsx-sennheiser-gsx`); if not installed, **fall back to a safe dry HRIR and raise an import prompt** (no silent permanent substitution). |
| Convolver latency | **Pin blocksize 128** (~2.7 ms @ 48 kHz; full reverb tail preserved). |
| Scaffolding scope | **Full**: build the data-driven factory-profile catalog, generic apply, Tauri/UI listing, and the two new `SurroundConfig` fields; DayZ is the first catalog entry. |

## 4. Architecture — the reusable foundation

Everything game-specific becomes **data in a catalog**; everything mechanical becomes a
**general capability** any profile can use. This mirrors the existing `const CATALOG`
(HRIRs) and `factory_eq_presets()` (EQ) patterns.

### A1. Data-driven Factory-Profile Catalog

New module `crates/engine/src/factory_profiles.rs` (extend in place):

```rust
/// One channel's pre-spatial content EQ seed (a named preset applied to a channel).
pub struct ChannelEqSeed {
    pub channel_id: &'static str,   // "game"
    pub preset_name: &'static str,  // "FPS / Footsteps (Competitive)"
}

/// A factory profile template: the overrides applied onto the active profile.
pub struct FactoryProfileSpec {
    pub name: &'static str,                          // "DayZ"
    pub hrir_stem: Option<&'static str>,             // Some("04-gsx-sennheiser-gsx")
    pub mode: SurroundMode,                          // Hrir71
    pub blocksize: Option<u32>,                      // Some(128)
    pub surround_channels: &'static [&'static str],  // ["game"]
    pub default_sink_channel: Option<&'static str>,  // Some("game")
    pub content_eq: Option<ChannelEqSeed>,           // pre-spatial, per channel
    pub correction_eq_preset: Option<&'static str>,  // post-spatial → SurroundConfig.output_eq
}

/// The catalog. DayZ is the first (today: only) entry.
pub fn factory_profiles() -> &'static [FactoryProfileSpec];

/// Look up a template by name, case-insensitive.
pub fn find_factory_profile(name: &str) -> Option<&'static FactoryProfileSpec>;

/// Apply a template onto a clone of the active profile, preserving hardware-specific
/// settings (node names, hw_sink, mic chain, master volume). Resolves preset names to
/// bands via the EQ-preset catalog; unknown preset names are a hard error (caught by tests).
pub fn apply_factory_spec(active: &Profile, spec: &FactoryProfileSpec)
    -> Result<Profile, EngineError>;
```

`dayz_profile(active)` collapses to `apply_factory_spec(active, &DAYZ)`. In `engine.rs`,
`create_factory_profile(template)` replaces its hardcoded match with
`find_factory_profile(template).ok_or(BadRequest)?` → `apply_factory_spec`. **Adding a future
game = one `FactoryProfileSpec` struct literal**, no new code paths.

### A2. General two-stage spatial EQ

A documented, reusable pattern any profile inherits:

- **Channel EQ** (`ChannelConfig.eq`, existing) = *pre-spatial content shaping*, per source/game.
- **Surround output EQ** (`SurroundConfig.output_eq`, new) = *post-spatial headphone/room correction*, per headphone.

Both are plain `EqModel`s reusing the existing `EqEditor`, `band_kind_*`, the EQ-preset
catalog, and the low/high-shelf default work already shipped. No EQ machinery is duplicated.

### A3. General `SurroundConfig` knobs (additive, back-compat)

Two new fields on `SurroundConfig` (`crates/config/src/schema.rs`), both `#[serde(default)]`:

```rust
/// Post-convolution EQ applied to the 2-ch binaural output. Empty = no post EQ.
#[serde(default)]
pub output_eq: Vec<EqBandConfig>,

/// Convolver partition size. None = PipeWire default.
#[serde(default)]
pub blocksize: Option<u32>,
```

Old configs load with `output_eq=[]`, `blocksize=None` → **behavior identical to today** for
every existing/non-DayZ profile. These flow through the renderer for *any* surround profile.

### A4. General missing-pinned-HRIR mechanism

When a profile pins an HRIR stem that is not installed in the profiles dir:

1. **Audio keeps working** — the engine resolves to a safe **bundled dry** fallback
   (`07-oal+++-openal-max`), so surround is never silently broken.
2. **A dismissible prompt flag** is surfaced in engine state (e.g. a
   `surround.hrir_missing: Option<String>` carrying the requested stem) that the Spatial page
   renders as *"<HRIR> not installed — Import your HRIRs"*, wired to the existing import action.

This is generic: it covers every HRIR/profile, not just GSX/DayZ. The fallback choice and the
flag are computed in the engine apply path (where `resolve_hrir_path` already runs).

### A5. HRIR curation via existing metadata (no new field)

The HRIR picker groups entries by the existing `Tonality` (`Dry`≈Competitive,
`Neutral`, `Roomy`≈Cinematic), so "pick a positioning HRIR" generalizes to all future
profiles without adding a `use_case` field. UI-only grouping; the catalog is unchanged.

## 5. The DayZ catalog entry (first concrete instance)

```rust
const DAYZ: FactoryProfileSpec = FactoryProfileSpec {
    name: "DayZ",
    hrir_stem: Some("04-gsx-sennheiser-gsx"),
    mode: SurroundMode::Hrir71,
    blocksize: Some(128),
    surround_channels: &["game"],
    default_sink_channel: Some("game"),
    content_eq: Some(ChannelEqSeed {
        channel_id: "game",
        preset_name: "FPS / Footsteps (Competitive)",  // existing preset, PRE
    }),
    correction_eq_preset: Some("Arctis Spatial Correction"),  // new preset, POST
};
```

### 5.1 New EQ preset: "Arctis Spatial Correction" (post-convolution)

Added to `factory_eq_presets()` in `crates/engine/src/presets.rs`. Gentle (≤ ±2 dB),
purpose = neutralize the Arctis bass shelf + HRIR coloration so HRTF cues land cleanly,
**without** re-boosting the presence band the footstep EQ already handles. 10 bands at the
canonical centers; shelves at the extremes (consistent with the default EQ):

| Band | Center | Kind | Gain (dB) | Q | Rationale |
|---|---|---|---|---|---|
| 0 | 31 Hz | lowshelf | −2.0 | 0.7 | tighten Arctis bass shelf; reduce footstep masking |
| 1 | 62 Hz | peaking | 0.0 | 1.0 | — |
| 2 | 125 Hz | peaking | −1.5 | 1.0 | reduce low-mid bloom |
| 3 | 250 Hz | peaking | 0.0 | 1.0 | — (weight handled pre by footstep EQ) |
| 4 | 500 Hz | peaking | 0.0 | 1.0 | — |
| 5 | 1000 Hz | peaking | 0.0 | 1.0 | — |
| 6 | 2000 Hz | peaking | 0.0 | 1.0 | — (presence handled pre) |
| 7 | 4000 Hz | peaking | 0.0 | 1.0 | — (presence handled pre) |
| 8 | 8000 Hz | peaking | +1.5 | 1.0 | gentle localization clarity (kept modest — 6–10 kHz fatigue risk) |
| 9 | 16000 Hz | highshelf | +1.5 | 0.7 | slight "air" for distance/elevation cues |

All gains within the engine's existing ±12 dB bounds. The preset is visible and editable
like any other.

## 6. Mechanical layers

### 6.1 Renderer (`crates/audio/src/surround.rs`)

- Add `blocksize: Option<u32>` to `SurroundRender` (and `SurroundSpec` as needed).
- Emit `blocksize = <N>` into each convolver node's args when `Some`; omit when `None`
  (preserves today's default behavior and keeps existing snapshots valid for `None`).
- `output_eq` is already supported — no renderer change for the post EQ itself.
- Snapshot tests in `surround.rs` get a new `Some(128)` case; existing `None` snapshots unchanged.

### 6.2 Engine apply path (`crates/engine/src/engine.rs`)

- Thread `surround.output_eq` (→ `EqModel` via `convert`) and `surround.blocksize` into the
  `recreate_ex(hrir_path, channels, output_eq, blocksize)` call (extend its signature).
- Compute the missing-HRIR fallback + state flag where `resolve_hrir_path` runs.
- `create_factory_profile` becomes catalog-driven (A1).

### 6.3 Tauri commands + UI

- **Command:** `list_factory_profiles()` → returns catalog entries (name + short metadata)
  for the UI to render. `profileCreateFromFactory(name)` already exists and stays generic.
- **ProfilesDropdown.svelte:** replace the hardcoded "Create DayZ profile" button with a
  data-driven list of factory templates (DayZ is today's only row).
- **SpatialPage.svelte:** (a) render the missing-HRIR import prompt from the new state flag;
  (b) add a toggleable "Spatial correction (post)" section reusing `EqEditor` bound to
  `surround.output_eq`; (c) group the HRIR `Select` by tonality. Mode/blocksize shown read-only.
- New IPC setters as needed: `surroundSetOutputEq(bands)`, `surroundSetBlocksize(n)`
  (follow the existing `surroundSetHrir`/`surroundSetChannels` shape).

### 6.4 Migration / back-compat

Both new `SurroundConfig` fields are `#[serde(default)]`; existing TOML configs load
unchanged. Existing saved profiles (including a previously-created "DayZ") are not rewritten;
re-creating the DayZ factory profile applies the new template. No destructive migration.

## 7. Testing strategy

- **Catalog (`factory_profiles.rs`):** `find_factory_profile` is case-insensitive; `apply_factory_spec`
  for DAYZ yields `name="DayZ"`, `hrir=Some("04-gsx-sennheiser-gsx")`, `mode=Hrir71`,
  `blocksize=Some(128)`, `surround.channels=["game"]`, `default_sink_channel="game"`,
  game channel EQ = footstep preset (non-empty, 10 bands), `surround.output_eq` non-empty;
  node names / hw_sink / mic preserved from active. Unknown preset name → `EngineError`.
  Existing DayZ tests are updated to the catalog path.
- **EQ preset:** `"Arctis Spatial Correction"` present, all |gain| ≤ 2 dB, shelves at the extremes,
  10 bands, validates within ±12 dB bounds.
- **Schema:** `SurroundConfig` round-trips with and without `output_eq`/`blocksize`; an old TOML
  (missing both) loads with `[]`/`None`.
- **Renderer:** snapshot for `blocksize=Some(128)` (emits `blocksize = 128` per convolver);
  `None` snapshot byte-identical to today.
- **Engine apply:** missing pinned HRIR → resolves to bundled fallback **and** sets the
  `hrir_missing` flag; present HRIR → no flag, pinned stem used. `output_eq` + `blocksize`
  reach `recreate_ex`.
- **Frontend:** pure helpers only (per repo convention — no jsdom): factory-list shaping,
  tonality grouping, output-EQ band mapping.

## 8. Out of scope / future (catalog-ready, not built now)

- Additional factory profiles (Arma Reforger, generic FPS-Competitive, Cinematic) — future
  struct literals.
- Head-tracking and personalized HRIR (Impulcifer import) — the remaining ceiling on
  front/back confusion; roadmap items, not this spec.
- Per-profile global blocksize policy beyond the `Option<u32>` field.
- A studio-mic chain profile (separate spec).

## 9. Risks / notes

- **Live validation is still the owner's.** The discrete-7.1 path is live but the owner's
  in-game DayZ confirmation (imaging, front/back, footstep audibility) is the real acceptance
  test; this spec does not and cannot substitute for it.
- **Renderer snapshot churn** is expected and intended (blocksize emission).
- **GSX is proprietary** — bundled redistribution is forbidden (design constraint D2 from the
  spatial overhaul). It is imported locally; the missing-HRIR mechanism (A4) covers the
  fresh-install case.
- Changing band kinds / blocksize requires a **filter-chain rebuild** (daemon reconcile),
  which the profile-apply path already performs.
