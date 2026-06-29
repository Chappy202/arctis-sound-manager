# Daemon-Unavailable View Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the 3 divergent daemon-unavailable treatments + 2 gap pages with ONE shared `<DaemonUnavailable>` component (with Start-daemon + Retry), rendered consistently by every page.

**Architecture:** A pure `viewFor(connectionStatus)` helper + a thin `DaemonUnavailable.svelte` driven by the existing `connectionStatus`/`loadError` stores and the existing `daemonStart`/`reconnect` calls. Each page gates its content on `$connectionStatus !== "connected"` and deletes its bespoke daemon-down/loading markup + CSS. The AppShell runtime banner is removed; the topbar dot stays.

**Tech Stack:** Svelte 5 runes, existing Svelte stores (`stores/connection.ts`, `stores.ts`), `--ss` design tokens, vitest (no jsdom).

## Global Constraints

- Frontend-only; NO Rust/engine/Tauri changes. Reuse existing `connectionStatus` (writable `"connecting"|"connected"|"disconnected"`), `loadError`, `reconnect()` (from `src/lib/stores/connection.ts` + `src/lib/stores.ts`), and `daemonStart()` (from `src/lib/ipc.ts`).
- NO jsdom/testing-library: logic in a `.ts` helper + vitest; `.svelte` files are thin views (owner-manual-verify visuals).
- `--ss` design tokens only; reuse existing button styles; no ad-hoc hex unless no token exists.
- Reuse over duplication (G1): exactly ONE component + ONE helper; the 5 pages + AppShell must NET SHRINK (delete bespoke markup/CSS, don't add parallel copies).
- Canonical copy (single source, use verbatim): disconnected title **"Daemon not running"**; body **"The Arctis Sound Manager service isn't reachable, so changes won't apply."**; buttons **"Start daemon"** / **"Retry"**; connecting **"Connecting to daemon…"**.
- "Clean up as you go": each page edit removes its now-orphaned CSS; a final sweep removes leftover dead styles + the deferred `extern crate libc`.
- Build/test: `cd frontend && npm test`, `npm run check` (0 errors), `npm run build` (Svelte compile gate). Commit after each task.

---

## File Structure

**New:**
- `frontend/src/lib/daemonUnavailable.ts` — `viewFor` helper. [T1]
- `frontend/src/lib/daemonUnavailable.test.ts` — vitest. [T1]
- `frontend/src/lib/components/DaemonUnavailable.svelte` — the shared view. [T2]

**Modified (net shrink):**
- `MixerPage.svelte`, `DevicePage.svelte`, `SpatialPage.svelte` — gate on the shared component, delete bespoke daemon-down/loading. [T3]
- `AppShell.svelte` — remove the runtime reconnect banner + CSS. [T3]
- `EqPage.svelte`, `MicPage.svelte` — add the gate (previously no handling). [T4]
- `src-tauri/src/daemon_control.rs` — remove redundant `extern crate libc;`. [T5]

---

## Task 1: `viewFor` helper + test

**Files:**
- Create: `frontend/src/lib/daemonUnavailable.ts`
- Create: `frontend/src/lib/daemonUnavailable.test.ts`

**Interfaces:**
- Consumes: `ConnectionStatus` from `./stores/connection.js`.
- Produces: `viewFor(status: ConnectionStatus): "connecting" | "disconnected" | "hidden"`.

- [ ] **Step 1: Write the failing test**

```ts
import { describe, it, expect } from "vitest";
import { viewFor } from "./daemonUnavailable";
describe("viewFor", () => {
  it("maps store status to sub-view", () => {
    expect(viewFor("connecting")).toBe("connecting");
    expect(viewFor("disconnected")).toBe("disconnected");
    expect(viewFor("connected")).toBe("hidden");
  });
});
```

- [ ] **Step 2: Run → FAIL**

Run: `cd frontend && npm test -- daemonUnavailable`
Expected: FAIL (module not found).

- [ ] **Step 3: Implement**

```ts
import type { ConnectionStatus } from "./stores/connection.js";

/** Which sub-view DaemonUnavailable renders for a given connection status. */
export function viewFor(status: ConnectionStatus): "connecting" | "disconnected" | "hidden" {
  if (status === "connected") return "hidden";
  return status; // "connecting" | "disconnected"
}
```

- [ ] **Step 4: Run → PASS.** `cd frontend && npm run check`.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/daemonUnavailable.ts frontend/src/lib/daemonUnavailable.test.ts
git commit -m "feat(ui): viewFor helper for unified daemon-unavailable view"
```

---

## Task 2: `DaemonUnavailable.svelte` shared component

**Files:**
- Create: `frontend/src/lib/components/DaemonUnavailable.svelte`
- Test: none (thin view; logic in T1) — owner-manual-verify

**Interfaces:**
- Consumes: `viewFor` [T1]; stores `connectionStatus` (`./stores/connection.js`), `loadError` (`./stores.js`); `reconnect` (`./stores/connection.js`); `daemonStart` (`./ipc.js`).

- [ ] **Step 1: Build the component**

```svelte
<script lang="ts">
  import { connectionStatus, reconnect } from "../stores/connection.js";
  import { loadError } from "../stores.js";
  import { daemonStart } from "../ipc.js";
  import { viewFor } from "../daemonUnavailable.js";

  const view = $derived(viewFor($connectionStatus));
  let busy = $state(false);
  let actionError = $state<string | null>(null);

  async function onStart() {
    busy = true; actionError = null;
    try { await daemonStart(); await reconnect(); }
    catch (e) { actionError = String(e); }
    finally { busy = false; }
  }
  async function onRetry() {
    busy = true; actionError = null;
    try { await reconnect(); }
    catch (e) { actionError = String(e); }
    finally { busy = false; }
  }
</script>

{#if view === "connecting"}
  <div class="du-card" role="status" aria-live="polite">
    <div class="du-spinner" aria-hidden="true"></div>
    <p class="du-title">Connecting to daemon…</p>
  </div>
{:else if view === "disconnected"}
  <div class="du-card" role="alert" aria-live="assertive">
    <span class="du-icon" aria-hidden="true">◉</span>
    <p class="du-title">Daemon not running</p>
    <p class="du-body">The Arctis Sound Manager service isn't reachable, so changes won't apply.</p>
    {#if $loadError}<p class="du-error-detail">{$loadError}</p>{/if}
    <div class="du-actions">
      <button class="du-btn du-btn--primary" onclick={onStart} disabled={busy}>Start daemon</button>
      <button class="du-btn" onclick={onRetry} disabled={busy}>Retry</button>
    </div>
    {#if actionError}<p class="du-action-error">{actionError}</p>{/if}
  </div>
{/if}

<style>
  .du-card {
    display: flex; flex-direction: column; align-items: center; justify-content: center;
    gap: var(--ss-space-3); text-align: center;
    margin: auto; max-width: 460px; padding: var(--ss-space-6);
    background: var(--ss-surface-1); border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-md); color: var(--ss-text-primary);
  }
  .du-icon { font-size: 28px; color: var(--ss-danger); }
  .du-title { font-family: var(--ss-font-display); font-size: var(--ss-type-h3-size, 18px); margin: 0; }
  .du-body { color: var(--ss-text-secondary); margin: 0; }
  .du-error-detail { font-family: var(--ss-font-mono); font-size: var(--ss-type-caption-size, 12px); color: var(--ss-text-disabled); margin: 0; }
  .du-actions { display: flex; gap: var(--ss-space-2); margin-top: var(--ss-space-2); }
  .du-btn {
    height: var(--ss-control-h-sm); padding: 0 var(--ss-space-4);
    border: var(--ss-border-width) solid var(--ss-border); border-radius: var(--ss-radius-xs);
    background: var(--ss-surface-input); color: var(--ss-text-primary);
    font-family: var(--ss-font-ui); cursor: pointer;
  }
  .du-btn:disabled { opacity: 0.5; cursor: default; }
  .du-btn--primary { background: var(--ss-accent); border-color: var(--ss-accent); color: var(--ss-on-accent, #fff); }
  .du-action-error { color: var(--ss-danger); font-size: var(--ss-type-caption-size, 12px); margin: 0; }
  .du-spinner {
    width: 28px; height: 28px; border-radius: 50%;
    border: 3px solid var(--ss-border); border-top-color: var(--ss-accent);
    animation: du-spin 0.8s linear infinite;
  }
  @keyframes du-spin { to { transform: rotate(360deg); } }
</style>
```

NOTE: confirm the exact token names against `DESIGN.md` / an existing component (e.g. `--ss-accent`, `--ss-on-accent`, `--ss-surface-input`, `--ss-control-h-sm`, `--ss-radius-*`, `--ss-space-*`, `--ss-type-*`). If a token used above doesn't exist, substitute the nearest existing one used by the current cards/buttons (read MixerPage's `.daemon-down-card`/`.retry-btn` and AppShell's button for the real token names) — do NOT introduce hardcoded hex.

- [ ] **Step 2: Verify** `cd frontend && npm run check` (0 errors), `npm run build` (compiles).

- [ ] **Step 3: Commit**

```bash
git add frontend/src/lib/components/DaemonUnavailable.svelte
git commit -m "feat(ui): shared DaemonUnavailable component (connecting + disconnected, Start/Retry)"
```

---

## Task 3: Integrate into Mixer / Device / Spatial + remove AppShell banner

**Files:**
- Modify: `frontend/src/lib/components/MixerPage.svelte` (replace lines ~136–154 daemon-down/loading; remove `.daemon-down-card`/`.retry-btn`/`.loading-*` CSS)
- Modify: `frontend/src/lib/components/DevicePage.svelte` (replace the daemon-error card ~268–275 + loading ~642–649; KEEP the no-device card ~278–309; remove orphaned `.state-card--error`/loading CSS)
- Modify: `frontend/src/lib/components/SpatialPage.svelte` (replace the loading `{#if !$engineState}` block ~116–123; remove orphaned `.state-card` CSS if now unused)
- Modify: `frontend/src/lib/components/AppShell.svelte` (remove the `daemon-reconnect-banner` markup ~127–138 + its CSS ~345–; keep the topbar status dot/label + the `connectionStatus`/`startConnectionMonitor` import; drop the now-unused `reconnect` import if nothing else uses it)
- Test: none (thin views) — owner-manual-verify

**Interfaces:**
- Consumes: `DaemonUnavailable.svelte` [T2]; `connectionStatus` store.

Pattern for EACH page — import the component + store, then gate the WHOLE page body:
```svelte
import DaemonUnavailable from "./DaemonUnavailable.svelte";
import { connectionStatus } from "../stores/connection.js";
…
{#if $connectionStatus !== "connected"}
  <DaemonUnavailable />
{:else}
  …the page's existing content (UNCHANGED) — for DevicePage this {:else} branch still contains the no-device card…
{/if}
```
Delete the bespoke `{#if $loadError}…{:else if !$engineState}…` daemon-down + loading blocks (the shared component now covers them). Delete the CSS rules that styled ONLY those removed blocks (grep each class name in the file to confirm it's unused before deleting).

- [ ] **Step 1: MixerPage** — wrap content in the gate; delete the `daemon-down-card` + loading blocks and their CSS. `npm run check` + `npm run build`.
- [ ] **Step 2: DevicePage** — wrap content in the gate; delete the daemon-error card + the `Connecting to Daemon…` loading card and their CSS; **keep** the no-device card inside the `{:else}` branch. Verify no-device still renders when connected-but-no-headset.
- [ ] **Step 3: SpatialPage** — wrap content in the gate; delete the `Connecting to Daemon…` block; remove `.state-card` CSS if unused.
- [ ] **Step 4: AppShell** — delete the `daemon-reconnect-banner` block + its CSS; keep the topbar dot. Ensure `reconnect` import is removed if now unused (else `npm run check` warns).
- [ ] **Step 5: Verify** `cd frontend && npm run check` (0 errors), `npm test` (unchanged green), `npm run build` (clean).
- [ ] **Step 6: Commit**

```bash
git add frontend/src/lib/components/MixerPage.svelte frontend/src/lib/components/DevicePage.svelte frontend/src/lib/components/SpatialPage.svelte frontend/src/lib/components/AppShell.svelte
git commit -m "refactor(ui): Mixer/Device/Spatial use shared DaemonUnavailable; drop AppShell banner"
```

---

## Task 4: Add the gate to EqPage + MicPage (the gaps)

**Files:**
- Modify: `frontend/src/lib/components/EqPage.svelte`
- Modify: `frontend/src/lib/components/MicPage.svelte`
- Test: none — owner-manual-verify

**Interfaces:** Consumes `DaemonUnavailable.svelte` [T2] + `connectionStatus`.

- [ ] **Step 1:** In EACH page, import `DaemonUnavailable` + `connectionStatus` and wrap the page body in the same gate (`{#if $connectionStatus !== "connected"} <DaemonUnavailable/> {:else} …existing content… {/if}`). These pages had NO daemon handling, so there's nothing to delete — only the gate to add. Confirm the existing content lands intact in the `{:else}` branch.
- [ ] **Step 2: Verify** `cd frontend && npm run check` (0 errors), `npm run build`.
- [ ] **Step 3: Commit**

```bash
git add frontend/src/lib/components/EqPage.svelte frontend/src/lib/components/MicPage.svelte
git commit -m "feat(ui): EqPage + MicPage show shared DaemonUnavailable when disconnected"
```

---

## Task 5: Cleanup sweep

**Files:**
- Modify: `src-tauri/src/daemon_control.rs` (remove redundant `extern crate libc;`)
- Modify: any page with leftover orphaned daemon-down CSS missed in T3

**Interfaces:** none.

- [ ] **Step 1:** Remove the redundant `extern crate libc;` line from `src-tauri/src/daemon_control.rs` (edition 2018+ doesn't need it; `libc::setsid()` still resolves via the Cargo dep). Run `cargo build -p arctis-sound-manager-ui` (clean) and `cargo test -p arctis-sound-manager-ui` (37 green).
- [ ] **Step 2:** Grep the 5 page components for any now-dead daemon-down CSS class names that survived T3/T4 (e.g. `daemon-down-card`, `daemon-reconnect`, `state-card--error`, `loading-text`, leftover `.retry-btn`) — if a class is defined in `<style>` but no longer referenced in that file's markup, delete the rule. `cd frontend && npm run check` (0 errors), `npm run build`.
- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "chore: remove redundant extern crate libc + orphaned daemon-down CSS"
```

---

## Self-Review Notes (author)

- **Spec coverage:** §4 component → T2; §4 per-page integration + AppShell banner removal → T3; gap pages → T4; §5 behavior (Start/Retry, copy) → T2; §5 helper → T1; "clean up as you go" → each page edit + T5. All covered.
- **DevicePage no-device preserved:** explicitly kept in the T3 `{:else}` branch (it's a different signal; spec §4).
- **No backend behavior change:** T5's `extern crate libc` removal is a no-op cleanup (the dep + `libc::setsid()` call are unchanged).
- **Net shrink (G1):** T3/T4 delete bespoke markup+CSS and add a one-line gate; only T2 adds (the single shared component).
- **No-jsdom:** logic (`viewFor`) in T1 `.ts` + vitest; components are thin, owner-verified.
- **Type consistency:** `viewFor: ConnectionStatus → "connecting"|"disconnected"|"hidden"` stable T1↔T2; `connectionStatus` store imported from `./stores/connection.js` everywhere.
- **Open item (spec §9):** the card glyph (`◉`) is a placeholder — the implementer should match the app's existing iconography; confirm against current cards in T2.
