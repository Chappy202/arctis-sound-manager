/**
 * stores/connection.ts — Daemon connection status (derived) + health monitor.
 *
 * `connectionStatus` is DERIVED from the real data (`loadError` + `engineState`)
 * via `deriveConnectionStatus` — the SAME computation the topbar dot uses. This
 * gives a single source of truth: the status can never disagree with the data,
 * and there is no manually-driven writable to get stuck (the previous design's
 * bug). The monitor and `init()` only ever write `loadError`/`engineState`; the
 * status follows automatically.
 *
 * Design notes:
 * - Module-level `started` guard prevents duplicate monitors.
 * - `inFlight` guard prevents overlapping polls when getState() is slow.
 * - init() (stores.ts) does a getState() fetch immediately on startup, so the
 *   monitor only needs to handle ongoing liveness on its interval.
 * - reconnect() is the user-triggered immediate retry; the monitor's periodic
 *   poll is the background auto-retry while disconnected.
 */
import { derived, get, type Readable } from "svelte/store";
import { getState } from "../ipc.js";
import { loadError, engineState, init, destroy } from "../stores.js";
import { deriveConnectionStatus, type ConnectionStatus } from "../connection.js";

export type { ConnectionStatus };

// ---------------------------------------------------------------------------
// Derived status — single source of truth
// ---------------------------------------------------------------------------

/**
 * Live daemon connection status, derived from `loadError` + `engineState`.
 *   loadError set → "disconnected"; engineState null → "connecting"; else "connected".
 */
export const connectionStatus: Readable<ConnectionStatus> = derived(
  [loadError, engineState],
  ([$loadError, $engineState]) => deriveConnectionStatus($loadError, $engineState),
);

// ---------------------------------------------------------------------------
// Health monitor
// ---------------------------------------------------------------------------

let started = false;

/**
 * Start a periodic health monitor that calls getState() every `intervalMs`.
 *
 * This is a SLOW FALLBACK: primary outage detection is the Rust state poll's
 * edge-triggered `daemon-down` event (subscribed in stores.ts init()), which
 * flips the status within a poll tick. This poll only catches whatever slips
 * past it (e.g. the event subscription itself failed) and drives recovery from
 * a cold/offline start — hence the relaxed 15 s default cadence.
 *
 * On failure it sets `loadError` (→ status "disconnected"). On success it clears
 * `loadError` and, if `engineState` is still null (recovery from a cold/offline
 * start), populates it so the status derives to "connected" immediately. It does
 * NOT overwrite a populated `engineState` (the 250 ms `state-changed` event is the
 * source of truth for the data; re-setting the ref every poll would churn every
 * app-wide $derived).
 *
 * Returns a stop function. A second call while one is running is a no-op.
 */
export function startConnectionMonitor(intervalMs = 15000): () => void {
  if (started) return () => {};
  started = true;

  let inFlight = false;
  const poll = async (): Promise<void> => {
    if (inFlight) return; // skip if previous poll hasn't completed
    inFlight = true;
    try {
      const state = await getState();
      // Populate engineState BEFORE clearing loadError so the status never
      // flashes through "connecting" (engineState set → still disconnected
      // while loadError holds; then loadError cleared → connected).
      if (get(engineState) === null) engineState.set(state);
      // Only clear on recovery — avoids a redundant notify on every healthy poll.
      if (get(loadError) !== null) loadError.set(null);
    } catch (e) {
      const msg =
        e instanceof Error
          ? e.message
          : typeof e === "string"
            ? e
            : "Daemon unavailable";
      loadError.set(msg);
    } finally {
      inFlight = false;
    }
  };

  const id = setInterval(() => void poll(), intervalMs);

  return () => {
    clearInterval(id);
    started = false;
  };
}

// ---------------------------------------------------------------------------
// Explicit reconnect
// ---------------------------------------------------------------------------

/**
 * Force an immediate reconnect. Clears `loadError` + `engineState` (so the status
 * derives to "connecting"), tears down the event subscription, and re-runs init()
 * — which sets `loadError`/`engineState` again once the fetch resolves.
 */
export async function reconnect(): Promise<void> {
  loadError.set(null);
  engineState.set(null);
  destroy();
  await init();
}
