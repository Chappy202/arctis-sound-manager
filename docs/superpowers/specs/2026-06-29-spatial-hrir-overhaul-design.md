# Spatial Audio / HRIR Overhaul — Design Spec

**Date:** 2026-06-29
**Status:** Approved decisions (owner D1–D6); one phase gated on a live hardware validation (see §6, §9).
**Refs:** `ARCHITECTURE.md` (G1–G10), `2026-06-20-arctis-sound-manager-design.md`,
`2026-06-28-eq-mic-preset-packs-design.md`, project memory (no-live-audio-writes-during-debug,
daemon-restart-after-engine-changes).

## 1. Goal

Make the spatial/HRIR feature deliver the **best achievable positional audio on the Arctis** and make it
work **out of the box**, with a concrete worked example: a **"DayZ" profile** tuned for footstep and
directional awareness on the Game channel. Specifically:

- Ship a real, usable **HRIR library** (today the feature has *nothing to convolve with* unless the user
  manually drops files in). Bundle the permissive IRs, import the user's existing local collection, and
  optionally fetch the full HeSuVi set.
- Drive DayZ (and any Enfusion/discrete-surround game) through a **true 7.1 → binaural** path, since the
  engine renders discrete surround sized to the device channel count. Degrade gracefully when Proton's
  channel negotiation falls short.
- Replace the free-text **hardware-sink** field with a proper device **dropdown**.
- Add a **Footstep/Competitive EQ preset** and a ready-made **DayZ profile**.

This is one spec, delivered as a **phased plan** (§8).

## 2. Background — validated findings (research, 2026-06-29)

These are the load-bearing facts the design rests on. Full sourcing lives in the research transcripts;
the conclusions are summarized here.

- **The surround DSP is already real, not stubbed.** `crates/audio/src/surround.rs` renders a PipeWire
  `filter-chain` that fans 8 input channels (7.1) through convolvers loading a HeSuVi 14-channel WAV and
  sums to stereo. `apply_surround()` in `crates/engine/src/engine.rs` (~L2269–2410) creates/removes the
  sink idempotently and routes channels. Per-profile (`Profile.surround: SurroundConfig`).
- **DayZ emits discrete surround and has no HRTF of its own.** The Enfusion engine uses XAudio2
  `SetOutputMatrix(srcChannels, destChannels, …)` to pan each source across however many channels the
  output device advertises; there is **no `HRTF`/`binaural` string in the binary** and no Steam
  Audio/Phonon/OpenAL shipped. There is no in-game speaker selector — it follows the OS device. The only
  stereo-spatial code is a crossfeed widener (`Stereo3DEffectAPO`), not HRTF.
  → A 7.1 sink presented to DayZ yields a genuine 7.1 bed for the convolver; **no double-processing** risk
  from the game. (The only conflicting virtualizers would be *external* ones — keep Sonar/Sonic off.)
- **Proton/FAudio is the weak link.** Under Proton, XAudio2 → FAudio → Wine → PipeWire channel
  negotiation is inconsistent and can cap at 5.1 or drop to stereo. So presenting 8ch is necessary but
  not sufficient — we must **detect what was actually negotiated** and fall back, never upmix.
- **Localization science (for tuning, not architecture):** 5–10 kHz spectral notches carry front/back &
  elevation cues — keep EQ gentle there. Footstep energy: ~150–450 Hz (weight), ~2–4 kHz (surface/"click"),
  ~4–5 kHz (clarity). Distance is carried by direct-to-reverberant ratio, so a *dry* HRIR is best for
  precision; "immersive/roomy" IRs blur direction. Generic HRTFs cause front/back confusion — make the
  HRIR **user-selectable** (already are). Head tracking would be the biggest future lever (out of scope).
- **HRIR inventory on this machine:** `hrir-switch` does **not** exist; the switching logic is the old
  Python app's `hrir_catalog.py` (copies a chosen WAV over `~/.local/share/pipewire/hrir_hesuvi/hrir.wav`).
  The full 57-file HeSuVi collection (14ch/48kHz, correct format) lives at
  `/home/jj/Dev/Personal/sound-manager/Arctis-Sound-Manager/hrir/HRIR_wav_files/` with an `info.csv`
  description index. The community-rated best *positional* sets (GSX, CMSS-3D, SBX) are in there.
