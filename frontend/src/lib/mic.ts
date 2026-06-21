/**
 * mic.ts — Pure helpers for the Mic page.
 *
 * No DOM, no Svelte, no IPC. All functions are unit-tested in mic.test.ts.
 * MicPage.svelte is a thin view that delegates logic here.
 */

import type { MicStageSnapshot } from "./ipc.js";
import type { Band } from "./eq.js";

// ---------------------------------------------------------------------------
// Stage wire-name mapping (E3)
// ---------------------------------------------------------------------------

/**
 * Convert a snapshot stage `kind` to the wire stage string expected by
 * `Request::MicStage { stage, enabled }`.
 *
 * The only mismatch: snapshot kind "mic_eq" → wire string "eq".
 * All other kinds map identity.
 */
export function stageWireName(kind: string): string {
  if (kind === "mic_eq") return "eq";
  return kind;
}

// ---------------------------------------------------------------------------
// Plugin path lookup (E6/E7)
// ---------------------------------------------------------------------------

/**
 * Returns the filesystem path of the LADSPA plugin for a stage, or null for
 * builtin stages that need no external plugin.
 *
 * Used for the "Plugin not installed: <path>" unavailability tooltip.
 */
export function stagePluginPath(kind: string): string | null {
  switch (kind) {
    case "rnnoise":
      return "/usr/lib64/ladspa/librnnoise_ladspa.so";
    case "compressor":
      return "/usr/lib64/ladspa/sc4m_1916.so";
    default:
      return null; // builtins: gain, highpass, gate, mic_eq
  }
}

// ---------------------------------------------------------------------------
// Unavailability helpers (E7)
// ---------------------------------------------------------------------------

/**
 * Returns true when a stage's plugin/builtin is not present on the system
 * (i.e., its `available` flag is false). Disabled + dimmed rendering applies.
 */
export function isStageDisabled(stage: MicStageSnapshot): boolean {
  return !stage.available;
}

/**
 * Returns the native `title` tooltip text for an unavailable stage, or
 * `undefined` when the stage is available (no tooltip needed).
 *
 * For stages with a known plugin path: "Plugin not installed: <path>".
 * For builtin stages that are still somehow unavailable: "Stage not available".
 */
export function stageUnavailableTooltip(stage: MicStageSnapshot): string | undefined {
  if (stage.available) return undefined;
  const path = stagePluginPath(stage.kind);
  if (path) {
    return `Plugin not installed: ${path}`;
  }
  // Builtin without a path (edge case: shouldn't happen in normal operation)
  return "Stage not available";
}

// ---------------------------------------------------------------------------
// Band argument mapping (E6)
// ---------------------------------------------------------------------------

/**
 * Maps a Band (camelCase EqCanvas format) and its index to the snake_case args
 * expected by the `micEqBand` IPC wrapper (and ultimately `Request::MicEqBand`).
 */
export function micBandToArgs(
  index: number,
  band: Band,
): { band: number; kind: string; freq_hz: number; q: number; gain_db: number } {
  return {
    band: index,
    kind: band.kind,
    freq_hz: band.freqHz,
    q: band.q,
    gain_db: band.gainDb,
  };
}
