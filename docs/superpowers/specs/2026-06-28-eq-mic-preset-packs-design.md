# EQ & Mic Preset Packs — Design Spec

**Date:** 2026-06-28
**Status:** Approved architecture (owner); catalog values pending research reconciliation (see §8).
**Refs:** `ARCHITECTURE.md` (G1–G10), prior EQ redesign spec, project memory.

## 1. Goal

Ship **built-in preset packs**:
- **Channel EQ:** factory presets (Flat, Bass Boost, FPS/Footsteps, Immersive, Vocal, Warm, Bright,
  Treble Smooth, Bass Reducer), including Arctis Nova Pro-tuned curves.
- **Microphone:** Sonar-style voice presets (Clean, Less Nasal, Warmth, Clarity, Broadcast, De-ess,
  Deep Voice, Streamer) — **full mic-chain** presets (EQ + gain/highpass/compressor/gate/suppression).

## 2. Decisions (owner)

- **D1 — Read-only factory catalog.** Built-in presets are defined in Rust, always present, never
  written to disk, and cannot be deleted. They are distinct from the user's saved
  `config.eq_presets` library (which keeps working as-is: save/delete only touch user presets).
- **D2 — Full mic chain.** A mic preset sets EQ **and** the gain/highpass/compressor/gate/suppression
  stages. Applying a mic preset **preserves** the user's `enabled` (master on/off) and `hw_mic`
  (selected hardware mic) — it only overlays DSP stage settings.

## 3. Non-negotiable constraints

- Pure software EQ/DSP only — **no device writes**; the device-write allowlist stays empty (G2).
- 48 kHz; PipeWire subprocess model; engine UI-agnostic (tauri only in src-tauri).
- **GUI ⇄ CLI feature parity is mandatory.** Every preset action available in one is available in both.
- Reuse over duplication (G1); typed errors, no `unwrap`/`expect`/`panic` on runtime paths (G7);
  small focused files (G6).
- No automated test performs a live audio write — `MockRunner` + fixtures only.
- Canonical EQ model: fixed 10 bands at `[31,62,125,250,500,1000,2000,4000,8000,16000]` Hz;
  `EqBandConfig { kind: "peaking"|"lowshelf"|"highshelf", freq_hz, q, gain_db }`; band 0 may be
  lowshelf, band 9 may be highshelf, rest peaking; gains clamped to the existing EQ bounds.

## 4. Data model

- **Channel presets** reuse the existing `EqPreset { name, kind_hint, bands }`.
- **Mic presets** are new (`crates/config/src/schema.rs`):
  ```rust
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
  ```
  (= `MicChainConfig` minus `enabled` and `hw_mic`.) Reuses the existing stage structs so `apply`
  is a field-by-field overlay.

## 5. Factory catalog (code-defined)

New module `crates/engine/src/presets.rs` (pure data + constructors) exposing:
- `pub fn factory_eq_presets() -> Vec<arctis_config::EqPreset>`
- `pub fn factory_mic_presets() -> Vec<arctis_config::MicPreset>`

Band order in every vector below: `[31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000]` Hz.
Q = 1.0 for peaking unless noted; shelves (band 0 / band 9) Q = 0.7. Band 0 = lowshelf, band 9 =
highshelf in presets that move those bands as shelves; otherwise peaking at 0 dB.

### 5.1 Channel EQ presets (gain_db per band) — **research-grounded (oratory1990 measured)**

Anchored to oratory1990's acoustic measurement of the Nova Pro Wireless (Bluetooth, ANC off) via
AutoEq, cross-checked against SoundGuys / TechPowerUp / HiFi Oasis / Igor's Lab. The headset's
signature: a large **+8–10 dB mid-bass hump at ~125 Hz** (the dominant flaw — every preset except Warm
corrects it hard), a **dip at ~4 kHz** (the key clarity fix), and a **narrow 8 kHz sibilance peak**.
Band 0 (31 Hz) = lowshelf Q0.7, band 9 (16 kHz) = highshelf Q0.7; the 125/250 Hz corrective bands use
Q1.41; others peaking Q1.0 unless noted.

| Preset | kind_hint | 31 | 62 | 125 | 250 | 500 | 1k | 2k | 4k | 8k | 16k | preamp |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| Flat | reference | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |
| Reference (Calibrated) | reference | −1.5 | +0.3 | −9.5 | −1.6 | +1.0 | +2.2 | +1.6 | +2.8 | +0.2 | +2.2 | −3.2 |
| Bass Boost | music | +4.0 | +2.0 | −5.0 | −0.5 | +0.5 | +1.5 | +1.0 | +2.5 | −1.0 | +1.0 | −4.0 |
| FPS / Footsteps | gaming | −2.0 | −1.5 | −7.0 | −2.5 | 0 | +2.0 | +3.5 | +5.0 | +2.5 | +1.0 | −5.0 |
| Immersive | movies | +4.5 | +2.5 | −5.5 | −1.0 | 0 | +0.5 | +1.0 | +2.0 | −2.0 | +3.5 | −4.5 |
| Vocal Clarity | voice | −3.0 | −1.5 | −8.5 | 0 | +1.0 | +3.0 | +3.0 | +4.0 | −1.5 | +1.5 | −4.0 |
| Warm | music | +2.5 | +3.0 | −3.5 | +1.5 | +0.5 | 0 | 0 | +1.5 | −3.5 | −1.5 | −3.0 |
| Treble Smooth (De-Harsh) | fatigue | −1.0 | +0.5 | −8.0 | −1.5 | +1.0 | +2.0 | +2.0 | +3.0 | −4.0 | −2.0 | −3.0 |

