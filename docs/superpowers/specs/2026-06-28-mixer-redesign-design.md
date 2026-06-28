# Mixer / Profiles Page Redesign — Design Spec

**Date:** 2026-06-28
**Status:** Draft for owner review.
**Refs:** `ARCHITECTURE.md` (G1–G10, esp. **G2 device-write safety**), `DESIGN.md` (Sonar look),
the validated-environment notes, and three research reports (Sonar UX, responsive Svelte slider,
ChatMix HID protocol) summarized inline.

## 1. Goal

Rebuild the Mixer (a.k.a. "profiles") page to match SteelSeries Sonar's layout/UX (non-streamer mode),
with a **simple, super-responsive 0–100% volume slider synced to the real sink volume**, a per-channel
**settings gear (output device) + EQ** action row, a **drag-and-drop apps block with full-text pills**,
a working **ChatMix** (software + the gated hardware-dial enable), and a **new-profile modal** triggered
top-right. Make better use of horizontal space — the current page is cramped.

## 2. Decisions (owner-approved)

- **D1 — Volume = real sink volume, 0–100%.** The per-channel slider controls that channel's actual
  PipeWire sink volume (Master = master/headset output, Game/Chat/Media/Aux = their virtual sinks,
  Mic = mic source), 0% bottom → 100% top. Replaces the dB slider. EQ keeps its own dB gains. The CLI
  volume commands switch to 0–100 to stay aligned.
- **D2 — ChatMix: research opcode → owner validates → enable.** The hardware dial needs a HID
  mode-enable opcode (found: `[0x06, 0x49, 0x01]`). It ships **gated** (allowlist empty) until the owner
  runs a one-time validation on real hardware; only then is it enabled. Software game/chat mixing is
  fixed regardless.
- **D3 — Slider component: bits-ui `Slider` v1.** Svelte 5-native, vertical orientation without the
  WebKitGTK `writing-mode` risk, built-in `onValueCommit`. New dependency. (Fallback if rejected: native
  `<input type=range>` with a `@supports` vertical fallback — same responsiveness architecture.)
- **D4 — Sonar non-streamer layout.** Single slider per channel (no streamer Personal/Stream split).

## 3. Non-negotiable constraints

- **G2 device-write safety:** never replay an unverified opcode; the write allowlist stays EMPTY until
  the owner validates `[0x06,0x49,0x01]` on this headset. Never write the OLED. One serialized writer.
- GUI ⇄ CLI ⇄ daemon **feature parity** (the volume model, chatmix, presets — all must match in CLI).
- 48 kHz; PipeWire subprocess model; engine UI-agnostic. Typed errors, no `unwrap`/`expect`/`panic` on
  runtime paths. No automated test performs a live audio write or a live device write.
- Build on the existing `--ss-*` design system / DESIGN.md; dark theme; per-channel accent colors.

## 4. Sub-project A — Volume model: 0–100% sink volume

**Current:** `set_channel_volume(channel, volume_db: -60..+6)` applies a software gain inside the channel
filter-chain (which already moves the sink's wpctl volume — verified: `channel volume game -12` set
`Arctis_Game` to 0.63). The dial-balance thread (chatmix) also calls `set_channel_volume` in dB.

**Target:**
- Engine: volume is a **percent 0–100** (≙ linear sink volume 0.0–1.0). A lightweight apply path sets the
  channel sink volume (reuse the existing Props/volume apply — it already moves the sink volume — but
  expressed in linear %, and WITHOUT any chain rebuild for responsiveness). Add a **read** of the current
  sink volume (parse from `pw-dump`, extending the existing sink parser) so the slider reflects reality
  and external changes (other apps / the hardware dial).
- `EngineState` channel/master/mic snapshots expose `volume_pct: u8` (0–100) (alongside or replacing
  `volume_db`). `MicSnapshot`/`MasterStrip` likewise.
- Config: store volume as percent (migrate existing `volume_db` → percent on load; document the
  migration). Master + per-channel.
