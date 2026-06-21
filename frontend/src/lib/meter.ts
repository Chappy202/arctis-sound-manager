/**
 * meter.ts — Pure helpers for the live level-meter display.
 *
 * WHAT THE METERS ACTUALLY SHOW (honesty note)
 * ─────────────────────────────────────────────
 * The `levels` Tauri event carries the *configured software volume* (0.0–1.0
 * linear, averaged across channels) sampled from `pw-dump` Props.channelVolumes
 * for each Arctis virtual sink and the clean-mic source.
 *
 * This is NOT a real-time audio signal peak or RMS — it reflects the volume
 * setting the user (or engine) has applied, not instantaneous signal activity.
 *
 * True real-time peak/RMS metering would require a native pipewire-rs capture
 * stream (a !Send PWConnection on a dedicated thread monitoring the sink-input
 * monitor ports).  That is out of scope for R3 and documented as a follow-up.
 *
 * All functions are pure and free of side effects so they can be unit-tested
 * without a DOM or Tauri runtime.
 */

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/**
 * Event payload shape emitted by the src-tauri `levels` event.
 * Keys are PipeWire node.name strings; values are linear volume scalars [0, 1].
 */
export type LevelsPayload = Record<string, number>;

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/** Clamp a value to the [0, 1] range. */
export function clampLevel(v: number): number {
  return Math.max(0, Math.min(1, v));
}

/**
 * Convert a linear volume scalar [0, 1] to a percentage [0, 100].
 * Out-of-range inputs are clamped before conversion.
 */
export function linearToPercent(linear: number): number {
  return clampLevel(linear) * 100;
}

/**
 * Exponential moving average for smoothing rapid level changes.
 *
 * @param prev   Previous smoothed value (null on first call — returns target directly).
 * @param target Raw new level in [0, 1].
 * @param alpha  Smoothing coefficient in (0, 1].  Higher = faster response.
 *               Default 0.3 gives a snappy-but-smooth feel at ~2 Hz updates.
 * @returns Smoothed level, clamped to [0, 1].
 */
export function smoothLevel(prev: number | null, target: number, alpha = 0.3): number {
  if (prev === null) return clampLevel(target);
  return clampLevel(alpha * target + (1 - alpha) * prev);
}

/**
 * Convert a level scalar [0, 1] to a CSS percentage string suitable for
 * a meter bar's `width` or `height` style property.
 *
 * @example levelToBarStyle(0.75) → "75.0%"
 */
export function levelToBarStyle(level: number): string {
  const pct = linearToPercent(level);
  if (pct === 0) return "0%";
  if (pct === 100) return "100%";
  return `${pct.toFixed(1)}%`;
}

// ---------------------------------------------------------------------------
// pw-dump payload builder
// ---------------------------------------------------------------------------

/**
 * Parse the JSON array from `pw-dump` and extract the average linear volume
 * for each node whose `node.name` is in `targetNodes`.
 *
 * Returns a LevelsPayload (node.name → [0,1] average channelVolume).
 * Nodes not in `targetNodes`, nodes without Props params, and non-Node entries
 * are silently skipped.
 *
 * This is used by the src-tauri metering task to parse pw-dump output,
 * but it is exposed here (pure, testable) for unit testing.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function buildLevelsPayload(pwDumpData: any[], targetNodes: string[]): LevelsPayload {
  const targetSet = new Set(targetNodes);
  const result: LevelsPayload = {};

  for (const entry of pwDumpData) {
    if (entry?.type !== "PipeWire:Interface:Node") continue;

    const nodeName: string | undefined = entry?.info?.props?.["node.name"];
    if (!nodeName || !targetSet.has(nodeName)) continue;

    const propsParams: unknown[] | undefined = entry?.info?.params?.Props;
    if (!Array.isArray(propsParams) || propsParams.length === 0) continue;

    let channelVolumes: number[] | undefined;
    for (const p of propsParams) {
      const cv = (p as Record<string, unknown>)?.channelVolumes;
      if (Array.isArray(cv) && cv.length > 0) {
        channelVolumes = cv as number[];
        break;
      }
    }
    if (!channelVolumes) continue;

    // Average all channels to a single scalar (mono or stereo → one value)
    const avg = channelVolumes.reduce((s, v) => s + v, 0) / channelVolumes.length;
    result[nodeName] = clampLevel(avg);
  }

  return result;
}
