/**
 * stores/connection.ts — Daemon connection status store + health monitor.
 *
 * Tracks daemon liveness independently of the one-shot init() in stores.ts.
 * A periodic getState() poll detects when the daemon dies after the initial
 * connect and flips connectionStatus to "disconnected" without requiring a
 * UI reload.
 *
 * Design notes:
 * - Module-level `started` guard prevents duplicate monitors (mirrors the
 *   pattern in stores/streams.ts).
 * - `inFlight` guard prevents overlapping polls when getState() is slow.
 * - reconnect() is the user-triggered immediate retry; the monitor's own
 *   periodic poll is the background auto-retry while disconnected.
 */
import { writable } from "svelte/store";
import { getState } from "../ipc.js";
import { loadError, init, destroy } from "../stores.js";

// ---------------------------------------------------------------------------
// Public types + store
// ---------------------------------------------------------------------------

export type ConnectionStatus = "connecting" | "connected" | "disconnected";

/** Live daemon connection status, updated by the monitor and by stores.ts hooks. */
export const connectionStatus = writable<ConnectionStatus>("connecting");

// ---------------------------------------------------------------------------
// Thin setters (called by stores.ts on every successful state update)
// ---------------------------------------------------------------------------

/** Signal that the daemon is reachable. */
export function markConnected(): void {
  connectionStatus.set("connected");
}

/** Signal that the daemon is unreachable. */
export function markDisconnected(): void {
  connectionStatus.set("disconnected");
}

// ---------------------------------------------------------------------------
// Health monitor
// ---------------------------------------------------------------------------

let started = false;

/**
 * Start a periodic health monitor that calls getState() every `intervalMs`.
 *
 * This is a pure liveness ping. It does NOT write engineState — the 250 ms
 * `state-changed` event (subscribed in stores.ts) is the single source of truth
 * for the store, and it already applies a Rust-side emit-on-change guard.
 * Overwriting engineState here every poll would replace the object reference and
 * needlessly re-run every app-wide $derived. We only flip connection status and
 * clear/set loadError (the latter guarded so it never writes redundantly).
 *
 * On success: marks connected, clears any stale loadError.
 * On failure: marks disconnected, sets loadError.
 *
 * Returns a stop function that cancels the interval and allows a new monitor
 * to be started.  Calling startConnectionMonitor() a second time while one is
 * already running is a no-op — the returned stop function does nothing.
 */
export function startConnectionMonitor(intervalMs = 5000): () => void {
  if (started) return () => {};
  started = true;

  let inFlight = false;
  // Tracks the last error we wrote, so we only clear loadError on recovery
  // (avoids a redundant `loadError.set(null)` notify on every healthy poll).
  let lastErrorSet: string | null = null;

  const id = setInterval(async () => {
    if (inFlight) return; // skip if previous poll hasn't completed
    inFlight = true;
    try {
      await getState(); // liveness ping only — result is intentionally unused
      markConnected();
      if (lastErrorSet !== null) {
        loadError.set(null);
        lastErrorSet = null;
      }
    } catch (e) {
      const msg =
        e instanceof Error
          ? e.message
          : typeof e === "string"
            ? e
            : "Daemon unavailable";
      markDisconnected();
      loadError.set(msg);
      lastErrorSet = msg;
    } finally {
      inFlight = false;
    }
  }, intervalMs);

  return () => {
    clearInterval(id);
    started = false;
  };
}

// ---------------------------------------------------------------------------
// Explicit reconnect
// ---------------------------------------------------------------------------

/**
 * Force an immediate reconnect.  Sets connectionStatus to "connecting",
 * tears down the event subscription, and re-initialises the state store.
 * stores.ts's init() will call markConnected() or markDisconnected() once the
 * initial fetch resolves.
 */
export async function reconnect(): Promise<void> {
  connectionStatus.set("connecting");
  destroy();
  await init();
}
