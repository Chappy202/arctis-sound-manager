import type { EqPresetSnapshot } from "../ipc.js";

export interface GroupedPresets {
  builtin: EqPresetSnapshot[];
  saved: EqPresetSnapshot[];
}

/**
 * Splits factory and saved preset arrays into named display groups.
 * Pure function — safe to unit-test without Svelte or Tauri.
 */
export function groupPresets(
  factory: EqPresetSnapshot[],
  saved: EqPresetSnapshot[],
): GroupedPresets {
  return { builtin: [...factory], saved: [...saved] };
}
