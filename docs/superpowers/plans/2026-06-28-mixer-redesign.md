# Mixer / Profiles Page Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Tasks tagged **[parallel-safe: GROUP]** touch disjoint files and may run concurrently with other tasks of a different group.

**Goal:** Rebuild the mixer/profiles page to Sonar's non-streamer layout with a super-responsive 0–100%
sink-volume slider, per-channel settings(output)+EQ, full-text app pills, working ChatMix (software + a
gated, owner-validated hardware-dial enable), a new-profile modal, and a bits-ui headless-control
foundation. Full design detail: `docs/superpowers/specs/2026-06-28-mixer-redesign-design.md` (read it).

**Architecture:** Engine/CLI volume becomes percent-of-sink-volume (lightweight apply + read). Frontend
adopts `bits-ui` headless primitives (Slider/Select/Checkbox/Switch) and rebuilds the mixer layout +
a responsive `VolumeSlider`. ChatMix hardware-enable opcode ships gated until owner validation.

**Tech Stack:** Rust workspace (domain/config/engine/device/cli/src-tauri), Svelte 5 + Vite + Vitest +
bits-ui, PipeWire subprocesses, hidraw HID.

## Global Constraints
- **G2 device-write safety:** allowlist stays EMPTY; the ChatMix opcode `[0x06,0x49,0x01]` is sent ONLY by
  the owner's `--validate` run and (post-sign-off) daemon init. NO automated test sends a real HID write.
- **GUI ⇄ CLI ⇄ daemon parity** for volume (percent), chatmix, output device.
- Volume is **percent 0–100** (≙ linear sink volume 0.0–1.0). EQ keeps dB.
- No live audio writes in tests (MockRunner/fixtures/mock transport). Typed errors; no `unwrap`/`expect`/
  `panic` on runtime paths. Frontend build warning-clean. Reuse existing `--ss-*` tokens / DESIGN.md.
- Commit trailers: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` +
  `Claude-Session: https://claude.ai/code/session_01329izwVKkyPS1uskubQC28`. Never `git add` `.superpowers/`,
  `.claude/`, `*.rpm`.

---

## PHASE A — Volume model: dB → percent sink volume  (Rust) [parallel-safe: RUST]

### Task A1: domain/config percent volume + migration
**Files:** `crates/domain/src/eq_bounds.rs` (or a volume bounds module), `crates/config/src/schema.rs`,
`crates/config/src/migrate.rs`. **Test:** config tests.
- Add percent volume bounds (0..=100). Add a `volume_pct: u8` representation to `ChannelConfig` (and the
  master/mic equivalents). Provide a migration: existing `volume_db` (-60..+6) → percent via the same
  cubic/linear mapping wpctl uses (document the formula; 0 dB ≈ 100%? NO — 0 dB unity = 100%; pick the
  mapping: percent = clamp(round(10^(db/60? )...)). SIMPLER: since the slider now means "linear sink
  volume", define percent as the linear sink fraction; migrate old `volume_db` by `pct = round(100 *
  db_to_linear(db))` clamped 0..100, OR default everything to 100% and drop the dB field — decide in the
  task, document, and keep a back-compat `#[serde(default)]`/alias so old configs still load.
- [ ] TDD: a test that an old config with `volume_db` loads and yields a sane `volume_pct`; new configs
  round-trip percent. Commit: `feat(config): channel/master/mic volume as percent + migration`.

### Task A2: engine percent apply + read + state
**Files:** `crates/engine/src/engine.rs` (set_channel_volume/set_master_volume/mic; a lightweight
sink-volume apply + a read), `crates/engine/src/state.rs` (snapshots expose `volume_pct: u8`),
`crates/engine/src/convert.rs`, `crates/audio/src/*` (a cheap volume-set + a volume-read parsed from
`pw-dump`). **Test:** engine tests (MockRunner).
- `set_channel_volume(channel, pct)` applies the channel **sink** volume at `pct/100` linear via the
  cheapest path that already moves the sink volume (the current Props apply does — keep that, expressed in
  linear %, NO chain rebuild). Add `read_channel_volume`/include sink volume in the sinks parser so
  `state()` reports the live `volume_pct`. Master = master output device volume; Mic = mic source volume.
- [ ] TDD: setting percent applies the expected wpctl/Props argv; `state()` reports `volume_pct` parsed
  from a pw-dump fixture with a known sink volume. Commit: `feat(engine): percent sink volume apply+read+state`.

