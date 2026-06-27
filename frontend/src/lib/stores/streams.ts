import { writable } from "svelte/store";
import { listStreams, onStreamsChanged, type AppStream } from "../ipc.js";
import type { UnlistenFn } from "@tauri-apps/api/event";

export const streamsStore = writable<AppStream[]>([]);

let unlisten: UnlistenFn | null = null;
let started = false;

export async function initStreams(): Promise<void> {
  if (started) return;
  started = true;
  try {
    unlisten = await onStreamsChanged((s) => streamsStore.set(s));
  } catch (e) {
    console.warn("[streams] subscribe failed:", e);
  }
  try {
    streamsStore.set(await listStreams());
  } catch {
    // daemon down — keep empty; poll will refill.
  }
}

export function destroyStreams(): void {
  if (unlisten) {
    unlisten();
    unlisten = null;
  }
  started = false;
  streamsStore.set([]);
}
