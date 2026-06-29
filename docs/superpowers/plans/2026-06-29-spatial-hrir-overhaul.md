# Spatial Audio / HRIR Overhaul Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make virtual surround work out-of-the-box and deliver best-achievable positional audio, with a worked DayZ example — via an HRIR asset library (import/bundle/fetch), a hardware-sink dropdown, a footstep EQ preset, a ready-made DayZ profile, and a validated true-7.1→binaural path with Proton-negotiation fallback.

**Architecture:** Rust Cargo workspace (engine headless, UI-agnostic) + Tauri v2 + Svelte 5. Surround is a PipeWire `filter-chain` subprocess that convolves a HeSuVi 14-ch WAV. The engine owns all logic; `asm-cli` and the Tauri GUI are thin clients over the daemon (parity required). External tools (ffmpeg/curl/pw-dump) are invoked through the existing `CommandRunner` abstraction.

**Tech Stack:** Rust (engine/config/audio/cli/client crates), Tauri v2 (`src-tauri`), Svelte 5 runes + bits-ui (`frontend`), PipeWire 1.4.x filter-chain.

## Global Constraints

- **No device writes.** Device-write allowlist stays empty (G2). Never write OLED, never replay firmware opcodes.
- **No live audio writes in tests or by the assistant without explicit per-test consent** (project memory). Sink creation / DayZ launch live validation is **owner-run only** (Phase B gate, §Phase B preamble).
- **48 kHz only, no runtime resampling.** Any 44.1 kHz asset is converted at import time via an external tool subprocess, or skipped-with-reason — never resampled in the audio path.
- **GUI ⇄ CLI parity is mandatory.** Every spatial action exists in both `asm-cli` and the GUI.
- Reuse over duplication (G1); live changes without service restarts where feasible (G3); single source of truth (G4); small focused files (G6); typed errors, no `unwrap`/`expect`/`panic` on runtime paths (G7).
- `tauri` only in `src-tauri`; engine and below stay UI-agnostic.
- Canonical EQ model: fixed 10 bands `[31,62,125,250,500,1000,2000,4000,8000,16000]` Hz; band 0 lowshelf, band 9 highshelf, rest peaking.
- Bundled IRs must be permissively licensed; record provenance in `LICENSES/hrir/`. Proprietary IRs are NEVER committed to this repo (D2).
- Build/test: `cargo test --workspace` (Rust), `cd frontend && npm test` (vitest). Commit after each task.

---

## File Structure

**New files:**
- `crates/engine/src/hrir_catalog.rs` — code-defined catalog (stem → display/group/tonality/license/origin). [Task A1]
- `crates/engine/src/hrir_import.rs` — WAV header validation + import/resample/bundle helpers. [Tasks A3–A5]
- `crates/engine/src/factory_profiles.rs` — factory profile templates (DayZ). [Task A8]
- `assets/hrir/` — bundled permissive WAVs. [Task A5]
- `LICENSES/hrir/` — per-asset provenance/license. [Task A5]

**Modified files:**
- `crates/config/src/schema.rs` — `SurroundMode` enum + `mode`/`crossfeed` on `SurroundConfig`. [Task B1]
- `crates/engine/src/state.rs` — richer HRIR entries + `negotiated_channels`/`resolved_sink` snapshot fields. [Tasks A2, B3]
- `crates/engine/src/engine.rs` — import/fetch/ensure-bundled/factory-profile methods; apply_surround marker-desync fix; negotiation probe; fallback selection. [Tasks A4, A6, A8, A10, B2–B6]
- `crates/engine/src/convert.rs` — surround spec channel count (5.1/stereo variants). [Task B2/B5]
- `crates/audio/src/surround.rs` — 5.1 + stereo-bypass render paths; optional convolver2. [Tasks B5, B7]
- `crates/engine/src/presets.rs` — Footstep/Competitive EQ preset. [Task A7]
- `crates/client/src/protocol.rs` — `SurroundImportHrirs`/`SurroundFetchHrirs`/`ProfileCreateFromFactory` requests. [Tasks A6, A8]
- `crates/cli/src/daemon.rs` — dispatch new requests. [Tasks A6, A8]
- `crates/cli/src/main.rs` — CLI subcommands (parity). [Tasks A6, A8]
- `src-tauri/src/commands.rs` — Tauri commands. [Tasks A6, A8]
- `frontend/src/lib/ipc.ts` — IPC wrappers. [Tasks A6, A8]
- `frontend/src/lib/components/SpatialPage.svelte` — sink dropdown; import/fetch buttons. [Tasks A6, A9]

---

# Phase A — Quick wins (no live-audio risk)

## Task A1: HRIR catalog module

**Files:**
- Create: `crates/engine/src/hrir_catalog.rs`
- Modify: `crates/engine/src/lib.rs` (add `pub mod hrir_catalog;`)
- Test: inline `#[cfg(test)]` in `hrir_catalog.rs`