Notes: **Flat** is true bypass (all zeros — a reset). **Reference (Calibrated)** is oratory1990's
neutral correction verbatim. FPS adds the 125 Hz correction the only published community FPS curve
(EveZone) omits, then keeps that curve's 2–4 kHz footstep emphasis. Treble Smooth uses a narrower
Q1.4 at 8 kHz to target the specific sibilance peak. **8 kHz at 16 kHz differs in wired mode** (the
measured wired profile wants +2.8/−5.0 there vs +0.2/+2.2 BT) — these presets target the primary
Bluetooth/wireless use case.

**Preamp / headroom:** the `preamp` column is the recommended pre-EQ attenuation (dB) to avoid digital
clipping from the positive boosts. This iteration does **not** add a `preamp_db` field to `EqPreset`
and does **not** auto-apply it (no schema change); the value is carried in the preset's `kind_hint`/
description text and shown to the user as information only. Auto-preamp is future work (§10).

### 5.2 Mic presets — **verbatim from the Sonar preset library** (reference app)

Lifted verbatim from the bundled reference app's Sonar-replica preset files
(`/home/jj/.../Arctis-Sound-Manager/.../gui/presets/*[Mic].json`), which use our **exact** 10 canonical
frequencies (filter1 = 31 Hz `lowShelving`, filter10 = 16 kHz `highShelving`, rest `peakingEQ`,
Q = 0.7071). Gains below are the Sonar values 1:1 (band order `[31,62,125,250,500,1000,2000,4000,8000,16000]`).
Notably, **every** Sonar voice preset cuts 31 Hz by −12 dB (a universal rumble/handling-noise floor)
and enables noise-canceling (value 0.9) — which we map to our `suppression` stage.

| Preset | EQ gains (10-band) | supp | gate | comp | source |
|---|---|---|---|---|---|
| Flat | `0,0,0,0,0,0,0,0,0,0` | off* | off | off | Sonar |
| Less Nasal | `−12,0,0,0,−2,−4,−2,+2,+4,0` | on | off | off | Sonar |
| Deep Voice | `−12,+4,+4,+3,0,−3,−1,0,0,0` | on | off | off | Sonar |
| Balanced | `−12,0,0,−3,−2,0,+2,+3,+1,0` | on | off | off | Sonar |
| Clarity High Pitch | `−12,−12,−4,−4,0,0,+3,+4,+4,+4` | on | off | off | Sonar |
| Clarity Low Pitch | `−12,−3,−2.5,0,0,0,+3,+4,+4,+4` | on | off | off | Sonar |
| Broadcast High Pitch | `−12,−12,−3,+6,+3,−2.5,0,+3,+4,+4` | on | off | off | Sonar |
| Broadcast Low Pitch | `−12,+2,+5,+2,−3,−2,0,+3,+4,+4` | on | off | off | Sonar |
| Walkie Talkie | `−12,−12,−12,−12,+5,+6,+6,+5,−12,−12` | on | off | off | Sonar |
| Streamer | `−12,0,0,−2,−1,0,+2,+3,+1,0` | on | on | on | ASM original |

`eq_enabled = true` for all. `*Flat` keeps noise-canceling on in Sonar; to make Flat a genuine reset
we ship it with `suppression = off` (a true "no processing" baseline). **Streamer** is the one
ASM-original preset that exercises the full chain (gate + compressor on) to satisfy D2's full-chain
intent; the 9 Sonar presets are EQ + noise-canceling snapshots, faithful to Sonar.

**Stage mapping (Sonar → our `MicChainConfig`):** `noiseCanceling` → `suppression` (DeepFilterNet
default backend); `parametricEQ` → `eq` + `eq_enabled`; `noiseGate`/`automaticNoiseGate` → `gate`;
`volumeStabilizer` → `compressor`. `gain` and `highpass` are not used by Sonar voice presets (the
−12 dB 31 Hz lowshelf already removes rumble) → left at stage defaults (off). Where a preset enables
suppression but the DeepFilter/RNNoise plugin is unavailable, the engine degrades gracefully (warns,
chain still builds) — same as the existing `mic suppression` path.

