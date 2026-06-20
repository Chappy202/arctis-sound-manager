/**
 * stores.ts — Application-level Svelte stores for Arctis Sound Manager.
 *
 * `engineState` is the single source of truth for EngineState, populated by
 * a one-shot getState() call on init and kept fresh via the state-changed event.
 */
import { writable } from "svelte/store";
import { getState, onStateChanged, type EngineState } from "./ipc.js";
import type { UnlistenFn } from "@tauri-apps/api/event";

/** The latest EngineState from the daemon. null = not yet loaded. */
export const engineState = writable<EngineState | null>(null);

/** Set when the daemon is unreachable or returns an error on startup. */
export const loadError = writable<string | null>(null);

let unlistenFn: UnlistenFn | null = null;

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

  // Subscribe first so we never miss an event that races the initial fetch.
  try {
    unlistenFn = await onStateChanged((state) => {
      engineState.set(state);
      loadError.set(null);
    });
  } catch (e) {
    // Non-fatal; we'll still try the initial fetch.
    console.warn("[stores] Failed to subscribe to state-changed events:", e);
  }

  // Initial fetch.
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
}

/** Tear down the event subscription (e.g. on component destroy). */
export function destroy(): void {
  if (unlistenFn) {
    unlistenFn();
    unlistenFn = null;
  }
  initialised = false;
}
