import type { MicPresetSnapshot } from "../ipc.js";

/**
 * Returns the description for a named mic preset, or null when the name is
 * empty or no preset with that name exists in the supplied list.
 * Pure function — safe to unit-test without Svelte or Tauri.
 */
export function findMicPresetDescription(
  name: string,
  presets: MicPresetSnapshot[],
): string | null {
  if (!name) return null;
  return presets.find((p) => p.name === name)?.description ?? null;
}