## 6. Engine API

`crates/engine/src/engine.rs`:
- `apply_eq_preset(name, channel)` — extend lookup: user `config.eq_presets` first, then
  `presets::factory_eq_presets()`. Saving/deleting still only touch the user library. A factory and a
  user preset with the same name → user wins (override).
- `apply_mic_preset(&mut self, name: &str) -> Result<(), EngineError>` — find in
  `presets::factory_mic_presets()`; overlay its stage fields + `eq_enabled` + `eq` onto the active
  profile's `mic` (preserving `enabled`, `hw_mic`); persist; rebuild the mic chain live (reuse the
  existing mic-apply path used by `mic_set_*`); emit `MicPresetApplied { name }`.
- `EngineState` gains: `factory_eq_presets: Vec<EqPresetSnapshot>` and
  `mic_presets: Vec<MicPresetSnapshot { name, description }>` so both UIs can list built-ins without a
  round-trip. (User `eq_presets` snapshot already exists.)

## 7. Surfaces (parity)

- **Protocol/IPC** (`crates/client`, `src-tauri`): new `Request::ApplyMicPreset { name }` + Tauri
  command `mic_preset_apply`; built-in lists ride in `EngineState`. (Channel `eq_preset_apply` already
  exists and now resolves factory names too.)
- **CLI** (`crates/cli`): `eq preset list` shows a "Built-in" section + the user's saved presets; new
  `mic preset list` and `mic preset apply <name>`.
- **GUI:**
  - `EqPage.svelte`: a "Built-in" presets group (read-only, Apply only) alongside the existing
    save/apply/delete for user presets.
  - `MicPage.svelte`: a preset picker (dropdown or list) of the mic presets with Apply; shows the
    applied preset's description.

## 8. Research provenance

- **§5.1 Channel EQ — DONE (incorporated).** Grounded in oratory1990's measured fixed-band EQ for the
  Nova Pro Wireless (Bluetooth, ANC off) via AutoEq, cross-checked against multiple reviews. "Reference
  (Calibrated)" is the measurement verbatim; the others are measurement-anchored variants. Key sources:
  AutoEq/oratory1990 (`github.com/jaakkopasanen/AutoEq` → SteelSeries Arctis Nova Pro Wireless),
  EveZone FPS guide, TechPowerUp, HiFi Oasis, SoundGuys, Igor's Lab. Per-preset confidence
  (measurement-based vs informed-synthesis) is recorded in the plan.
- **§5.2 Mic — DONE (incorporated).** The Sonar voice-preset names were verified across the web
  (slurptech, prosettings.net, steelseries.com/lp/sonar-for-streamers) and the **exact EQ values lifted
  verbatim** from the bundled reference app's Sonar-replica preset JSONs (which use our 10 canonical
  frequencies). All 9 named Sonar voice presets ship as-is; "Streamer" is one ASM-original full-chain
  addition. Confidence: Sonar-name-verified + reference-app values (community replica, not SteelSeries
  source — values are acoustically coherent and name-matched).
- **Bonus discovery (future work):** the reference app also contains **326 game-specific `[Game]` EQ
  profiles** and 9 `[Chat]` profiles (Apex, Baldur's Gate 3, …). These use **arbitrary** center
  frequencies and Q (e.g. 920 Hz Q10, 3780 Hz Q4) and `highPass` filters our fixed-10-band model can't
  represent faithfully. Importing them is a separate feature requiring variable-frequency parametric
  bands — see §10.

## 9. Testing

- `presets.rs`: every factory preset has exactly 10 bands at the canonical freqs, valid `kind`, and
  gains within EQ bounds (a single table-driven test over both catalogs).
- `apply_eq_preset` resolves a factory name and applies it (MockRunner); user-name override wins.
- `apply_mic_preset` overlays stages + eq, **preserves `enabled` and `hw_mic`**, persists, rebuilds
  (MockRunner); unknown name → `BadRequest`.
- Protocol round-trip for `ApplyMicPreset`; CLI arg-parse tests; GUI unit tests for the preset lists +
  apply wiring. No test touches real audio.

## 10. Out of scope

- User-**saved** mic presets (only built-in mic presets ship now; channel user-saves already exist).
- **Game-specific `[Game]` profiles** (the 326 in the reference app) — they need arbitrary-frequency /
  high-Q / high-pass parametric bands the fixed-10-band model can't represent. A real future feature
  (variable-frequency EQ + a Sonar-preset importer).
- **Automatic EQ preamp/headroom.** §5.1 shows a recommended preamp per preset (to avoid clipping from
  large boosts), surfaced in the UI as info only. Auto-applying it (e.g. as a master gain offset) is a
  future enhancement; for now users lower channel volume if a boosted preset clips.
- Per-preset icons/artwork; import/export of preset packs; surround/HRIR presets.
- Auto-applying a preset on startup (presets are explicit user actions).