**Interfaces:**
- Produces: `pub struct HrirCatalogEntry { pub stem: &'static str, pub display: &'static str, pub group: &'static str, pub tonality: Tonality, pub license: License, pub origin: Origin }`, enums `Tonality { Dry, Roomy, Neutral }`, `License { Permissive, Proprietary }`, `Origin { Bundled, Import, Fetch }`; `pub fn catalog() -> &'static [HrirCatalogEntry]`; `pub fn entry_for(stem: &str) -> Option<&'static HrirCatalogEntry>`; `pub fn display_name(stem: &str) -> String` (catalog hit → display; else heuristic: strip leading `NN-`, replace `-`/`_` with spaces, title-case).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_stem_resolves_display_group_and_license() {
        let e = entry_for("04-gsx-sennheiser-gsx").expect("gsx in catalog");
        assert_eq!(e.display, "Sennheiser GSX");
        assert_eq!(e.group, "Sennheiser");
        assert!(matches!(e.license, License::Proprietary));
    }
    #[test]
    fn permissive_openal_is_marked_permissive_and_bundled() {
        let e = entry_for("07-oal+++-openal-max").expect("openal in catalog");
        assert!(matches!(e.license, License::Permissive));
        assert!(matches!(e.origin, Origin::Bundled));
    }
    #[test]
    fn unknown_stem_falls_back_to_humanized_name() {
        assert_eq!(display_name("12-foo-bar-baz"), "Foo Bar Baz");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-engine hrir_catalog -- --nocapture`
Expected: FAIL (module/functions not defined).

- [ ] **Step 3: Write minimal implementation**

```rust
//! Code-defined HRIR catalog: maps known HeSuVi profile stems to display name,
//! vendor group, tonality, license class, and shipping origin. Drives the picker
//! and gates which assets the app may bundle/redistribute (permissive only).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tonality { Dry, Roomy, Neutral }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum License { Permissive, Proprietary }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin { Bundled, Import, Fetch }

#[derive(Debug, Clone, Copy)]
pub struct HrirCatalogEntry {
    pub stem: &'static str,
    pub display: &'static str,
    pub group: &'static str,
    pub tonality: Tonality,
    pub license: License,
    pub origin: Origin,
}

const CATALOG: &[HrirCatalogEntry] = &[
    HrirCatalogEntry { stem: "07-oal+++-openal-max", display: "OpenAL (Max)", group: "OpenAL", tonality: Tonality::Dry, license: License::Permissive, origin: Origin::Bundled },
    HrirCatalogEntry { stem: "04-gsx-sennheiser-gsx", display: "Sennheiser GSX", group: "Sennheiser", tonality: Tonality::Dry, license: License::Proprietary, origin: Origin::Import },
    HrirCatalogEntry { stem: "06-cmss-game-creative-cmss3d", display: "Creative CMSS-3D", group: "Creative", tonality: Tonality::Dry, license: License::Proprietary, origin: Origin::Import },
    HrirCatalogEntry { stem: "05-sbx67-sbx-pro-studio", display: "Creative SBX Pro Studio", group: "Creative", tonality: Tonality::Neutral, license: License::Proprietary, origin: Origin::Import },
    HrirCatalogEntry { stem: "02-dh-dolby-headphone", display: "Dolby Headphone", group: "Dolby", tonality: Tonality::Roomy, license: License::Proprietary, origin: Origin::Import },
    HrirCatalogEntry { stem: "03-dht-dolby-atmos-headphones", display: "Dolby Atmos", group: "Dolby", tonality: Tonality::Roomy, license: License::Proprietary, origin: Origin::Import },
    HrirCatalogEntry { stem: "08-dtshx-dts-headphone-x", display: "DTS Headphone:X", group: "DTS", tonality: Tonality::Roomy, license: License::Proprietary, origin: Origin::Import },
    HrirCatalogEntry { stem: "10-waves-nx", display: "Waves NX", group: "Waves", tonality: Tonality::Roomy, license: License::Proprietary, origin: Origin::Import },
    HrirCatalogEntry { stem: "12-ssc-ny-sonic-studio-ny", display: "Sonic Studio NY", group: "Spatial Sound Card", tonality: Tonality::Roomy, license: License::Proprietary, origin: Origin::Import },
];

pub fn catalog() -> &'static [HrirCatalogEntry] { CATALOG }

pub fn entry_for(stem: &str) -> Option<&'static HrirCatalogEntry> {
    CATALOG.iter().find(|e| e.stem == stem)
}

pub fn display_name(stem: &str) -> String {
    if let Some(e) = entry_for(stem) { return e.display.to_string(); }
    // Heuristic fallback: strip leading "NN-", split on - and _, title-case.
    let no_prefix = stem.splitn(2, '-').nth(1).filter(|_| stem
        .split_once('-').map(|(h, _)| h.chars().all(|c| c.is_ascii_digit()) && !h.is_empty()).unwrap_or(false))
        .unwrap_or(stem);
    no_prefix
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| { let mut c = w.chars(); match c.next() { Some(f) => f.to_uppercase().chain(c).collect::<String>(), None => String::new() } })
        .collect::<Vec<_>>()
        .join(" ")
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p arctis-engine hrir_catalog -- --nocapture`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/hrir_catalog.rs crates/engine/src/lib.rs
git commit -m "feat(spatial): code-defined HRIR catalog (display/group/license/origin)"
```

---

## Task A2: Richer HRIR entries in the surround snapshot

**Files:**
- Modify: `crates/engine/src/state.rs` (extend `SurroundSnapshot`)
- Modify: `crates/engine/src/engine.rs:544-556` (populate new field)
- Modify: `frontend/src/lib/ipc.ts:27-` (mirror type) — wired visually in A9
- Test: inline in `engine.rs` surround state tests

**Interfaces:**
- Consumes: `hrir_catalog::display_name`, `hrir_catalog::entry_for` [A1], `convert::available_hrirs` (existing).
- Produces: on `SurroundSnapshot` a new field `pub available_hrir_entries: Vec<HrirEntrySnapshot>` where `pub struct HrirEntrySnapshot { pub stem: String, pub display: String, pub group: String, pub tonality: String }`. Keep existing `available_hrirs: Vec<String>` untouched (back-compat).

- [ ] **Step 1: Write the failing test** (in `engine.rs` test module)

```rust
#[test]
fn surround_snapshot_includes_display_entries_for_available_hrirs() {
    let dir = tempfile::tempdir().unwrap();
    let profiles = dir.path().join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(profiles.join("04-gsx-sennheiser-gsx.wav"), b"RIFF").unwrap();
    let entries = crate::engine::hrir_entries_for(dir.path());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].stem, "04-gsx-sennheiser-gsx");
    assert_eq!(entries[0].display, "Sennheiser GSX");
    assert_eq!(entries[0].group, "Sennheiser");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-engine surround_snapshot_includes_display -- --nocapture`
Expected: FAIL (`hrir_entries_for` / field not defined).

- [ ] **Step 3: Write minimal implementation**

Add to `state.rs`:
```rust
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HrirEntrySnapshot {
    pub stem: String,
    pub display: String,
    pub group: String,
    pub tonality: String,
}
```
Add `pub available_hrir_entries: Vec<HrirEntrySnapshot>` to `SurroundSnapshot` (and `#[serde(default)]` if it derives Deserialize; keep it in `Default`).

