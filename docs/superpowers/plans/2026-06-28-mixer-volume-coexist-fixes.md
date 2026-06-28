# Mixer Volume Slider + IPC/Coexist Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Fix the Tauri camelCase IPC-arg bugs (channel/master volume, coexist disable, surround hw_sink),
stop the coexist detector from false-flagging our own sinks as a "legacy stack" (and from destroying
them), and completely rebuild the channel volume fader as a reliable vertical slider.

**Architecture:** Three independent fixes. (1) Frontend `ipc.ts`: send camelCase arg keys to match Tauri
v2's expected (Rust snake_case → JS camelCase) param names. (2) `crates/cli/src/coexist.rs`: drop the
node-name loopback heuristic (collides with our own `Arctis_*` sinks) and detect legacy only via
RPM-specific markers. (3) `ChannelStrip.svelte`: replace the opacity-0 overlay fader with one directly
styled vertical range input, live drag feedback + debounced commit.

**Tech Stack:** Tauri v2 + Svelte 5 (runes) + Vitest frontend; Rust workspace (`arctis-cli` coexist).

## Global Constraints

- **Tauri v2 invoke args must be camelCase** (Rust `volume_db` param ⇒ JS key `volumeDb`). Single-word
  params (`channel`, `device`, `name`) need no conversion; multi-word ones DO.
- No live audio writes in tests; no real daemon/pw-* in tests. Frontend build must be **warning-clean**.
- No `unwrap`/`expect`/`panic` on runtime paths. Reuse existing patterns (G1). Small focused files (G6).
- Coexist teardown must NEVER `pw-cli destroy` a node our own engine owns (`Arctis_Game/Chat/Media/Aux`).
- Commit trailers: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` and
  `Claude-Session: https://claude.ai/code/session_01329izwVKkyPS1uskubQC28`. Never `git add` `.superpowers/`,
  `.claude/`, or `*.rpm`.

## File Structure
- `frontend/src/lib/ipc.ts` — fix 4 arg objects to camelCase + the misleading comment + arg-builder tests.
- `crates/cli/src/coexist.rs` — remove node-name loopback detection; keep RPM/service/hrir markers; update tests.
- `frontend/src/lib/components/ChannelStrip.svelte` — rebuild the vertical fader.

---

### Task 1: Fix Tauri camelCase IPC argument keys

**Files:**
- Modify: `frontend/src/lib/ipc.ts`
- Test: `frontend/src/lib/*.test.ts` (the existing arg-builder tests)

**Interfaces:** Tauri commands expect camelCase: `set_channel_volume(channel, volumeDb)`,
`set_master_volume(volumeDb)`, `coexist_disable(dryRun)`, `surround_set_hw_sink(hwSink)`.

- [ ] **Step 1 — Update the failing arg-builder test(s) to assert camelCase.** Find the test for
  `buildSetChannelVolumeArgs` (it currently asserts `{ channel, volume_db }`). Change the expectation to
  `{ channel, volumeDb }`. If there are tests for the other three, update them too. Run them → they FAIL
  against the current (snake_case) code.

```ts
// expectation becomes:
expect(buildSetChannelVolumeArgs("game", -6)).toEqual({ channel: "game", volumeDb: -6 });
```

- [ ] **Step 2 — Run, verify fail:** `pnpm -C frontend test` → the updated assertion FAILS.
- [ ] **Step 3 — Fix the four arg sites in `ipc.ts`:**
  - `buildSetChannelVolumeArgs` → return `{ channel, volumeDb: volume_db }`; change its return type
    annotation to `{ channel: string; volumeDb: number }`; rename the param or keep `volume_db` input but
    map to `volumeDb`. Fix the misleading comment (it is NOT "pass through as-is").
  - `setMasterVolume` (~line 390) → `invoke("set_master_volume", { volumeDb })`.
  - `coexistDisable` (~line 443) → `invoke("coexist_disable", { dryRun })`.
  - `surroundSetHwSink` (~line 326) → `invoke("surround_set_hw_sink", { hwSink })`.
- [ ] **Step 4 — Run:** `pnpm -C frontend test && pnpm -C frontend build` → PASS, warning-clean.
- [ ] **Step 5 — Commit:** `fix(frontend): send camelCase IPC args (volumeDb, dryRun, hwSink) for Tauri v2`.

---

### Task 2: Stop coexist from false-flagging our own sinks

**Files:**
- Modify: `crates/cli/src/coexist.rs`
- Test: `crates/cli/src/coexist.rs` (`#[cfg(test)]`)

**Background:** `detect_from` flags any node named `Arctis_Game/Chat/Media` as a "legacy loopback", but
those are our OWN engine's channel-sink node names. On a clean system (no legacy RPM) this fires falsely,
and `teardown_plan` would `pw-cli destroy` those nodes — destroying the user's channels. Legacy must be
detected only by RPM-specific markers (the legacy systemd user services, `~/.local/bin/hrir-switch`, the
RPM package / its daemon), never by node names we also use.

- [ ] **Step 1 — Failing test:** with a `node_list_stdout` that contains `Arctis_Game`/`Arctis_Chat`/
  `Arctis_Media` but NO legacy service/script/RPM markers, `detect_from` must report a CLEAN system
  (`legacy_loopbacks` empty AND `warning()` returns `None`).