### Task A3: CLI percent + dial scale + parity
**Files:** `crates/cli/src/main.rs` (`channel volume <0-100>`, `master volume <0-100>`, mic), help text;
`crates/cli/src/dial.rs` (dial reading is **0–100**, map to game/chat percent, drop the 0–9 assumption).
**Test:** cli arg-parse + dial mapping tests.
- [ ] TDD: `channel volume game 50` parses to percent 50; `dial_to_channel_volumes` maps a 0–100 dial
  reading to game/chat percent correctly. Commit: `feat(cli): percent volume args + 0-100 dial scale`.

---

## PHASE C0 — bits-ui headless foundation + control migration (frontend) [parallel-safe: FE-FOUNDATION]

### Task C0a: add bits-ui + wrapper primitives
**Files:** `frontend/package.json` (+ `pnpm install bits-ui`), `frontend/src/lib/ui/Select.svelte`,
`Checkbox.svelte`, `Switch.svelte` (NEW — thin styled wrappers over bits-ui, using `--ss-*` tokens).
**Test:** a render/behavior test per wrapper.
- [ ] Add bits-ui; build styled `Select`, `Checkbox`, `Switch` wrappers matching the current visual
  language (dark, accent). Tests assert value-change callbacks fire. Build warning-clean.
  Commit: `feat(frontend): bits-ui + styled Select/Checkbox/Switch primitives`.

### Task C0b: migrate existing controls to the primitives
**Files:** inventory + migrate every existing native `<select>`/checkbox/toggle: `ChannelStrip.svelte`
(output select — note this is replaced by C3a's popover, coordinate), `MasterStrip.svelte` (auto-route
checkbox + default toggle), `DevicePage.svelte` (selects, segmented controls), `ProfilesDropdown` (if
applicable), `MicPage`/`SpatialPage` selects. **Test:** existing component tests stay green.
- [ ] Replace native controls with the C0a wrappers; keep behavior identical; build warning-clean.
  Commit: `refactor(frontend): migrate selects/checkboxes/switches to bits-ui primitives`.
  (Depends on C0a. Independent of Phase A/B.)

---

## PHASE C1 — Responsive VolumeSlider (frontend) [needs C0a + A2 state shape]

### Task C1: VolumeSlider.svelte
**Files:** `frontend/src/lib/components/VolumeSlider.svelte` (NEW). **Test:** unit tests (fake timers).
**Interface:** `Props { volume: number /*0-100*/, oncommit: (v:number)=>void, onError?:(m:string)=>void,
label?: string, accent?: string }`. Vertical bits-ui `Slider`.
- Implement per spec §7: local `value=$state`, `dragging=$state`; `onValueChange` → set local + 80ms
  trailing throttle `scheduleCommit`; `onValueCommit` → `flushCommit`; reconcile `$effect` with
  `if (untrack(()=>!dragging)) value=[volume]`. Accent fill, % readout, full-height grabbable, ARIA.
- [ ] TDD: throttle schedules one commit per 80ms during rapid changes; flush fires immediately on commit;
  reconcile updates local when not dragging and does NOT while dragging. Build warning-clean.
  Commit: `feat(frontend): responsive VolumeSlider (bits-ui, throttle+flush+reconcile guard)`.

---

## PHASE C2 — Channel strip + layout redesign (frontend) [needs C1, C0; do strips then page]

### Task C2a: redesigned ChannelStrip
**Files:** `frontend/src/lib/components/ChannelStrip.svelte`. **Test:** component utils tests.
- Header: accent icon + name; **action row** = settings gear (opens output popover — C3a), EQ button
  (next to gear), remove/hide (non-permanent only). Body: `VolumeSlider` bound to `channel.volume_pct`
  with `oncommit=(p)=>setChannelVolume(channel.id,p)`; mute below; apps block (drop target). Remove the
  old dB fader + the confusing sub-section. Errors via the existing onError banner.
- [ ] TDD + build. Commit: `feat(frontend): redesigned ChannelStrip (gear+EQ row, % slider, apps block)`.

### Task C2b: MasterStrip + MicStrip
**Files:** `MasterStrip.svelte`, `MicStrip.svelte`. Use `VolumeSlider` (master output volume / mic source
volume), the gear (master output device / mic hw device), consistent header.
- [ ] TDD + build. Commit: `feat(frontend): Master/Mic strips on VolumeSlider + consistent header`.