- CLI: `channel volume <ch> <0-100>`, `master volume <0-100>` (and mic) — switch from dB to percent.
  Update help + tests.
- Dial-balance (`cli/src/dial.rs`): map the dial reading to game/chat **percent** sink volumes (see §5
  scale fix), consistent with this model.

**Responsiveness:** the GUI throttles commits (§7); the engine's volume-apply must be a single cheap
subprocess (no `reconcile`, no chain rebuild) returning fast.

## 5. Sub-project B — ChatMix (software + gated hardware enable)

**Root cause (found):** the headset only emits dial frames `[0x07,0x45]` (game level @ offset 2, chat @
offset 3, **0–100**) after it is put into game/chat mode by the init opcode **`[0x06,0x49,0x01]`** (64-byte
padded, interface 4). Our daemon **never sends any device-init sequence**, so the dial does nothing.
Secondary: our `dial.rs` assumes a 0–9 dial range; the device reports 0–100.

**Plan (gated, owner-validated):**
1. Add a **device-init / one-time `init_writes`** concept to the device descriptor + controller (a list
   of raw 64-byte reports sent once on attach). Include `[0x06,0x49,0x01]` (+ the companion `0x3b`/`0x8d`
   group the old app uses) — but **gated**: not sent until the opcode name is in `enabled_writes`
   (allowlist stays empty by default).
2. **Owner validation command** (read-mostly + one gated write): e.g.
   `asm-cli device chatmix-enable --validate` — sends `[0x06,0x49,0x01]` once and watches for incoming
   `[0x07,0x45]` frames, printing whether the dial now reports. The owner runs it, confirms the headset is
   fine + frames arrive, and signs off.
3. After sign-off: add `chatmix_enable` to `enabled_writes` in `HidOpener` and send it on daemon
   init/attach. Document the opcode's provenance (reverse-engineered USB capture; software mode switch;
   recoverable by replug) per G2.
4. Fix the **scale** in `dial.rs` (0–100, not 0–9) → map to game/chat percent volumes.
5. **Software ChatMix UI** (the on-screen slider) works regardless: it's a game↔chat balance that sets
   game/chat sink volumes; grey it out (with a "controlled by headset dial" hint) when the hardware dial
   is active (`dial_controls_balance` + device present), mirroring Sonar.

**Safety:** all device-write tests use mocks; no automated test sends a real HID write. The real opcode is
sent ONLY by the owner's explicit `--validate` run and (post-sign-off) the daemon init.

## 6. Sub-project C — Mixer page redesign (frontend)

Per the Sonar research + the owner's reference images (#2 = target mixer, #3 = output-device view).

### 6.1 Layout & space
- Channels as **flex columns** (`flex: 1; min-width: ~140px`), even gaps (~8–12px), using the full window
  width — no more cramming. Order: **Master · Game · Chat · Media · Aux · Mic**.
- Channel accent colors (top border + icon + slider fill): Master indigo/blue, Game green, Chat blue,
  Media pink/red, Aux purple, Mic orange (brand palette in DESIGN tokens).
- Remove the confusing lower section under the dB readout. A column is: **header → volume slider → mute →
  apps block**. The Routes/"remembered routes" table moves below or into a collapsible section (kept, but
  de-emphasized).

