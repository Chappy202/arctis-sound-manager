/**
 * stores.ts — Application-level Svelte stores for Arctis Sound Manager.
 *
 * `engineState` is the single source of truth for EngineState, populated by
 * a one-shot getState() call on init and kept fresh via the state-changed event.
 */
import { writable } from "svelte/store";
import { getState, onStateChanged, onDaemonDown, type EngineState } from "./ipc.js";
import type { UnlistenFn } from "@tauri-apps/api/event";
// NOTE: connectionStatus is DERIVED from engineState + loadError (see
// stores/connection.ts), so init() only writes those two stores — there is no
// manual markConnected/markDisconnected to keep in sync.

/** The latest EngineState from the daemon. null = not yet loaded. */
export const engineState = writable<EngineState | null>(null);

/** Set when the daemon is unreachable or returns an error on startup. */
export const loadError = writable<string | null>(null);

let unlistenFn: UnlistenFn | null = null;
let unlistenDownFn: UnlistenFn | null = null;

/**
 * Initialise the state store.
 * - Fetches the current state from the daemon immediately.
 * - Subscribes to state-changed events for live updates.
 * - Saves the unlisten handle so callers can tear down cleanly.
 *
 * Safe to call more than once: subsequent calls are no-ops.
 */
let initialised = false;

export async function init(): Promise<void> {
  if (initialised) return;
  initialised = true;

  // Determine connectivity FIRST. Doing the fetch before the event subscription
  // means a slow or hanging `onStateChanged()` can never block the connection
  // status from resolving to "connected"/"disconnected" (the bug that left the
  // UI stuck on a "Connecting…" spinner with no controls).
  try {
    const state = await getState();
    engineState.set(state);
    loadError.set(null);
  } catch (e) {
    const msg =
      e instanceof Error
        ? e.message
        : typeof e === "string"
          ? e
          : "Daemon unavailable";
    loadError.set(msg);
  }

  // Then subscribe for live updates. (At 250 ms cadence, the tiny window between
  // the fetch and the subscription is immediately re-covered by the next event.)
  try {
    unlistenFn = await onStateChanged((state) => {
      engineState.set(state);
      loadError.set(null);
    });
  } catch (e) {
    console.warn("[stores] Failed to subscribe to state-changed events:", e);
  }

  // Edge-triggered outage push from the Rust state poll — flips the derived
  // connection status to "disconnected" within a poll tick instead of waiting
  // for the slow JS fallback poll (stores/connection.ts). Recovery flows back
  // through state-changed above (the poll force re-emits after an outage).
  try {
    unlistenDownFn = await onDaemonDown((msg) => {
      loadError.set(msg || "Daemon unavailable");
    });
  } catch (e) {
    console.warn("[stores] Failed to subscribe to daemon-down events:", e);
  }
}

/** Tear down the event subscriptions (e.g. on component destroy). */
export function destroy(): void {
  if (unlistenFn) {
    unlistenFn();
    unlistenFn = null;
  }
  if (unlistenDownFn) {
    unlistenDownFn();
    unlistenDownFn = null;
  }
  initialised = false;
}
