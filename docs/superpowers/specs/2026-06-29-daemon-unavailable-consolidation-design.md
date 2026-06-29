# Daemon-Unavailable View Consolidation — Design Spec

**Date:** 2026-06-29
**Status:** Approved (owner) — shared per-page component, with a Start-daemon action.
**Refs:** `DESIGN.md` (`--ss` tokens), `frontend/src/lib/stores/connection.ts`, the daemon-control feature on this branch (`daemonStart`).

## 1. Goal

Replace the **three divergent** daemon-unavailable treatments (MixerPage centered card, DevicePage inline error card, AppShell runtime banner) plus the **two gaps** (EqPage, MicPage show nothing) with **one shared component** rendered consistently by every page. The unified view also lets the user **start the daemon** in place (reusing the daemon-control `daemonStart`).

## 2. Owner decisions

- **D1 — Shared component, per-page** (not a single global gate). Each page renders the same `<DaemonUnavailable>` component in its own content area; the *look and copy* are identical everywhere, the *placement* stays per-page.
- **D2 — Include a "Start daemon" action** in the unified view, reusing `daemonStart()` from the daemon-control feature; plus a "Retry" (reconnect) action.

## 3. Current state (why)

Connection state is already centralized: `connectionStatus` store (`"connecting" | "connected" | "disconnected"`) and `loadError` (`stores.ts`), with `reconnect()` (user retry) and `startConnectionMonitor()` (5 s background poll) in `stores/connection.ts`. But each page hand-rolls its own daemon-down/loading markup with different copy ("Daemon not running" vs "Daemon Unavailable" vs "Daemon disconnected"), icons (◉ / ⚠ / dot), layouts (420 px card vs inline card vs banner), and EqPage/MicPage have none. There is no shared error/empty-state primitive.

## 4. Architecture

**One new component `frontend/src/lib/components/DaemonUnavailable.svelte`** that owns the entire not-connected experience and is driven by the existing `connectionStatus`/`loadError` stores:

- `connectionStatus === "connecting"` → a **spinner + "Connecting to daemon…"** (unifies the 3 bespoke loading spinners).
- `connectionStatus === "disconnected"` → the **unified card**: icon, canonical title/body, the raw `loadError` shown subtly in mono, and two actions — **Start daemon** and **Retry**.
- `connectionStatus === "connected"` → renders nothing (`{#if}` guard; pages render their real content).

**Per-page integration:** each page (`MixerPage`, `EqPage`, `DevicePage`, `SpatialPage`, `MicPage`) gates its content:
```svelte
{#if $connectionStatus !== "connected"}
  <DaemonUnavailable />
{:else}
  …existing page content…
{/if}
```
…and DELETES its bespoke daemon-down card + loading-spinner markup/CSS. Net: one component, five identical call sites.

**AppShell:** the runtime `daemon-reconnect-banner` (lines ~127–138) is **removed** — it's superseded by the per-page component (and was one of the inconsistent treatments). The always-on **topbar status dot + label stays** (small persistent indicator, not a "view").

**Distinct, NOT in scope:** DevicePage's **"No Arctis Nova Pro Detected"** state is a *different* signal (daemon reachable, headset absent) — it stays as-is. This spec only unifies the *daemon-unreachable / connecting* states.

## 5. Component behavior

`DaemonUnavailable.svelte` (thin view; subscribes to `connectionStatus`, `loadError`):

- **Start daemon** button: `busy = true; try { await daemonStart(); await reconnect(); } catch (e) { actionError = String(e); } finally { busy = false; }`. On the binary-not-found error from `daemonStart`, the message surfaces in `actionError` (e.g. "asm-cli binary not found; set $ASM_CLI_BIN") — graceful, no crash.
- **Retry** button: `reconnect()` (the existing user-triggered immediate retry).
- Both disabled while `busy`. An inline `actionError` area shows a failed Start/Retry message.
- Canonical copy (single source):
  - disconnected title: **"Daemon not running"**
  - body: **"The Arctis Sound Manager service isn't reachable, so changes won't apply."**
  - buttons: **"Start daemon"**, **"Retry"**
  - connecting: **"Connecting to daemon…"**
- Styling: `--ss` tokens only (danger/warning/surface/border/text, `--ss-font-*`), matching the existing card/button styles already in the app (reuse the `.ss-btn`-style buttons; the Start button is the primary/accent action).

**Small testable helper** `frontend/src/lib/daemonUnavailable.ts`: `viewFor(status: ConnectionStatus): "connecting" | "disconnected" | "hidden"` (maps the store value to which sub-view renders). Unit-tested (no jsdom). The component delegates its top-level branch to this.

## 6. Non-negotiable constraints

- No backend/engine changes — pure frontend consolidation reusing existing stores + the existing `daemonStart`/`reconnect` IPC. `tauri`/Rust untouched.
- Frontend convention: NO jsdom/testing-library; logic (the `viewFor` mapping) in a `.ts` helper with vitest; `.svelte` is a thin view (owner-manual-verifies visuals).
- Design system: `--ss` tokens, no ad-hoc hex unless a token genuinely doesn't exist.
- Reuse over duplication (G1): exactly one component + one helper; the five pages and AppShell shrink (delete bespoke markup), they don't grow.
- Behavior parity: the unified view must cover BOTH init failure (`loadError` at startup) AND runtime disconnect (health-monitor) via the single `connectionStatus` signal — same look, same timing semantics, on every page.

## 7. Testing

- **Vitest (no jsdom):** `daemonUnavailable.ts` `viewFor` — "connecting"→"connecting", "disconnected"→"disconnected", "connected"→"hidden".
- **Type/compile:** `npm run check` 0 errors; `npm run build` clean (Svelte compile catches the per-page edits).
- **Owner-manual-verify (can't unit-test):** with the daemon stopped, EVERY page (Mixer/EQ/Device/Spatial/Mic) shows the identical "Daemon not running" view; "Start daemon" launches it and the page populates; "Retry" reconnects; the old per-page cards + AppShell banner are gone; the Device "no headset" state is unaffected.

## 8. Out of scope (YAGNI)

- Unifying the "no device detected" empty-state (different signal).
- Reducing the 5 s runtime-disconnect detection latency (separate concern; the view itself is now consistent regardless of when it appears).
- A general error/banner component library beyond this one component.

## 9. Open items

- Exact icon/glyph for the unified card (pick one consistent with the app's existing iconography during implementation — likely a single neutral/warning glyph, not three different ones).
