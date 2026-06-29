/**
 * surround.ts — Pure helpers for the Spatial (virtual surround) page.
 *
 * No DOM, no Svelte, no IPC. All functions are unit-tested in surround.test.ts.
 * SpatialPage.svelte is a thin view that delegates logic here.
 */

import type { OutputDeviceSnapshot } from "./ipc.js";
import type { SelectOption } from "./ui/selectUtils.js";

// ---------------------------------------------------------------------------
// Hardware-sink Select helpers
// ---------------------------------------------------------------------------

/**
 * Build the dropdown option list for the hardware-sink Select.
 *
 * The first entry is always "Auto-detect" (value = ""), which maps to
 * `hw_sink = null` in the engine. When `resolved` is provided (the sink name
 * the engine auto-detected), it appears in parentheses so the user can see
 * what the engine chose.
 */
export function buildSinkOptions(
  outputs: OutputDeviceSnapshot[],
  resolved: string | null,
): SelectOption[] {
  const autoLabel = resolved ? `Auto-detect (${resolved})` : "Auto-detect";
  return [
    { value: "", label: autoLabel },
    ...outputs.map((o) => ({ value: o.node_name, label: o.description || o.node_name })),
  ];
}

/**
 * Convert `hw_sink` (possibly null/undefined from engine state) to a Select value.
 * null/undefined → "" which corresponds to the Auto-detect entry.
 */
export function sinkSelectValue(hwSink: string | null | undefined): string {
  return hwSink ?? "";
}

/**
 * Convert a Select value back to the `hw_sink` field for `surroundSetHwSink`.
 * "" → null (auto-detect); any non-empty string → pin to that specific sink.
 */
export function sinkValueToHwSink(v: string): string | null {
  return v === "" ? null : v;
}

// ---------------------------------------------------------------------------
// HRIR display name (strip leading NN- numeric prefix; replace dashes with spaces)
// ---------------------------------------------------------------------------

/**
 * Convert an HRIR stem (filename without .wav) to a human-readable display name.
 *
 * Rules (simple + deterministic):
 *   1. Strip a leading `NN-` numeric prefix (one or more digits + dash).
 *   2. Replace remaining dashes with spaces.
 *   3. Title-case each word (first char uppercase, rest lowercase).
 *
 * Examples:
 *   "02-dh-dolby-headphone"  → "Dh Dolby Headphone"
 *   "00-default-asm"         → "Default Asm"
 *   "flat"                   → "Flat"
 *   "custom-hrir-v2"         → "Custom Hrir V2"
 */
export function hrirDisplayName(stem: string): string {
  // Strip leading numeric prefix (e.g. "02-")
  const stripped = stem.replace(/^\d+-/, "");
  // Replace dashes with spaces, then title-case each word
  return stripped
    .split("-")
    .map((word) => (word.length > 0 ? word[0].toUpperCase() + word.slice(1).toLowerCase() : ""))
    .join(" ");
}

// ---------------------------------------------------------------------------
// Channel helpers
// ---------------------------------------------------------------------------

/**
 * Returns true when the given channel id is in the surround channels list.
 * Used to determine checkbox state for each channel.
 */
export function channelChecked(id: string, channels: string[]): boolean {
  return channels.includes(id);
}

/**
 * Returns a new channels list with the given id toggled in or out.
 * Deterministic: order is stable (existing order preserved for kept items;
 * new additions are appended).
 */
export function toggleChannel(id: string, channels: string[]): string[] {
  if (channels.includes(id)) {
    return channels.filter((c) => c !== id);
  }
  return [...channels, id];
}
