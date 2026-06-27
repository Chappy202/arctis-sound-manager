import { writable } from "svelte/store";
import { listOutputs, type OutputDeviceSnapshot } from "../ipc.js";

export const outputsStore = writable<OutputDeviceSnapshot[]>([]);

let started = false;

/**
 * Fetch the output device list once and populate the store.
 * Idempotent — safe to call multiple times; only the first call per lifecycle
 * does work. Errors are swallowed so the store stays empty rather than
 * throwing when the daemon is down.
 */
export async function initOutputs(): Promise<void> {
  if (started) return;
  started = true;
  try {
    outputsStore.set(await listOutputs());
  } catch {
    // daemon down — keep empty list; MixerPage re-mounts on reconnect.
  }
}

/**
 * Reset the store and allow a future `initOutputs()` call to re-fetch.
 * Call this in the cleanup return of the `$effect` that called `initOutputs`.
 */
export function destroyOutputs(): void {
  started = false;
  outputsStore.set([]);
}