- **Two pre-existing bugs found:** the live `hrir.wav` is the Atmos capture while `.active-profile` reads
  `00-default-asm` (state desync); and `00-default-asm.wav` is **44.1 kHz** — off-spec for the 48 kHz-only
  constraint.

## 3. Owner decisions

- **D1 — HRIR sourcing = import-local + bundle-permissive + optional-fetch.** The app **bundles only
  permissively-licensed IRs** in its own tree (OpenAL-Soft/CIAIR HRTFs, MIT KEMAR, a neutral passthrough).
  It provides **one-click import** of the user's existing local HeSuVi collection (so GSX/CMSS/Dolby/DTS
  etc. are available *on the user's machine* without the app redistributing them) and an **optional
  download** of the HeSuVi set for users who lack it.
- **D2 — Proprietary IRs are never redistributed.** Dolby, DTS, Creative/CMSS-3D, Sennheiser GSX, Razer,
  Waves, Nahimic, Spatial-Sound-Card, etc. are captures of proprietary virtualizers. They reach the app
  **only** via user import or user-initiated download — never committed to this repo or shipped in the
  installer. (Mirrors the old app, which downloaded rather than bundled.)
- **D3 — DayZ-class games use the 7.1 HRIR path,** validated against actual Proton negotiation, with
  graceful fallback to 5.1 HRIR or stereo bypass. **Never upmix** pre-folded stereo into the convolver.
- **D4 — Hardware sink = dropdown,** sourced from `list_outputs`, "Auto-detect" → `hw_sink = null`.
- **D5 — Ship a Footstep/Competitive EQ preset and a ready-made DayZ profile.** (Distance/Immersion knobs
  are **out of scope** — not selected by owner.)
- **D6 — One spec, phased plan.** Quick wins (asset import, sink dropdown, EQ preset, DayZ profile on the
  existing path) land before the trickier 8-channel/Proton-negotiation work that needs live validation.

## 4. Non-negotiable constraints

- **No device writes.** This feature is pure PipeWire/software; the device-write allowlist stays empty
  (G2). Never write the OLED, never replay firmware opcodes.
- **No live audio writes in tests or during debug without explicit per-test consent** (project memory).
  Sink creation, profile switching, and DayZ launch in §6 are **owner-run** validation steps, not
  automated, not performed by the assistant unprompted.
- **48 kHz only, no resampling at runtime.** Any 44.1 kHz asset (e.g. `00-default`) is converted to 48 kHz
  **at import/build time**, not in the audio path.
- **GUI ⇄ CLI parity** is mandatory — every spatial action (enable, pick HRIR, import, fetch, set sink,
  switch profile) must exist in both `asm-cli` and the GUI.
- Reuse over duplication (G1); live changes without service restarts where feasible (G3); single source of
  truth (G4); small focused files (G6); typed errors, no `unwrap`/`expect`/`panic` on runtime paths (G7).
- `tauri` only in `src-tauri`; engine and below stay UI-agnostic.
- **Licensing:** bundled IRs ship with their attribution (KEMAR cite Gardner & Martin; OpenAL-Soft/CIAIR
  per upstream). A `LICENSES/hrir/` directory records each bundled asset's provenance + license.

## 5. Components

### 5.1 HRIR asset library (Phase A)

A manifest-driven catalog, porting the old app's vendor grouping (`hrir_catalog.py`) to Rust.

- **Store layout** (unchanged base): `~/.local/share/pipewire/hrir_hesuvi/profiles/<stem>.wav` is the
  source of truth the engine already reads (`convert::available_hrirs`, `resolve_hrir_path`). Keep it.
- **Manifest:** a code-defined catalog (`crates/engine/src/hrir_catalog.rs`) mapping stem → display name,
  vendor group, "dry/positional" vs "roomy/immersive" hint, license/source, and `bundled|import|fetch`
  origin. Display names come from the catalog, not ad-hoc string munging (replaces
  `frontend/src/lib/surround.ts::hrirDisplayName` heuristics, which stay only as a fallback).