### Task C2c: MixerPage layout
**Files:** `MixerPage.svelte`. Flex columns (`flex:1; min-width:140px`), full width, accent borders,
order Master·Game·Chat·Media·Aux·Mic, routes table moved to a collapsible section, add-channel relocated
(small affordance at row end). De-cramp spacing per spec §6.1.
- [ ] TDD + build. Commit: `feat(frontend): mixer layout — flex columns, full width, de-cramped`.

---

## PHASE C3 — Popover / pills / chatmix / profile modal (frontend) [parallel-safe within C3 by file]

### Task C3a: output-device popover (gear)  [file: a new Popover + ChannelStrip hook]
Popover (bits-ui or a small custom) anchored to the gear: "Playback device" Select (from `listOutputs`)
→ `setChannelOutput`. Replaces the inline OUTPUT select. **Commit:** `feat(frontend): output-device popover`.

### Task C3b: app pills full-text  [file: AppPill.svelte]
Pill = icon + name, **truncate + `title` tooltip showing the full name**; accent tint; keep drag/drop.
**Commit:** `fix(frontend): app pills show full name on hover (truncate+tooltip)`.

### Task C3c: ChatMix horizontal slider  [file: ChatmixSlider.svelte + MixerPage placement]
Horizontal bits-ui Slider between Game/Chat (game icon left, chat right); bound to `set_chatmix`; greyed
with a "controlled by headset dial" hint when `dial_controls_balance && device_present`. **Commit:**
`feat(frontend): horizontal ChatMix slider with hardware-dial grey-out`.

### Task C3d: new-profile modal + add-channel relocation  [file: a Dialog + ProfilesDropdown/MixerPage]
A top-right **+** opens a bits-ui `Dialog` (name input + Create/Cancel → `profileNew`). Move add-custom-
channel to a small end-of-row affordance. **Commit:** `feat(frontend): new-profile modal + relocate add-channel`.

---

## PHASE B — ChatMix hardware enable (gated, owner-validated)  (Rust) [parallel-safe: RUST, after A]

### Task B1: device init_writes concept + gated opcode
**Files:** `crates/device/*` (descriptor: a `init_writes: Vec<[u8;N]>` or named one-time writes; the
controller sends them on attach ONLY if the name is in `enabled_writes`), the Nova Pro descriptor TOML
(add `chatmix_enable = [0x06,0x49,0x01]` + companions, gated). **Test:** mock transport asserts the gated
opcode is NOT sent while the allowlist is empty; IS sent (to the mock) when enabled.
- [ ] TDD + commit: `feat(device): gated one-time init writes + ChatMix-enable opcode (allowlist-gated)`.

### Task B2: owner-validation command
**Files:** `crates/cli/src/main.rs` + daemon: `asm-cli device chatmix-enable --validate` — sends the
opcode once (this is the OWNER-RUN gated write) and watches for `[0x07,0x45]` frames for a few seconds,
printing whether the dial now reports. **Test:** arg-parse + the watch-logic with a mock frame source.
- [ ] TDD + commit: `feat(cli): owner-run ChatMix validation command`.

### Task B3: scale fix + post-validation enable wiring
**Files:** `crates/cli/src/dial.rs` (already 0–100 from A3 — verify), `crates/cli/src/daemon.rs`
(`HidOpener` enabled_writes: add `chatmix_enable` BEHIND a clearly-commented OWNER-RUN gate that stays
DISABLED by default until the owner signs off; send the init on attach when enabled).
- [ ] Commit: `feat(daemon): wire ChatMix-enable on init behind the OWNER-RUN gate (disabled by default)`.

---

## Execution / parallelization notes
- **Wave 1 (parallel):** Phase A (RUST) ∥ Phase C0 (FE-FOUNDATION) — fully disjoint (Rust vs frontend).
- **Wave 2:** C1 (needs C0a + A2 state shape).
- **Wave 3:** C2a/b/c (sequential-ish; same files region) ∥ Phase B (RUST, after A).
- **Wave 4 (parallel within C3 by file):** C3a, C3b, C3c, C3d touch mostly different files.
- Owner-run validation of the ChatMix opcode happens between B2 and enabling B3.

## Self-Review
- Volume %: A1–A3 (config/engine/cli) keep CLI/daemon aligned. ✓
- ChatMix: software (C3c + A3 scale) + gated hardware (B1–B3 + owner validate). Allowlist empty by default. ✓
- Redesign: C0 (primitives+migration), C1 (slider), C2 (strips+layout), C3 (popover/pills/chatmix/modal). ✓
- No automated test does a live audio or HID write. ✓  bits-ui added once (C0a) before consumers. ✓
