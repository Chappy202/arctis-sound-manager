/**
 * volumeSliderLogic.test.ts — Pure logic tests for VolumeSlider timing + reconcile.
 *
 * No DOM, no jsdom, no component rendering. Vitest fake timers drive all timing.
 * Style modelled on stores/connection.test.ts (fake timer setUp/tearDown pattern).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  createVolumeCommitter,
  reconcileValue,
  VOLUME_COMMIT_INTERVAL_MS,
} from "./volumeSliderLogic.js";

// ---------------------------------------------------------------------------
// VolumeCommitter
// ---------------------------------------------------------------------------
describe("VolumeCommitter", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
  });

  // Test 1: Throttle coalesces — 10 rapid schedule() calls fire only one commit
  it("throttle coalesces: 10 rapid schedules → one commit with the latest value", () => {
    const spy = vi.fn();
    const c = createVolumeCommitter(80, spy);
    for (let i = 1; i <= 10; i++) c.schedule(i);
    // No time advanced yet — timer still pending
    expect(spy).not.toHaveBeenCalled();
    vi.advanceTimersByTime(80);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith(10);
  });

  // Test 2: One commit per window — two independent drag windows each fire once
  it("one commit per window: two sequential 80ms windows each fire independently", () => {
    const spy = vi.fn();
    const c = createVolumeCommitter(80, spy);
    c.schedule(5);
    vi.advanceTimersByTime(80); // fires commit(5)
    c.schedule(6);
    vi.advanceTimersByTime(80); // fires commit(6)
    expect(spy).toHaveBeenCalledTimes(2);
    expect(spy.mock.calls[0][0]).toBe(5);
    expect(spy.mock.calls[1][0]).toBe(6);
  });

  // Test 3: flush() commits immediately and cancels the timer (no double-commit)
  it("flush is immediate + no double-commit: timer cancelled after flush", () => {
    const spy = vi.fn();
    const c = createVolumeCommitter(80, spy);
    c.schedule(50);
    c.flush(); // must commit immediately, cancel timer
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith(50);
    vi.advanceTimersByTime(80); // cancelled timer must NOT fire again
    expect(spy).toHaveBeenCalledTimes(1);
  });

  // Test 4: flush() with nothing pending is a no-op; flush after commit also a no-op
  it("flush with nothing pending: spy not called; flush after timer commit: still only one call", () => {
    const spy = vi.fn();
    const c = createVolumeCommitter(80, spy);
    c.flush(); // nothing pending — no-op
    expect(spy).not.toHaveBeenCalled();

    // schedule, let timer commit, then flush → no extra call
    c.schedule(7);
    vi.advanceTimersByTime(80);
    expect(spy).toHaveBeenCalledTimes(1);
    c.flush(); // pending already cleared by timer
    expect(spy).toHaveBeenCalledTimes(1);
  });

  // Test 5: dispose() cancels the timer without committing
  it("dispose cancels: schedule then dispose → timer fires but spy NOT called", () => {
    const spy = vi.fn();
    const c = createVolumeCommitter(80, spy);
    c.schedule(9);
    c.dispose();
    vi.advanceTimersByTime(80);
    expect(spy).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// VOLUME_COMMIT_INTERVAL_MS constant
// ---------------------------------------------------------------------------
describe("VOLUME_COMMIT_INTERVAL_MS", () => {
  it("is 80 (the configured IPC throttle window)", () => {
    expect(VOLUME_COMMIT_INTERVAL_MS).toBe(80);
  });
});

// ---------------------------------------------------------------------------
// reconcileValue
// ---------------------------------------------------------------------------
describe("reconcileValue", () => {
  it("returns current value when dragging=true (guard prevents engine snap)", () => {
    expect(reconcileValue(true, 30, 80)).toBe(80);
  });

  it("returns incoming value when dragging=false (accepts engine echo)", () => {
    expect(reconcileValue(false, 30, 80)).toBe(30);
  });

  it("edge cases: same values are fine", () => {
    expect(reconcileValue(true, 50, 50)).toBe(50);
    expect(reconcileValue(false, 50, 50)).toBe(50);
  });

  it("boundary values: 0 and 100", () => {
    expect(reconcileValue(false, 0, 75)).toBe(0);
    expect(reconcileValue(false, 100, 0)).toBe(100);
    expect(reconcileValue(true, 0, 75)).toBe(75);
    expect(reconcileValue(true, 100, 0)).toBe(0);
  });
});
