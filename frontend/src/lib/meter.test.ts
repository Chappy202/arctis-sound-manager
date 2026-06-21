/**
 * meter.test.ts — Unit tests for the pure level-meter helper module.
 *
 * These run in vitest (Node, no DOM / Tauri).  All functions under test are
 * pure transformations so they can be exercised without any runtime.
 *
 * What the meters actually show:
 *   The `levels` event carries the *real-time signal peak* (0.0–1.0
 *   normalised) captured by `pw-record` PCM capture workers — s16le at
 *   48 kHz.  This is a genuine signal level, not a configured volume scalar.
 *   A silent channel shows 0.0 regardless of its volume setting.
 */
import { describe, expect, it } from "vitest";
import {
  clampLevel,
  linearToPercent,
  smoothLevel,
  levelToBarStyle,
  peakDecay,
  type LevelsPayload,
} from "./meter.js";

// ---------------------------------------------------------------------------
// clampLevel
// ---------------------------------------------------------------------------

describe("clampLevel", () => {
  it("clamps values below 0 to 0", () => {
    expect(clampLevel(-0.5)).toBe(0);
  });

  it("clamps values above 1 to 1", () => {
    expect(clampLevel(1.5)).toBe(1);
  });

  it("passes through values in [0, 1]", () => {
    expect(clampLevel(0)).toBe(0);
    expect(clampLevel(0.5)).toBe(0.5);
    expect(clampLevel(1)).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// linearToPercent
// ---------------------------------------------------------------------------

describe("linearToPercent", () => {
  it("maps 0 to 0%", () => {
    expect(linearToPercent(0)).toBe(0);
  });

  it("maps 1 to 100%", () => {
    expect(linearToPercent(1)).toBe(100);
  });

  it("maps 0.5 to 50%", () => {
    expect(linearToPercent(0.5)).toBeCloseTo(50);
  });

  it("clamps out-of-range inputs", () => {
    expect(linearToPercent(-1)).toBe(0);
    expect(linearToPercent(2)).toBe(100);
  });
});

// ---------------------------------------------------------------------------
// smoothLevel — exponential smoothing (kept for backwards compatibility)
// ---------------------------------------------------------------------------

describe("smoothLevel", () => {
  it("returns target when there is no previous value", () => {
    expect(smoothLevel(null, 0.8)).toBeCloseTo(0.8);
  });

  it("smooths toward target (slow α = 0.1)", () => {
    // With α=0.1: new = 0.1 * 0.8 + 0.9 * 0.0 = 0.08
    const result = smoothLevel(0.0, 0.8, 0.1);
    expect(result).toBeCloseTo(0.08);
  });

  it("converges after many ticks", () => {
    let v = 0;
    for (let i = 0; i < 50; i++) {
      v = smoothLevel(v, 1.0, 0.3)!;
    }
    // After 50 ticks with α=0.3 it should be very close to 1.0
    expect(v).toBeGreaterThan(0.99);
  });

  it("clamps the returned value to [0, 1]", () => {
    // Even with numeric noise the output must stay in range
    expect(smoothLevel(0.5, 1.5, 0.8)).toBeLessThanOrEqual(1);
    expect(smoothLevel(0.5, -0.5, 0.8)).toBeGreaterThanOrEqual(0);
  });
});

// ---------------------------------------------------------------------------
// levelToBarStyle — percentage string for CSS width/height
// ---------------------------------------------------------------------------

describe("levelToBarStyle", () => {
  it("returns 0% for level 0", () => {
    expect(levelToBarStyle(0)).toBe("0%");
  });

  it("returns 100% for level 1", () => {
    expect(levelToBarStyle(1)).toBe("100%");
  });

  it("returns a percentage string with one decimal for mid values", () => {
    const s = levelToBarStyle(0.5);
    // Must be a CSS percentage string like "50.0%"
    expect(s).toMatch(/^\d+(\.\d+)?%$/);
    expect(parseFloat(s)).toBeCloseTo(50, 0);
  });

  it("clamps out-of-range inputs before converting", () => {
    expect(levelToBarStyle(-0.5)).toBe("0%");
    expect(levelToBarStyle(2.0)).toBe("100%");
  });
});

// ---------------------------------------------------------------------------
// peakDecay — fast-attack / slow-decay envelope for VU-style display
// ---------------------------------------------------------------------------

describe("peakDecay", () => {
  it("returns incoming level when there is no previous value", () => {
    expect(peakDecay(null, 0.8)).toBeCloseTo(0.8);
  });

  it("returns 0 when incoming is 0 and prev is null", () => {
    expect(peakDecay(null, 0.0)).toBe(0);
  });

  it("jumps instantly to a higher incoming peak (fast attack)", () => {
    // Previous display: 0.2; new incoming peak: 0.9 → must jump to 0.9
    const result = peakDecay(0.2, 0.9);
    expect(result).toBeCloseTo(0.9);
  });

  it("decays toward 0 when incoming signal drops to 0", () => {
    // With default decay=0.15: held = 1.0 * (1 - 0.15) = 0.85
    const result = peakDecay(1.0, 0.0);
    expect(result).toBeCloseTo(0.85);
  });

  it("decays all the way to 0 after many ticks of silence", () => {
    let v: number | null = 1.0;
    for (let i = 0; i < 200; i++) {
      v = peakDecay(v, 0.0);
    }
    expect(v).toBeLessThan(0.001);
  });

  it("holds the higher of decay and incoming", () => {
    // If incoming is higher than the decayed hold, use incoming
    const result = peakDecay(0.5, 0.9);
    // decayed hold = 0.5 * 0.85 = 0.425; incoming = 0.9 → result = 0.9
    expect(result).toBeCloseTo(0.9);
  });

  it("uses decayed hold when incoming is lower", () => {
    // prev=1.0, incoming=0.2, decay=0.15
    // decayed_hold = 1.0 * 0.85 = 0.85; incoming = 0.2 → result = 0.85
    const result = peakDecay(1.0, 0.2);
    expect(result).toBeCloseTo(0.85);
  });

  it("clamps returned level to [0, 1]", () => {
    expect(peakDecay(0.5, 1.5)).toBeLessThanOrEqual(1);
    expect(peakDecay(0.5, -0.5)).toBeGreaterThanOrEqual(0);
  });

  it("supports a custom decay coefficient", () => {
    // decay=0.5: held = 1.0 * (1 - 0.5) = 0.5
    const result = peakDecay(1.0, 0.0, 0.5);
    expect(result).toBeCloseTo(0.5);
  });
});

// ---------------------------------------------------------------------------
// LevelsPayload type shape
// ---------------------------------------------------------------------------

describe("LevelsPayload type", () => {
  it("is indexable by string → number", () => {
    const p: LevelsPayload = { Arctis_Game: 0.8, arctis_clean_mic: 0.6 };
    expect(p["Arctis_Game"]).toBe(0.8);
    expect(p["arctis_clean_mic"]).toBe(0.6);
  });
});