Add a free function in `engine.rs`:
```rust
pub(crate) fn hrir_entries_for(base_dir: &std::path::Path) -> Vec<crate::state::HrirEntrySnapshot> {
    crate::convert::available_hrirs(base_dir)
        .into_iter()
        .map(|stem| {
            let tonality = crate::hrir_catalog::entry_for(&stem)
                .map(|e| format!("{:?}", e.tonality)).unwrap_or_else(|| "Neutral".into());
            let group = crate::hrir_catalog::entry_for(&stem)
                .map(|e| e.group.to_string()).unwrap_or_default();
            crate::state::HrirEntrySnapshot {
                display: crate::hrir_catalog::display_name(&stem),
                group, tonality, stem,
            }
        })
        .collect()
}
```
In `Engine::state()` surround population (engine.rs ~L544), set `available_hrir_entries: hrir_entries_for(&base)` using the same `base` already computed for `available_hrirs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p arctis-engine surround_snapshot_includes_display -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/state.rs crates/engine/src/engine.rs
git commit -m "feat(spatial): expose HRIR display/group/tonality entries in snapshot"
```

---

## Task A3: HeSuVi WAV header validation

**Files:**
- Create: `crates/engine/src/hrir_import.rs`
- Modify: `crates/engine/src/lib.rs` (`pub mod hrir_import;`)
- Test: inline `#[cfg(test)]` in `hrir_import.rs`

**Interfaces:**
- Produces: `pub struct WavInfo { pub channels: u16, pub sample_rate: u32 }`; `pub fn read_wav_info(path: &std::path::Path) -> Result<WavInfo, EngineError>` (parses the RIFF `fmt ` chunk, no external deps); `pub fn is_importable(info: &WavInfo) -> ImportVerdict` where `pub enum ImportVerdict { Ready, NeedsResample, RejectChannels(u16) }` (Ready = 14ch & 48000; NeedsResample = 14ch & rate≠48000; RejectChannels otherwise).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn write_wav(path: &std::path::Path, channels: u16, rate: u32) {
        // Minimal canonical 16-bit PCM WAV header + zero data frames.
        let byte_rate = rate * channels as u32 * 2;
        let block_align = channels * 2;
        let data_len: u32 = 0;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());            // PCM
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&byte_rate.to_le_bytes());
        b.extend_from_slice(&block_align.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        std::fs::write(path, b).unwrap();
    }
    #[test]
    fn reads_channels_and_rate() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.wav");
        write_wav(&p, 14, 48000);
        let info = read_wav_info(&p).unwrap();
        assert_eq!(info.channels, 14);
        assert_eq!(info.sample_rate, 48000);
        assert!(matches!(is_importable(&info), ImportVerdict::Ready));
    }
    #[test]
    fn flags_44k_as_needs_resample_and_7ch_as_rejected() {
        let d = tempfile::tempdir().unwrap();
        let p1 = d.path().join("b.wav"); write_wav(&p1, 14, 44100);
        let p2 = d.path().join("c.wav"); write_wav(&p2, 7, 48000);
        assert!(matches!(is_importable(&read_wav_info(&p1).unwrap()), ImportVerdict::NeedsResample));
        assert!(matches!(is_importable(&read_wav_info(&p2).unwrap()), ImportVerdict::RejectChannels(7)));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-engine hrir_import -- --nocapture`
Expected: FAIL (not defined).

- [ ] **Step 3: Write minimal implementation**

```rust
//! HRIR WAV validation + import. Parses the RIFF header without external deps;
//! import copies HeSuVi 14-channel WAVs into the profiles dir, resampling 44.1→48
//! via an external tool subprocess (never in the audio path).
use crate::error::EngineError;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo { pub channels: u16, pub sample_rate: u32 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportVerdict { Ready, NeedsResample, RejectChannels(u16) }

pub fn read_wav_info(path: &Path) -> Result<WavInfo, EngineError> {
    let bytes = std::fs::read(path)
        .map_err(|e| EngineError::BadRequest(format!("cannot read {}: {e}", path.display())))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(EngineError::BadRequest(format!("not a RIFF/WAVE file: {}", path.display())));
    }
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let id = &bytes[i..i + 4];
        let sz = u32::from_le_bytes([bytes[i+4], bytes[i+5], bytes[i+6], bytes[i+7]]) as usize;
        if id == b"fmt " && i + 8 + 16 <= bytes.len() {
            let channels = u16::from_le_bytes([bytes[i+10], bytes[i+11]]);
            let sample_rate = u32::from_le_bytes([bytes[i+12], bytes[i+13], bytes[i+14], bytes[i+15]]);
            return Ok(WavInfo { channels, sample_rate });
        }
        i += 8 + sz + (sz & 1); // chunks are word-aligned
    }
    Err(EngineError::BadRequest(format!("no fmt chunk in {}", path.display())))
}