- **Bundled (permissive) assets:** committed under `assets/hrir/` (or fetched at build) and installed into
  `profiles/` on first run if absent: OpenAL/CIAIR (`oal+++` etc.), a neutral default, and a 48 kHz MIT
  KEMAR-derived profile. A new permissive **"00-neutral-48k"** replaces the off-spec 44.1 kHz default.
- **Import:** a command that copies HeSuVi WAVs from a chosen directory (default: the old-app path
  `/home/jj/Dev/Personal/sound-manager/Arctis-Sound-Manager/hrir/HRIR_wav_files/`) into `profiles/`,
  validating each is 14-channel and resampling 44.1→48 kHz if needed. Skips non-conforming files
  (`none.wav`/`razer.wav` are 7-channel) with a reported reason.
- **User drop folder:** `profiles/` itself remains the drop folder; the UI documents it (it already does).
- **Optional fetch:** a command that downloads the HeSuVi IR pack to `profiles/` (opt-in, networked,
  off by default). Surfaces progress/errors; never auto-runs.
- **Bug fixes:** (a) on apply, write `hrir.wav` and `.active-profile` atomically and consistently so they
  never desync; (b) convert/replace the 44.1 kHz default.

### 5.2 Hardware sink dropdown (Phase A)

- `SpatialPage.svelte`: replace the `<input type="text">` (the `hw_sink` field, ~L250–263) with a bits-ui
  `Select` (reuse `frontend/src/lib/ui/Select.svelte`).
- Options = `listOutputs()` results (`node_name` + `description`, `is_default` flagged) plus a top
  **"Auto-detect (Arctis)"** entry mapping to `hw_sink = null`. Selecting a device pins
  `hw_sink = node_name` via the existing `surroundSetHwSink`.
- Show the live auto-detected sink (`detect_headset_sink`) as the resolved target under the "Auto-detect"
  option so the user sees what it resolved to. No backend change required — `list_outputs` and
  `surround_set_hw_sink` already exist.

### 5.3 8-channel surround input + Proton negotiation & fallback (Phase B)

The core engineering item. Today the per-app routing channels are **stereo** virtual sinks; routing a
stereo Game channel into the 8-channel `effect_input.arctis_surround` would make PipeWire **upmix**, which
smears cues. For DayZ to render real 7.1, the sink it targets must advertise 8 channels.

- **Approach:** when a profile has surround enabled for the Game channel, the game must play into an
  **8-channel sink** (either the surround input sink directly, or an 8-channel Game channel sink), so
  Enfusion's `SetOutputMatrix` pans across 8 real channels → convolver → binaural.
- **Negotiation probe:** after the sink exists and the game is running, read the negotiated channel count
  of the game's stream (via `pw-dump`/`pw-cli` on the stream's `format`/`audio.channels`). Expose it in
  the surround snapshot so the UI can show "DayZ negotiated: 8ch / 6ch / 2ch".
- **Fallback policy:**
  - 8ch → 7.1 HRIR (ideal).
  - 6ch → **5.1 HRIR** variant (a 6-channel input position `[FL FR FC LFE RL RR]` convolution path).
  - 2ch → **stereo bypass** (no upmix): pass through with optional crossfeed + footstep EQ.
- **No automated live test.** Validation is the owner-run procedure in §6.

### 5.4 Footstep/Competitive EQ preset (Phase A)

Add to the factory EQ catalog (`crates/engine/src/presets.rs`, per the preset-packs spec). Gentle, high-Q
targeted so it doesn't disturb the 5–10 kHz localization band:

- `+3 dB @ 250 Hz` (peaking, weight/body — nearest band to 150–450 Hz),
- `+3 dB @ 2 kHz`, `+2 dB @ 4 kHz` (surface texture + "click"),
- `−3 dB @ 62 Hz`, `−2 dB @ 125 Hz` (reduce sub-bass masking from explosions),
- everything else flat. Name: **"FPS / Footsteps (Competitive)"** (reconcile with any existing FPS preset
  from the preset-packs spec — extend, don't duplicate).

### 5.5 Ready-made DayZ profile (Phase A/B)

A factory profile **"DayZ"** that wires:
- Game channel → surround **enabled**, dry/positional HRIR default (e.g. a GSX/CMSS-style set if imported,
  else the neutral bundled default),
- Game channel EQ = the Footstep preset,
- surround `channels = ["game"]` (chat/media stay direct so comms remain clean),
- master/chatmix sensible defaults.
Phase A ships it on the **current path** (works immediately with whatever HRIR is present); Phase B upgrades
its Game channel to the true 8-channel input once negotiation is validated.

### 5.6 (Optional) convolver2 graph simplification (Phase B)

Swap the 16 `convolver` nodes (2 per channel) for 8 `convolver2` nodes (one per channel, 2 IR outputs
each) — same binaural result, lighter graph, matches the canonical PipeWire HeSuVi conf. Gated behind the
existing surround test fixtures so output parity is proven before/after. Drop if it risks Phase B timeline.

## 6. Owner-run live validation (gated, consent-required)

Performed by the **owner**, not the assistant, per the no-live-audio-writes rule. The assistant prepares
exact commands and a checklist; the owner runs them and reports back.

1. Create the 8-channel surround sink (or enable the DayZ profile), confirm it appears (`pw-cli ls Node`).
2. Launch DayZ via Steam/Proton.
3. Read the DayZ stream's negotiated `audio.channels` (`pw-dump`); record 8/6/2.
4. A/B listen: 7.1-HRIR vs stereo-bypass for footstep directionality; record preference.
5. Confirm fallback triggers correctly if Proton caps below 8ch.

Outcomes feed the fallback thresholds and the DayZ profile default (§5.3, §5.5).

## 7. Data model changes

- `SurroundConfig` (`crates/config/src/schema.rs`) gains, additively (serde defaults, back-compat):
  - `mode: SurroundMode` — `Hrir71 | Hrir51 | StereoBypass | Auto` (default `Auto` = negotiate + fall back).
  - `crossfeed: u8` (0–100, default 0) — only applied on the stereo-bypass path.
  - (HRIR per-channel stays single — no per-channel HRIR; matches current behavior.)
- `SurroundSnapshot` (`crates/engine/src/state.rs`) gains `negotiated_channels: Option<u8>` and
  `resolved_sink: Option<String>` (what auto-detect resolved to) for the UI.
- New IPC/CLI: `surround_import_hrirs(dir)`, `surround_fetch_hrirs()`, surfaced in `asm-cli` too (parity).
- Catalog is code-defined (no schema field); user `profiles/` dir remains the runtime source of truth.

## 8. Phased plan

- **Phase A (quick wins, no live-audio risk):** HRIR catalog + import + bundle permissive + fix 44.1k/desync
  bugs (§5.1); sink dropdown (§5.2); Footstep EQ preset (§5.4); DayZ profile on current path (§5.5).
  Optional fetch (§5.1) if time allows.
- **Phase B (core, needs owner validation):** 8-channel input + negotiation probe + fallback (§5.3);
  `SurroundMode`/crossfeed model (§7); upgrade DayZ profile to true 7.1; optional convolver2 (§5.6).
- **Phase B is gated on §6.** If Proton can't negotiate ≥6ch reliably, the DayZ profile default becomes
  stereo-bypass + crossfeed + footstep EQ, and 7.1 stays available for titles that do negotiate.

## 9. Testing

- **Unit/snapshot (no live audio):** catalog parsing; import validation (channel count, 44.1→48 resample
  decision) via `MockRunner` + fixtures; render-conf snapshots for 7.1/5.1/stereo-bypass graphs; HRIR
  path resolution incl. the desync-fix (marker == file); fallback selection logic given a negotiated
  channel count; DayZ factory profile shape; Footstep preset values.
- **Frontend:** sink dropdown renders `listOutputs` + auto-detect entry; maps selection → `null`/node_name.
- **Live (owner-run only, §6):** real Proton negotiation + A/B listening. Never in CI.

## 10. Out of scope (YAGNI)

- Distance/Immersion knobs (owner declined).
- SOFA spatializer mode / runtime azimuth-elevation (libmysofa-dependent) — revisit later if wanted.
- Per-channel HRIR selection; head tracking; bundling proprietary IRs.

## 11. Open items

- Exact bundled permissive set (which CIAIR/OpenAL variants + the KEMAR build) — finalized in the plan.
- Whether the 8-channel game sink is the surround input directly or a dedicated 8ch Game channel —
  decided during Phase B implementation against the §6 negotiation result.
