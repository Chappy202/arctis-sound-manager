/**
 * volumeSliderLogic.ts — Pure timing + reconcile logic for VolumeSlider.
 *
 * Extracted from the Svelte component so it can be unit-tested with Vitest
 * fake timers without any DOM or component rendering harness.
 *
 * Design: trailing throttle (at most one commit per intervalMs during a drag),
 * immediate flush on pointer-up, and a reconcile guard that holds the local
 * value while the user is dragging so engine echoes don't snap the thumb.
 */

/** IPC throttle window in ms — one commit per interval during drag. */
export const VOLUME_COMMIT_INTERVAL_MS = 80;

/**
 * Trailing throttle around a volume commit callback.
 *
 * - `schedule(v)`: record v as the latest pending value; arm a single timer
 *   for intervalMs if none is armed. Further schedule() calls during an armed
 *   window only update the pending value (no extra timers) → at most one
 *   commit per interval during a drag.
 * - `flush()`: if a value is pending, commit it immediately and cancel the
 *   armed timer. No-op if nothing is pending (never double-commits).
 * - `dispose()`: cancel any armed timer WITHOUT committing (teardown).
 */
export interface VolumeCommitter {
  schedule(value: number): void;
  flush(): void;
  dispose(): void;
}

export function createVolumeCommitter(
  intervalMs: number,
  commit: (value: number) => void,
): VolumeCommitter {
  let pending = false;
  let latest = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;

  function fire() {
    if (pending) {
      commit(latest);
      pending = false;
    }
    timer = null;
  }

  return {
    schedule(value: number) {
      latest = value;
      pending = true;
      if (timer === null) {
        timer = setTimeout(fire, intervalMs);
      }
      // If timer is already armed, just updating latest is sufficient —
      // fire() will pick up the latest value when it runs.
    },

    flush() {
      if (!pending) return;
      // Cancel the pending timer so fire() doesn't double-commit.
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      commit(latest);
      pending = false;
    },

    dispose() {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
      // Clear pending so a stale flush() after dispose() is a no-op.
      pending = false;
    },
  };
}

/**
 * Reconcile guard: while the user is dragging, hold the local slider value
 * so engine/hardware echoes don't snap the thumb mid-drag. When not dragging,
 * accept the incoming engine value (e.g. after a hardware knob adjustment).
 *
 * Pure mirror of the component's $effect untrack guard — extracted here so it
 * can be tested without a Svelte runtime.
 */
export function reconcileValue(
  dragging: boolean,
  incoming: number,
  current: number,
): number {
  return dragging ? current : incoming;
}