pub fn is_importable(info: &WavInfo) -> ImportVerdict {
    match (info.channels, info.sample_rate) {
        (14, 48000) => ImportVerdict::Ready,
        (14, _) => ImportVerdict::NeedsResample,
        (c, _) => ImportVerdict::RejectChannels(c),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p arctis-engine hrir_import -- --nocapture`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/hrir_import.rs crates/engine/src/lib.rs
git commit -m "feat(spatial): RIFF header validation + import verdict for HRIR WAVs"
```

---

## Task A4: Import HRIRs from a directory (engine method)

**Files:**
- Modify: `crates/engine/src/hrir_import.rs` (add `import_dir`)
- Test: inline `#[cfg(test)]` in `hrir_import.rs`

**Interfaces:**
- Consumes: `read_wav_info`, `is_importable` [A3]; the engine's `CommandRunner` for resample.
- Produces: `pub struct ImportReport { pub imported: Vec<String>, pub skipped: Vec<(String, String)> }` (skipped = (stem, reason)); `pub fn import_dir<R: arctis_audio::CommandRunner>(runner: &R, src: &Path, base_dir: &Path) -> Result<ImportReport, EngineError>`. Copies `Ready` WAVs into `<base_dir>/profiles/`; for `NeedsResample`, runs `ffmpeg -y -i <src> -ar 48000 <dst>` via runner (skip-with-reason if exit≠0 or ffmpeg missing); rejects non-14ch with a reason. Idempotent (overwrites same stem).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn import_copies_ready_skips_wrong_channels() {
    use arctis_audio::testing::MockRunner; // existing mock
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src"); std::fs::create_dir_all(&src).unwrap();
    let base = d.path().join("base");
    // a Ready 14ch/48k and a 7ch reject (reuse write_wav helper from A3 tests via super)
    super::tests::write_wav(&src.join("04-gsx.wav"), 14, 48000);
    super::tests::write_wav(&src.join("none.wav"), 7, 48000);
    let runner = MockRunner::new();
    let report = import_dir(&runner, &src, &base).unwrap();
    assert!(report.imported.contains(&"04-gsx".to_string()));
    assert!(base.join("profiles/04-gsx.wav").exists());
    assert!(report.skipped.iter().any(|(s, _)| s == "none"));
}
```
(Make `write_wav` `pub(crate)` in the A3 test module, or move it to a `#[cfg(test)] pub(crate) fn` in the module body.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-engine import_copies_ready -- --nocapture`
Expected: FAIL (`import_dir` not defined).

- [ ] **Step 3: Write minimal implementation**

```rust
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport { pub imported: Vec<String>, pub skipped: Vec<(String, String)> }

pub fn import_dir<R: arctis_audio::CommandRunner>(
    runner: &R, src: &Path, base_dir: &Path,
) -> Result<ImportReport, EngineError> {
    let profiles = base_dir.join("profiles");
    std::fs::create_dir_all(&profiles)
        .map_err(|e| EngineError::BadRequest(format!("cannot create profiles dir: {e}")))?;
    let mut report = ImportReport::default();
    let entries = std::fs::read_dir(src)
        .map_err(|e| EngineError::BadRequest(format!("cannot read import dir: {e}")))?;
    for ent in entries.filter_map(|e| e.ok()) {
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("wav") { continue; }
        let stem = match path.file_stem().and_then(|s| s.to_str()) { Some(s) => s.to_string(), None => continue };
        let info = match read_wav_info(&path) { Ok(i) => i, Err(e) => { report.skipped.push((stem, e.to_string())); continue; } };
        let dst = profiles.join(format!("{stem}.wav"));
        match is_importable(&info) {
            ImportVerdict::Ready => {
                std::fs::copy(&path, &dst).map_err(|e| EngineError::BadRequest(format!("copy failed: {e}")))?;
                report.imported.push(stem);
            }
            ImportVerdict::NeedsResample => {
                let out = runner.run("ffmpeg", &["-y", "-i", path.to_str().unwrap_or(""), "-ar", "48000", dst.to_str().unwrap_or("")]);
                match out {
                    Ok(o) if o.status == 0 => report.imported.push(stem),
                    _ => report.skipped.push((stem, "44.1kHz and ffmpeg resample unavailable".into())),
                }
            }
            ImportVerdict::RejectChannels(c) => report.skipped.push((stem, format!("{c}-channel WAV is not HeSuVi 14-channel"))),
        }
    }
    report.imported.sort();
    Ok(report)
}
```
(Confirm the `CommandRunner` trait path and `Output { status, .. }` shape match `crates/audio`; adjust `arctis_audio::CommandRunner` / `MockRunner` import to the real path used elsewhere in `engine.rs`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p arctis-engine import_copies_ready -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/hrir_import.rs
git commit -m "feat(spatial): import HeSuVi WAVs from a directory with resample/skip report"
```

---

## Task A5: Bundle permissive HRIRs + first-run install

**Files:**
- Create: `assets/hrir/00-neutral-openal.wav` (copy of the permissive OpenAL/CIAIR IR from the old-app collection: `/home/jj/Dev/Personal/sound-manager/Arctis-Sound-Manager/hrir/HRIR_wav_files/oal+++.wav`)
- Create: `assets/hrir/07-oal+++-openal-max.wav` (same source; second permissive option)
- Create: `LICENSES/hrir/README.md` (provenance: OpenAL-Soft/CIAIR, permissive; cite source)
- Modify: `crates/engine/src/hrir_import.rs` (add `ensure_bundled`)
- Test: inline in `hrir_import.rs`

**Interfaces:**
- Produces: `pub fn ensure_bundled(base_dir: &Path) -> Result<Vec<String>, EngineError>` — for each bundled asset embedded via `include_bytes!`, write it into `<base_dir>/profiles/<stem>.wav` only if absent; return the stems newly installed.

**Note (deviation from spec §5.1):** the bundled default is the **permissive OpenAL/CIAIR** IR (already in 14ch/48k WAV form, zero conversion). A MIT-KEMAR-derived 48k profile requires SOFA→HeSuVi-WAV conversion tooling and is deferred to the open-items list — flagged for the owner.

- [ ] **Step 1: Verify the source asset is permissive and 14ch/48k**

Run: `soxi /home/jj/Dev/Personal/sound-manager/Arctis-Sound-Manager/hrir/HRIR_wav_files/oal+++.wav`
Expected: `Channels: 14`, `Sample Rate: 48000`. (If sox absent: `ffprobe -hide_banner <file>`.)

- [ ] **Step 2: Write the failing test**

```rust
#[test]
fn ensure_bundled_installs_when_absent_and_is_idempotent() {
    let d = tempfile::tempdir().unwrap();
    let installed = ensure_bundled(d.path()).unwrap();
    assert!(installed.contains(&"00-neutral-openal".to_string()));
    assert!(d.path().join("profiles/00-neutral-openal.wav").exists());
    // second run installs nothing new
    let again = ensure_bundled(d.path()).unwrap();
    assert!(again.is_empty());
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p arctis-engine ensure_bundled -- --nocapture`
Expected: FAIL.

- [ ] **Step 4: Copy assets + write implementation**

```bash
mkdir -p assets/hrir LICENSES/hrir
cp "/home/jj/Dev/Personal/sound-manager/Arctis-Sound-Manager/hrir/HRIR_wav_files/oal+++.wav" assets/hrir/00-neutral-openal.wav
cp "/home/jj/Dev/Personal/sound-manager/Arctis-Sound-Manager/hrir/HRIR_wav_files/oal+++.wav" assets/hrir/07-oal+++-openal-max.wav
```
```rust
const BUNDLED: &[(&str, &[u8])] = &[
    ("00-neutral-openal", include_bytes!("../../../assets/hrir/00-neutral-openal.wav")),
    ("07-oal+++-openal-max", include_bytes!("../../../assets/hrir/07-oal+++-openal-max.wav")),
];

pub fn ensure_bundled(base_dir: &Path) -> Result<Vec<String>, EngineError> {
    let profiles = base_dir.join("profiles");
    std::fs::create_dir_all(&profiles)
        .map_err(|e| EngineError::BadRequest(format!("cannot create profiles dir: {e}")))?;
    let mut installed = Vec::new();
    for (stem, data) in BUNDLED {
        let dst = profiles.join(format!("{stem}.wav"));
        if !dst.exists() {
            std::fs::write(&dst, data).map_err(|e| EngineError::BadRequest(format!("write bundled HRIR: {e}")))?;
            installed.push((*stem).to_string());
        }
    }
    Ok(installed)
}
```
Write `LICENSES/hrir/README.md` documenting that these derive from OpenAL-Soft / CIAIR HRTFs (permissive), with a source link, and that proprietary IRs are user-supplied only. (Verify the `include_bytes!` relative path matches the crate layout depth; adjust `../` count.)

- [ ] **Step 5: Run test + wire first-run install**

Run: `cargo test -p arctis-engine ensure_bundled -- --nocapture` → PASS.
Then call `hrir_import::ensure_bundled(&base)` once during engine startup (where the engine first computes `hrir_base_dir()`), logging installed stems; ignore errors non-fatally (surround simply reports "no HRIR").

- [ ] **Step 6: Commit**

```bash
git add assets/hrir LICENSES/hrir crates/engine/src/hrir_import.rs crates/engine/src/engine.rs
git commit -m "feat(spatial): bundle permissive OpenAL HRIR + first-run install"
```

---

## Task A6: Wire import/fetch through protocol, daemon, CLI, Tauri, IPC (parity)

**Files:**
- Modify: `crates/client/src/protocol.rs:44` (add `SurroundImportHrirs { dir: Option<String> }`, `SurroundFetchHrirs`)
- Modify: `crates/engine/src/engine.rs` (`surround_import_hrirs(dir: Option<String>) -> Result<ImportReport, EngineError>`, `surround_fetch_hrirs() -> Result<ImportReport, EngineError>`)
- Modify: `crates/cli/src/daemon.rs:162-` (dispatch arms)
- Modify: `crates/cli/src/main.rs` (subcommands `surround import [dir]`, `surround fetch`)
- Modify: `src-tauri/src/commands.rs` (`surround_import_hrirs`, `surround_fetch_hrirs`)
- Modify: `frontend/src/lib/ipc.ts` (wrappers)
- Test: dispatch test in `crates/cli/src/daemon.rs` test module

**Interfaces:**
- Consumes: `hrir_import::import_dir`, `ensure_bundled` [A4/A5]. `surround_fetch_hrirs` downloads the HeSuVi pack with `curl -L -o <tmp>` via the runner then unzips into a temp dir and calls `import_dir`; on any failure returns a typed error (never panics). Default import `dir = None` → the old-app path constant `OLD_APP_HRIR_DIR`.
- Produces: `Request::SurroundImportHrirs`, `Request::SurroundFetchHrirs`; `ImportReport` serialized in the response (extend the response type or return updated `EngineState` plus a logged report — match the existing surround command return convention, which is `EngineState`).

- [ ] **Step 1: Write the failing dispatch test** (in `daemon.rs` tests, mirroring `handle_surround_set_channels_updates_state`)

```rust
#[test]
fn handle_surround_import_with_missing_dir_returns_ok_empty() {
    let mut engine = test_engine_with_temp_home(); // existing helper pattern
    let resp = handle_request(&mut engine, Request::SurroundImportHrirs { dir: Some("/nonexistent".into()) });
    // missing dir is a soft error surfaced as ok:false with a message, not a panic
    assert!(!resp.ok || resp.state.is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-cli handle_surround_import -- --nocapture`
Expected: FAIL (variant not defined).

- [ ] **Step 3: Implement** the protocol variants, engine methods, daemon arms (following the existing `Request::SurroundSetHwSink` arm shape at daemon.rs:175), CLI subcommands (mirror existing `surround` subcommands in `main.rs`), Tauri commands (mirror `surround_set_hw_sink` at commands.rs:286), and ipc.ts wrappers:
```typescript
export const surroundImportHrirs = (dir: string | null): Promise<EngineState> =>
  invoke<EngineState>("surround_import_hrirs", { dir });
export const surroundFetchHrirs = (): Promise<EngineState> =>
  invoke<EngineState>("surround_fetch_hrirs", {});
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p arctis-cli handle_surround_import -- --nocapture` → PASS, then `cargo test --workspace`.

- [ ] **Step 5: Commit**

```bash
git add crates/client/src/protocol.rs crates/engine/src/engine.rs crates/cli/src/daemon.rs crates/cli/src/main.rs src-tauri/src/commands.rs frontend/src/lib/ipc.ts
git commit -m "feat(spatial): import/fetch HRIR commands across daemon, CLI, Tauri, IPC"
```

---

## Task A7: Footstep / Competitive EQ preset

**Files:**
- Modify: `crates/engine/src/presets.rs` (add one `eqp(...)` entry)
- Test: inline test in `presets.rs`

**Interfaces:**
- Consumes: existing `eqp`, `ls`, `pk`, `pkq`, `hs` builders.
- Produces: a new entry in `factory_eq_presets()` named **"FPS / Footsteps (Competitive)"**. Distinct from the existing punchier "FPS / Footsteps"; gentler per spec §5.4 to preserve 5–10 kHz localization cues.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn competitive_footsteps_preset_present_and_gentle() {
    let p = factory_eq_presets();
    let fp = p.iter().find(|p| p.name == "FPS / Footsteps (Competitive)").expect("preset present");
    // gentle: no band exceeds +4 dB, sub-bass cut present
    assert!(fp.bands.iter().all(|b| b.gain_db <= 4.0));
    assert!(fp.bands.iter().any(|b| b.freq_hz == 62.0 && b.gain_db < 0.0));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-engine competitive_footsteps -- --nocapture`
Expected: FAIL.

- [ ] **Step 3: Add the preset** inside the `vec![...]` of `factory_eq_presets()`:

```rust
        eqp("FPS / Footsteps (Competitive)", "Gaming · preamp -3 dB",
            vec![ls(0.0), pk(62.0,-3.0), pkq(125.0,1.41,-2.0), pk(250.0,3.0), pk(500.0,0.0), pk(1000.0,0.0), pk(2000.0,3.0), pk(4000.0,2.0), pk(8000.0,0.0), hs(0.0)]),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p arctis-engine competitive_footsteps -- --nocapture` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/presets.rs
git commit -m "feat(eq): add gentle FPS/Footsteps (Competitive) factory preset"
```

---

## Task A8: Ready-made DayZ factory profile

**Files:**
- Create: `crates/engine/src/factory_profiles.rs` (`dayz_profile()`)
- Modify: `crates/engine/src/lib.rs` (`pub mod factory_profiles;`)
- Modify: `crates/client/src/protocol.rs` (`ProfileCreateFromFactory { template: String }`)
- Modify: `crates/engine/src/engine.rs` (`create_factory_profile(template: &str) -> Result<(), EngineError>`)
- Modify: `crates/cli/src/daemon.rs`, `crates/cli/src/main.rs`, `src-tauri/src/commands.rs`, `frontend/src/lib/ipc.ts`
- Test: inline in `factory_profiles.rs` + dispatch test in `daemon.rs`

**Interfaces:**
- Consumes: `presets::factory_eq_presets` (for the footstep bands), `SurroundConfig`, `ChannelConfig`, `Profile` from `arctis_config`.
- Produces: `pub fn dayz_profile(base_channels: &[ChannelConfig]) -> Profile` — clones the caller's current channel set (so node names match the live system), sets the Game channel EQ to the Footstep (Competitive) bands, sets `surround.enabled = true`, `surround.channels = vec!["game".into()]`, leaves chat/media direct, name = "DayZ". `create_factory_profile("DayZ")` builds it from the active profile's channels via `new_profile_from_active`-style insertion and switches to it.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn dayz_profile_enables_game_surround_with_footstep_eq() {
    let chans = vec![
        arctis_config::ChannelConfig { id: "game".into(), node_name: "Arctis_Game".into(), description: "Game".into(), output_device: None, eq: vec![], volume_db: 0.0, volume_pct: 100, muted: false },
        arctis_config::ChannelConfig { id: "chat".into(), node_name: "Arctis_Chat".into(), description: "Chat".into(), output_device: None, eq: vec![], volume_db: 0.0, volume_pct: 100, muted: false },
    ];
    let p = dayz_profile(&chans);
    assert_eq!(p.name, "DayZ");
    assert!(p.surround.enabled);
    assert_eq!(p.surround.channels, vec!["game".to_string()]);
    let game = p.channels.iter().find(|c| c.id == "game").unwrap();
    assert!(!game.eq.is_empty(), "game channel seeded with footstep EQ");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-engine dayz_profile -- --nocapture`
Expected: FAIL.

- [ ] **Step 3: Implement** `dayz_profile()`:

```rust
//! Factory profile templates. Built from the live channel set so node names match.
use arctis_config::{ChannelConfig, Profile, SurroundConfig};

pub fn dayz_profile(base_channels: &[ChannelConfig]) -> Profile {
    let footstep = crate::presets::factory_eq_presets()
        .into_iter().find(|p| p.name == "FPS / Footsteps (Competitive)")
        .map(|p| p.bands).unwrap_or_default();
    let channels = base_channels.iter().cloned().map(|mut c| {
        if c.id == "game" { c.eq = footstep.clone(); }
        c
    }).collect();
    Profile {
        name: "DayZ".into(),
        channels,
        routes: Vec::new(),
        mic: Default::default(),
        surround: SurroundConfig { enabled: true, hrir: None, channels: vec!["game".into()], hw_sink: None },
        master_volume_db: 0.0,
        master_volume_pct: 100,
        master_mute: false,
        chatmix_position: 4,
        default_sink_channel: Some("game".into()),
    }
}
```
Then add `create_factory_profile`, the protocol variant, daemon arm, CLI subcommand `profile create-factory DayZ`, Tauri command, and ipc wrapper (mirror Task A6's wiring).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p arctis-engine dayz_profile -- --nocapture` → PASS, then `cargo test --workspace`.

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/factory_profiles.rs crates/engine/src/lib.rs crates/client/src/protocol.rs crates/engine/src/engine.rs crates/cli/src/daemon.rs crates/cli/src/main.rs src-tauri/src/commands.rs frontend/src/lib/ipc.ts
git commit -m "feat(spatial): ready-made DayZ factory profile (game surround + footstep EQ)"
```

---

## Task A9: Hardware-sink dropdown + import/fetch UI (frontend)

**Files:**
- Modify: `frontend/src/lib/components/SpatialPage.svelte` (replace `hw_sink` text input ~L250–263; add Import/Fetch buttons; show display/group from `available_hrir_entries`)
- Test: `frontend/src/lib/components/SpatialPage.test.ts` (vitest, if a test harness exists; else manual checklist)

**Interfaces:**
- Consumes: `listOutputs()` [existing], `surroundSetHwSink` [existing], `surroundImportHrirs`/`surroundFetchHrirs` [A6], `available_hrir_entries` [A2]; `Select.svelte` (`options: {value,label}[]`, `value`, `onValueChange`).

- [ ] **Step 1: Write the failing test** (vitest) — render SpatialPage with a mocked `listOutputs` returning two sinks; assert the Select shows an "Auto-detect" option plus both device labels, and selecting "Auto-detect" calls `surroundSetHwSink(null)`.

```ts
import { render, fireEvent } from "@testing-library/svelte";
import SpatialPage from "./SpatialPage.svelte";
import * as ipc from "$lib/ipc";
vi.spyOn(ipc, "listOutputs").mockResolvedValue([
  { node_name: "alsa_output.arctis", description: "Arctis Nova Pro", is_default: true },
  { node_name: "alsa_output.hdmi", description: "HDMI", is_default: false },
]);
const setSink = vi.spyOn(ipc, "surroundSetHwSink").mockResolvedValue({} as any);
// ...render, open select, click Auto-detect, expect setSink called with null
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npm test -- SpatialPage`
Expected: FAIL.

- [ ] **Step 3: Implement** — load `listOutputs()` in an `$effect`/on mount into `let outputs = $state([])`; build `options = [{ value: "", label: \`Auto-detect (${resolved})\` }, ...outputs.map(o => ({ value: o.node_name, label: o.description || o.node_name }))]`; bind `value = surround?.hw_sink ?? ""`; `onValueChange = v => surroundSetHwSink(v === "" ? null : v)`. Replace the `<input type="text">` block with `<Select {options} {value} {onValueChange} disabled={!masterEnabled} ariaLabel="Hardware sink" />`. Add "Import my HRIRs" → `surroundImportHrirs(null)` and "Download HeSuVi set" → `surroundFetchHrirs()` buttons near the no-HRIR banner, with busy/result feedback. Render HRIR options from `available_hrir_entries` (display + group) instead of raw stems.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npm test -- SpatialPage` → PASS. Then `cd frontend && npm run check`.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/components/SpatialPage.svelte frontend/src/lib/components/SpatialPage.test.ts
git commit -m "feat(spatial): hardware-sink dropdown + HRIR import/fetch UI"
```

---

## Task A10: Fix hrir.wav / .active-profile desync

**Files:**
- Modify: `crates/engine/src/engine.rs` (`apply_surround` / wherever `hrir.wav` is written) — write the resolved WAV and `.active-profile` marker together, deriving the marker from the actually-applied stem.
- Test: inline in `engine.rs`

**Interfaces:**
- Produces: a helper `fn write_active_marker(base_dir: &Path, stem: &str) -> Result<(), EngineError>` invoked from the same code path that copies into `hrir.wav`, so the marker always names the file actually in use.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn applying_hrir_writes_consistent_marker_and_wav() {
    let d = tempfile::tempdir().unwrap();
    let profiles = d.path().join("profiles"); std::fs::create_dir_all(&profiles).unwrap();
    // two profiles; pick the second explicitly
    super::hrir_import::tests::write_wav(&profiles.join("00-a.wav"), 14, 48000);
    super::hrir_import::tests::write_wav(&profiles.join("07-b.wav"), 14, 48000);
    crate::engine::apply_active_hrir(d.path(), Some("07-b")).unwrap();
    let marker = std::fs::read_to_string(d.path().join(".active-profile")).unwrap();
    assert_eq!(marker.trim(), "07-b.wav");
    // hrir.wav content equals the chosen profile
    assert_eq!(std::fs::read(d.path().join("hrir.wav")).unwrap(), std::fs::read(profiles.join("07-b.wav")).unwrap());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p arctis-engine applying_hrir_writes_consistent -- --nocapture`
Expected: FAIL.

- [ ] **Step 3: Implement** `apply_active_hrir(base_dir, stem)` that resolves the path (reuse `convert::resolve_hrir_path`-style logic), copies it to `<base>/hrir.wav`, and writes `<base>/.active-profile` = `<resolved-stem>.wav` in the same call. Route the existing surround-apply path through it so the two can never diverge.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p arctis-engine applying_hrir_writes_consistent -- --nocapture` → PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/engine/src/engine.rs
git commit -m "fix(spatial): keep hrir.wav and .active-profile marker consistent"
```

---

# Phase B — Core 7.1 path + Proton negotiation (GATED)

> **GATE:** Tasks B2, B5, B6 finalize behavior that depends on what DayZ/Proton actually negotiates. The **owner** runs the live validation (spec §6) — the assistant must NOT create live sinks or launch the game without explicit per-test consent (project memory). B1, B3, B4 are pure logic/schema and are safe to implement immediately.

## Task B1: SurroundMode + crossfeed schema (additive)

**Files:**
- Modify: `crates/config/src/schema.rs` (`SurroundConfig`)
- Test: inline in `schema.rs`

**Interfaces:**
- Produces: `pub enum SurroundMode { Auto, Hrir71, Hrir51, StereoBypass }` (serde `rename_all = "snake_case"`, `Default = Auto`); on `SurroundConfig`: `#[serde(default)] pub mode: SurroundMode`, `#[serde(default)] pub crossfeed: u8` (0–100, clamped on validate). Old configs deserialize cleanly.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn surround_config_defaults_mode_auto_crossfeed_zero_and_roundtrips() {
    let c = SurroundConfig::default();
    assert!(matches!(c.mode, SurroundMode::Auto));
    assert_eq!(c.crossfeed, 0);
    let toml = toml::to_string(&c).unwrap();
    let back: SurroundConfig = toml::from_str(&toml).unwrap();
    assert_eq!(c, back);
    // old config without the fields still loads
    let old: SurroundConfig = toml::from_str("enabled = true\n").unwrap();
    assert!(matches!(old.mode, SurroundMode::Auto));
}
```

- [ ] **Step 2–5:** Run (FAIL) → add the enum + fields with serde defaults, extend `Default` and `recommended`, clamp `crossfeed` in the existing `validate` path → run (PASS) → commit `feat(config): SurroundMode + crossfeed on SurroundConfig`.

---

## Task B2: 8-channel surround input sink (present 7.1 to the game)

**Files:**
- Modify: `crates/audio/src/surround.rs` / `crates/engine/src/convert.rs` (ensure the surround input sink advertises `audio.channels = 8`, `audio.position = [FL FR FC LFE RL RR SL SR]` — already the case for the convolver input; the change is routing the *game* channel into an 8-ch sink rather than a stereo channel feeding an upmix)
- Test: render-conf snapshot asserting 8-channel capture position

**GATE:** exact routing (dedicated 8-ch Game sink vs. routing game straight into `effect_input.arctis_surround`) is chosen against the §6 negotiation result. Implement the render/snapshot now; confirm routing during validation.

- [ ] Steps: snapshot test of the 8-channel input conf → ensure render emits it → PASS → commit. (Live routing wired in B6 after validation.)

---

## Task B3: Negotiation probe (parse pw-dump for the game stream's channels)

**Files:**
- Modify: `crates/audio/src/` (add `parse_stream_channels(pw_dump_json, node_name) -> Option<u8>`)
- Modify: `crates/engine/src/state.rs` (`SurroundSnapshot.negotiated_channels: Option<u8>`, `resolved_sink: Option<String>`)
- Test: inline parser test against a captured `pw-dump` JSON fixture

**Interfaces:**
- Produces: `pub fn parse_stream_channels(dump: &str, sink_node: &str) -> Option<u8>` — finds the stream/link feeding `sink_node` and returns its negotiated `audio.channels` / `format` channel count.

- [ ] Steps: write a fixture-based test (8ch and 2ch cases) → FAIL → implement JSON parse (reuse the serde_json approach already used by `parse_output_sinks`) → PASS → populate snapshot fields in `state()` → commit `feat(spatial): probe negotiated channel count for the surround sink`.

---

## Task B4: Fallback selection (pure function)

**Files:**
- Modify: `crates/engine/src/engine.rs` (or a small `surround_mode.rs`)
- Test: inline

**Interfaces:**
- Produces: `pub fn resolve_effective_mode(cfg_mode: SurroundMode, negotiated: Option<u8>) -> SurroundMode` — `Auto` + Some(8)→`Hrir71`; +Some(6)→`Hrir51`; +Some(2)→`StereoBypass`; +None→`Hrir71` (optimistic until probed); explicit non-Auto passes through unchanged. **Never returns a mode that upmixes** (no "upmix" mode exists by construction).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn auto_mode_maps_negotiated_channels_to_path() {
    use arctis_config::SurroundMode::*;
    assert!(matches!(resolve_effective_mode(Auto, Some(8)), Hrir71));
    assert!(matches!(resolve_effective_mode(Auto, Some(6)), Hrir51));
    assert!(matches!(resolve_effective_mode(Auto, Some(2)), StereoBypass));
    assert!(matches!(resolve_effective_mode(StereoBypass, Some(8)), StereoBypass));
}
```

- [ ] **Steps 2–5:** Run (FAIL) → implement → PASS → commit `feat(spatial): effective-mode fallback from negotiated channels`.

---

## Task B5: 5.1 + stereo-bypass render paths

**Files:**
- Modify: `crates/audio/src/surround.rs` (render a 6-channel `[FL FR FC LFE RL RR]` convolution variant; render a stereo-bypass + optional crossfeed graph)
- Test: render-conf snapshots for both

**GATE:** crossfeed amount/feel confirmed in §6 A/B listening. Implement the graphs + snapshots now; default crossfeed stays 0 until validated.

- [ ] Steps: snapshot tests (6-ch conf node count; stereo-bypass with crossfeed=0 == passthrough, crossfeed>0 == cross-mix) → implement → PASS → commit.

---

## Task B6: Wire effective mode into apply_surround + upgrade DayZ profile

**Files:**
- Modify: `crates/engine/src/engine.rs` (`apply_surround` selects the render path from `resolve_effective_mode`; DayZ profile keeps `mode: Auto`)
- Test: engine test (MockRunner) asserting the chosen conf matches the mode for a given probed channel count

**GATE:** owner validation (§6) confirms Proton negotiates ≥6ch reliably; if not, set the DayZ profile default to `StereoBypass` + crossfeed + footstep EQ.

- [ ] Steps: MockRunner test feeding a fake pw-dump (8ch) asserts the 7.1 conf is spawned; (2ch) asserts stereo-bypass → implement selection → PASS → commit.

---

## Task B7 (optional): convolver2 graph simplification

**Files:**
- Modify: `crates/audio/src/surround.rs` (8× `convolver2` replacing 16× `convolver`)
- Test: existing surround render snapshots updated; assert binaural output parity via the fixture

- [ ] Steps: update render + the `crates/audio/tests/fixtures/surround_7_1_hesuvi.conf` fixture → ensure all surround tests pass → commit `perf(spatial): use convolver2 (8 nodes) for 7.1 HRIR graph`. Drop this task if Phase B timeline is tight.

---

## Final integration

- [ ] Run `cargo test --workspace` and `cd frontend && npm test && npm run check` — all green.
- [ ] Update `CLAUDE.md` build/run section if any new CLI subcommands changed the documented surface.
- [ ] Owner manually restarts the daemon (engine changed) and dev server (frontend changed) — note in the handoff; do not do live audio changes without consent.

---

## Self-Review Notes (author)

- **Spec coverage:** §5.1 catalog/import/bundle/fetch → A1–A6; §5.2 sink dropdown → A9; §5.3 8-ch + negotiation + fallback → B2/B3/B4/B6; §5.4 footstep EQ → A7; §5.5 DayZ profile → A8; §5.6 convolver2 → B7; §7 schema → B1/B3; §2 desync+44.1 bugs → A10 + A5. §6 live validation → Phase B gate preamble + Final integration.
- **Known deviation (flag to owner):** bundled default is permissive **OpenAL/CIAIR** (already 14ch/48k, no conversion) rather than a KEMAR/SADIE-derived WAV; SOFA→HeSuVi-WAV conversion for KEMAR/SADIE is deferred to open-items. This keeps Phase A dependency-free and legal.
- **Resampling:** done at import time via `ffmpeg` subprocess (CommandRunner), skip-with-reason if absent — honors "no runtime resampling".
- **Parity:** every new action (import, fetch, create-factory) added to protocol + daemon + CLI + Tauri + ipc.
- **No live audio in automated tests:** all Rust tests use `MockRunner`/temp dirs/fixtures; live work is owner-gated.