```rust
#[test]
fn our_own_sinks_are_not_flagged_as_legacy() {
    let nodes = "node.name = \"Arctis_Game\"\nnode.name = \"Arctis_Chat\"\nnode.name = \"Arctis_Media\"\n";
    let home = std::env::temp_dir().join(format!("asm_coexist_clean_{}", std::process::id()));
    let report = detect_from(nodes, &home);
    assert!(report.legacy_loopbacks.is_empty(), "our own Arctis_* sinks must NOT be legacy");
    assert!(warning(&report).is_none(), "a clean system must not warn");
}
```

- [ ] **Step 2 — Run, verify fail:** `~/.cargo/bin/cargo test -p arctis-cli our_own_sinks_are_not_flagged_as_legacy` → FAIL.
- [ ] **Step 3 — Implement:** Remove the `["Arctis_Game","Arctis_Chat","Arctis_Media"]` node-name scan from
  `detect_from` so `legacy_loopbacks` is no longer populated from node names. Detect legacy ONLY via the
  reliable markers already modeled: `~/.local/bin/hrir-switch` (`hrir_switch_present`), the legacy RPM
  daemon (`rpm_daemon_running`), and — if the function has access to do so cheaply — the legacy systemd
  user service files in `LEGACY_SERVICES`. Keep `legacy_loopbacks` as a field (so the struct/teardown
  compile) but only ever populate it from a node list when a REAL legacy marker is ALSO present (or leave
  it permanently empty and have `teardown_plan` rely on service/script teardown). Update `teardown_plan`
  so it does NOT emit `pw-cli destroy <node>` for our own sink names on a clean system. Update any existing
  test that asserted the old node-name detection (those tests encode the bug — flip them to the safe
  behavior; do not keep an assertion that our sinks are "legacy").
- [ ] **Step 4 — Run:** `~/.cargo/bin/cargo test -p arctis-cli` → green; `~/.cargo/bin/cargo test --workspace` once.
  (The pre-existing `handle_set_channel_volume_ok` flake under the full parallel run is unrelated — re-run
  `-p arctis-cli` isolated if it trips.)
- [ ] **Step 5 — Commit:** `fix(coexist): don't flag our own Arctis_* sinks as a legacy stack`.

---

### Task 3: Rebuild the channel volume fader (reliable vertical slider)

**Files:**
- Modify: `frontend/src/lib/components/ChannelStrip.svelte`
- Test: `frontend/src/lib/components/*.test.ts` (extract any pure helper, e.g. the existing `sliderPercent`,
  and unit-test it; component render covered by build)

**Current problem:** the fader is a transparent (`opacity:0`) vertical `<input type=range>` (80×24px,
rotated) overlaid on a separate decorative `.fader-track`/`.fader-thumb` driven only by `channel.volume_db`.
The hit-area is misaligned (hard to grab) and it commits only `onchange` (no live feedback). Rebuild it as
ONE directly-styled vertical range input that fills the whole fader area, with live drag feedback and a
debounced engine commit.

- [ ] **Step 1 — Failing test** for the pure value→percent mapping the fill uses (keep/realign
  `sliderPercent`): assert `sliderPercent(-60)===0`, `sliderPercent(6)===100`, `sliderPercent(0)≈90.9`
  (clamped to [0,100]). If it already exists, add the clamp + boundary cases.
- [ ] **Step 2 — Run, verify fail (if new assertions):** `pnpm -C frontend test`.
- [ ] **Step 3 — Rebuild the fader:**
  - Replace the three-element overlay (`.fader-track` + `.fader-thumb` + opacity-0 `.fader-input`) with a
    single vertical range input that IS the visible control: `writing-mode: vertical-lr; direction: rtl;`
    (so low=bottom, high=top), sized to fill the full fader height/width, styled via
    `::-webkit-slider-runnable-track` / `::-webkit-slider-thumb` (+ `::-moz-*`) so the track + thumb are
    visible and the WHOLE column is grabbable. A filled portion below the thumb can be drawn with a
    `linear-gradient` track background keyed off a local display value.
  - Add a local `displayDb = $state(channel.volume_db)`. On `oninput` (fires continuously while dragging):
    update `displayDb` immediately (instant visual), and commit to the engine **debounced** (~80 ms) via
    `setChannelVolume(channel.id, displayDb)`. On `onchange` (release): clear any pending debounce and
    commit the final value. On failure, surface via the existing `onError` prop AND revert `displayDb` to
    `channel.volume_db`.
  - **Reconcile guard:** while the user is actively dragging, ignore engine echoes — only sync `displayDb`
    from `channel.volume_db` when not dragging (mirror the EQ-editing guard idea: a `dragging` flag set on
    pointerdown/`oninput`, cleared on `onchange`/`pointerup`/`pointercancel`). This prevents the value from
    snapping back mid-drag.
  - Keep ARIA: `role`/`aria-label`/`aria-valuemin/max/now` reflect `displayDb`. Keyboard arrows work
    natively on the range input.
  - Keep the dB readout (`formatDb(displayDb)`) and the mute button.
- [ ] **Step 4 — Run:** `pnpm -C frontend test && pnpm -C frontend build` → PASS, warning-clean.
- [ ] **Step 5 — Commit:** `feat(frontend): rebuild channel volume fader as a reliable vertical slider`.

---

## Self-Review
- Coverage: IPC bug → T1 (all 4 sites); coexist false-positive + danger → T2; fader rebuild → T3. ✓
- The arg-builder tests are flipped from snake_case (which encoded the bug) to camelCase. ✓
- Coexist: no test left asserting our own sinks are "legacy"; teardown can't destroy our nodes on a clean system. ✓
- Fader: single styled input (no invisible overlay), live `oninput` + debounced commit + drag reconcile-guard,
  errors surfaced via `onError`. ✓
- No test touches real audio / real daemon. ✓
