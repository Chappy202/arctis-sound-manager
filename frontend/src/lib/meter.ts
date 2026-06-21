/**
 * meter.ts — Pure helpers for the live signal-peak level-meter display.
 *
 * WHAT THE METERS ACTUALLY SHOW
 * ──────────────────────────────
 * The `levels` Tauri event carries the *real-time signal peak* (0.0–1.0
 * normalised) captured by `pw-record` PCM capture workers:
 *
 * - Channel sinks (Arctis_Game / Arctis_Chat / Arctis_Media): captured via
 *   their PipeWire monitor port (`<name>.monitor`), s16le stereo @ 48 kHz.
 * - Mic source (arctis_clean_mic): captured directly, s16le mono @ 48 kHz.
 *
 * Peak is computed over ~40 ms windows (1 920 samples) and emitted at ~25 Hz.
 * A channel that is silent shows 0.0; a full-scale signal shows 1.0 —
 * regardless of the configured volume setting.
 *
 * When the Arctis virtual nodes are not present (daemon not running), the
 * worker processes find no target and hold their level at 0.0; the UI shows
 * an inactive (dim) meter — honest: no signal → no bar.
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

/**
 * Apply a fast-attack / slow-decay envelope to a peak level for display.
 *
 * The meter rises instantly to any new peak (attack = 1.0) and decays
 * exponentially toward 0 when the incoming signal drops.
 *
 * @param prev     Previous displayed level (null = first call).
 * @param incoming Raw peak from the capture worker [0, 1].
 * @param decay    Decay coefficient per tick in (0, 1]. Lower = slower decay.
 *                 Default 0.15 gives ~250 ms fall-time at 25 Hz.
 * @returns New display level, clamped to [0, 1].
 */
export function peakDecay(
  prev: number | null,
  incoming: number,
  decay = 0.15,
): number {
  if (prev === null) return clampLevel(incoming);
  const p = clampLevel(incoming);
  const held = prev * (1 - decay);
  // Fast attack: jump to the incoming peak immediately if higher
  return clampLevel(Math.max(p, held));
}
