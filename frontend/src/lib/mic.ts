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
 * All other kinds (including "suppression") map identity.
 */
export function stageWireName(kind: string): string {
  if (kind === "mic_eq") return "eq";
  return kind;
}

// ---------------------------------------------------------------------------
// Plugin path lookup (E6/E7)
// ---------------------------------------------------------------------------

/**
 * Returns the LADSPA plugin basename for a stage, or null for builtin stages
 * that need no external plugin.
 *
 * Used for the "Plugin not installed: <basename>" unavailability tooltip.
 * Basenames are distro-agnostic — the daemon resolves the actual .so path via
 * $LADSPA_PATH and well-known search dirs at runtime.
 * Note: for "suppression", plugin availability is backend-specific — use
 * backendAvailable/backendTooltip helpers instead; this returns null.
 */
export function stagePluginPath(kind: string): string | null {
  switch (kind) {
    case "compressor":
      return "sc4m_1916";
    default:
      return null; // builtins: gain, highpass, gate, mic_eq; suppression uses backendTooltip
  }
}

// ---------------------------------------------------------------------------
// Suppression backend helpers (E4/§8)
// ---------------------------------------------------------------------------

/**
 * Returns a human-readable label for a suppression backend id.
 *   "deep_filter" → "DeepFilterNet"
 *   "rnnoise"     → "RNNoise"
 *   other         → the raw id (forward-compat)
 */
export function backendLabel(backend: string): string {
  switch (backend) {
    case "deep_filter":
      return "DeepFilterNet";
    case "rnnoise":
      return "RNNoise";
    default:
      return backend;
  }
}

/**
 * Returns true when the given backend id is present in the available list
 * (i.e., its plugin has been detected on this machine).
 */
export function backendAvailable(backend: string, available: string[]): boolean {
  return available.includes(backend);
}

/**
 * Returns an informational tooltip for a backend that is NOT available, or
 * undefined when the backend IS available (no tooltip needed).
 *
 * Only covers the known backends; unknown ids get a generic message.
 */
export function backendTooltip(backend: string, available: string[]): string | undefined {
  if (backendAvailable(backend, available)) return undefined;
  switch (backend) {
    case "deep_filter":
      return "DeepFilterNet plugin not installed — see README";
    case "rnnoise":
      return "RNNoise plugin not installed — see README";
    default:
      return `${backend} plugin not installed`;
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
 * For the suppression stage: handled separately via backendTooltip.
 * For builtin stages that are still somehow unavailable: generic message.
 * For the gate stage: version/plugin message.
 */
export function stageUnavailableTooltip(stage: MicStageSnapshot): string | undefined {
  if (stage.available) return undefined;
  if (stage.kind === "gate") {
    return "Requires PipeWire ≥1.6 (builtin gate) or the swh gate_1410 LADSPA plugin";
  }
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
 * Maps a Band (camelCase EqGraph format) and its index to the snake_case args
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