### 6.2 Channel column
- **Header:** accent icon + channel name (bold). An **action row**: a **settings gear** (opens the
  output-device popover, §6.3), the **EQ button** (next to the gear — opens that channel's EQ), and a
  remove/hide affordance for non-permanent channels (Master/Game/Chat/Mic permanent; Media/Aux/custom
  removable). (Optional active-EQ-preset label under the name — nice-to-have, can defer.)
- **Volume slider:** the new responsive vertical **`VolumeSlider`** (§7), 0–100%, accent fill, % readout,
  full-height grabbable.
- **Mute** toggle below the slider.
- **Apps block:** a drop target listing the app pills routed to this channel (§6.4).

### 6.3 Output-device settings (gear → popover)
A popover anchored to the gear (reference image #3): a "Playback device" dropdown populated from the
existing `list_outputs` command (already on master), showing the real sinks; selecting one calls
`set_channel_output`. Replaces the always-visible inline OUTPUT `<select>`. Master's gear sets the master
output device.

### 6.4 App pills (full text)
- Pill = `[app icon] [app name]` rounded chip in the channel accent tint. **Long names truncate with
  ellipsis AND show the full name on hover (title/tooltip)** — fixes the cut-off-with-no-recourse bug.
- Drag a pill between channels' apps blocks to re-route (existing drag/drop, restyled). Master holds the
  "apps to be routed" staging tray. MIC only accepts capture apps.

### 6.5 ChatMix
A **horizontal** ChatMix slider in a row **between/under Game and Chat** (game icon left, chat icon
right), bound to the software game/chat balance. Greyed with a hint when the hardware dial owns it.

### 6.6 Profiles & add-channel
- **Profile switcher** stays top-right (the existing `ProfilesDropdown`), but **"New profile" moves into a
  modal/dialog** triggered by a **+ button top-right** (name input + Create/Cancel). Rename/delete/export
  stay in the dropdown.
- **Add custom channel** moves out of the main strip row into the new-profile/again a small affordance
  (e.g. a "+" at the end of the strip row or in a menu) so the 6 standard channels are the focus.

## 7. Responsiveness architecture (the slider) — kills the lag

New `frontend/src/lib/components/VolumeSlider.svelte` (bits-ui `Slider`, vertical):
- **Instant visual:** local `value = $state` updated synchronously on `onValueChange` (every frame) — the
  thumb tracks the pointer with zero perceived lag.
- **Throttled commit:** an 80 ms trailing throttle calls the IPC volume-apply during drag; **flush
  immediately on `onValueCommit`** (pointer-up / Enter) so the final value always lands.
- **Reconcile guard:** a `$effect` reads the incoming `volume` prop (reactive dep) and writes local ONLY
  when not dragging — `if (untrack(() => !dragging)) value = [incoming]` — so engine echoes / the hardware
  dial / external volume changes never snap the slider back mid-drag.
- Errors surface via the existing mixer error banner (no silent revert).
This same component serves Master, the 4 channels, and Mic.

## 8. Testing
- Engine: percent volume apply + read (MockRunner); migration of `volume_db`→percent; dial scale 0–100
  mapping. CLI percent arg-parse. No live audio writes.
- Device: `init_writes` gating (a gated opcode is NOT sent unless enabled) — mock transport asserts no
  write occurs while the allowlist is empty; the `--validate` path is owner-run only (manual).
- Frontend: `VolumeSlider` reconcile-guard + throttle/flush logic (pure, fake timers); app-pill truncation
  + tooltip; output-device popover; chatmix grey-out; new-profile modal. Build warning-clean.
- Owner-run (manual, hardware): the ChatMix `--validate` opcode test; audible per-channel % volume;
  responsive drag.

## 9. Build order (phased; each phase shippable)
1. **A — Volume model** (engine % + read + CLI + config migration). Foundation for the slider.
2. **C1 — VolumeSlider component** (bits-ui + responsiveness architecture) + wire into a redesigned
   `ChannelStrip`/`MasterStrip`/`MicStrip`.
3. **C2 — Layout redesign** (flex columns, action row, gear→output popover, EQ button placement,
   app-pill full text, space).
4. **C3 — ChatMix UI** (horizontal slider, grey-out) + **profile modal** + add-channel relocation.
5. **B — ChatMix hardware** (init_writes + gated opcode + owner `--validate` + scale fix), then the
   owner-validation step, then enable.

(Phases A–C are frontend/engine and ship the visible redesign; phase B is the gated device work.)

## 10. Out of scope
- **Streamer mode** (the dual Personal/Stream sliders) — explicitly excluded by the owner.
- Importing Sonar's 326 game-specific EQ profiles (separate future feature).
- OLED / firmware writes; any device write beyond the single validated ChatMix-enable opcode.
