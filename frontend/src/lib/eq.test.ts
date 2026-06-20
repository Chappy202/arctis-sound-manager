/**
 * eq.test.ts — Unit tests for the pure EQ math module.
 *
 * These run in vitest (Node environment, no DOM/Tauri) as the CI-verifiable
 * coverage for the parametric-EQ feature.
 */
import { describe, expect, it } from "vitest";
import {
  FREQ_MAX,
  FREQ_MIN,
  GAIN_MAX,
  GAIN_MIN,
  Q_MAX,
  Q_MIN,
  type Band,
  bandMagnitudeDb,
  clampBand,
  freqToX,
  gainToY,
  logFreqAxis,
  summedCurveDb,
  xToFreq,
  yToGain,
} from "./eq.js";

// ---------------------------------------------------------------------------
// Coordinate mapping
// ---------------------------------------------------------------------------

describe("freqToX / xToFreq", () => {
  const W = 800;

  it("maps FREQ_MIN to 0", () => {
    expect(freqToX(FREQ_MIN, W)).toBeCloseTo(0);
  });

  it("maps FREQ_MAX to width", () => {
    expect(freqToX(FREQ_MAX, W)).toBeCloseTo(W);
  });

  it("maps 1 kHz to roughly the middle of the log scale", () => {
    // log10(1000/20) / log10(20000/20) = log10(50)/log10(1000) ≈ 0.566
    const x = freqToX(1000, W);
    expect(x).toBeGreaterThan(0);
    expect(x).toBeLessThan(W);
  });

  it("round-trips xToFreq(freqToX(f)) ≈ f for arbitrary freqs", () => {
    for (const f of [20, 100, 500, 1000, 5000, 20000]) {
      expect(xToFreq(freqToX(f, W), W)).toBeCloseTo(f, 3);
    }
  });
});

describe("gainToY / yToGain", () => {
  const H = 300;

  it("maps 0 dB to the vertical centre", () => {
    expect(gainToY(0, H)).toBeCloseTo(H / 2);
  });

  it("maps GAIN_MAX to 0 (top)", () => {
    expect(gainToY(GAIN_MAX, H)).toBeCloseTo(0);
  });

  it("maps GAIN_MIN to H (bottom)", () => {
    expect(gainToY(GAIN_MIN, H)).toBeCloseTo(H);
  });

  it("round-trips yToGain(gainToY(g)) ≈ g", () => {
    for (const g of [-12, -6, 0, 3, 6, 12]) {
      expect(yToGain(gainToY(g, H), H)).toBeCloseTo(g, 5);
    }
  });
});

// ---------------------------------------------------------------------------
// clampBand
// ---------------------------------------------------------------------------

describe("clampBand", () => {
  it("passes through in-range values unchanged", () => {
    const b: Band = { kind: "peaking", freqHz: 1000, q: 1.0, gainDb: 0 };
    const c = clampBand(b);
    expect(c.freqHz).toBe(1000);
    expect(c.q).toBe(1.0);
    expect(c.gainDb).toBe(0);
  });

  it("clamps freqHz below FREQ_MIN to FREQ_MIN", () => {
    const b: Band = { kind: "peaking", freqHz: 1, q: 1, gainDb: 0 };
    expect(clampBand(b).freqHz).toBe(FREQ_MIN);
  });

  it("clamps freqHz above FREQ_MAX to FREQ_MAX", () => {
    const b: Band = { kind: "peaking", freqHz: 999999, q: 1, gainDb: 0 };
    expect(clampBand(b).freqHz).toBe(FREQ_MAX);
  });

  it("clamps gainDb below GAIN_MIN to GAIN_MIN", () => {
    const b: Band = { kind: "peaking", freqHz: 1000, q: 1, gainDb: -100 };
    expect(clampBand(b).gainDb).toBe(GAIN_MIN);
  });

  it("clamps gainDb above GAIN_MAX to GAIN_MAX", () => {
    const b: Band = { kind: "peaking", freqHz: 1000, q: 1, gainDb: 100 };
    expect(clampBand(b).gainDb).toBe(GAIN_MAX);
  });

  it("clamps Q below Q_MIN to Q_MIN", () => {
    const b: Band = { kind: "peaking", freqHz: 1000, q: 0, gainDb: 0 };
    expect(clampBand(b).q).toBe(Q_MIN);
  });

  it("clamps Q above Q_MAX to Q_MAX", () => {
    const b: Band = { kind: "peaking", freqHz: 1000, q: 999, gainDb: 0 };
    expect(clampBand(b).q).toBe(Q_MAX);
  });
});

// ---------------------------------------------------------------------------
// bandMagnitudeDb
// ---------------------------------------------------------------------------

describe("bandMagnitudeDb — peaking", () => {
  const band: Band = { kind: "peaking", freqHz: 1000, q: 1.0, gainDb: 6 };

  it("flat band (gain=0) yields ~0 dB everywhere", () => {
    const flat: Band = { kind: "peaking", freqHz: 1000, q: 1.0, gainDb: 0 };
    for (const f of [100, 500, 1000, 5000, 10000]) {
      expect(bandMagnitudeDb(flat, f)).toBeCloseTo(0, 5);
    }
  });

  it("+6 dB peak at 1 kHz peaks near +6 dB at 1 kHz", () => {
    const peak = bandMagnitudeDb(band, 1000);
    expect(peak).toBeCloseTo(6, 1);
  });

  it("+6 dB peak at 1 kHz is much lower at 100 Hz (far off-center)", () => {
    const offPeak = bandMagnitudeDb(band, 100);
    expect(offPeak).toBeLessThan(3);
  });

  it("+6 dB peak at 1 kHz is much lower at 10 kHz (far off-center)", () => {
    const offPeak = bandMagnitudeDb(band, 10000);
    expect(offPeak).toBeLessThan(3);
  });

  it("negative gain (−6 dB) dips near −6 dB at center freq", () => {
    const cut: Band = { kind: "peaking", freqHz: 1000, q: 1.0, gainDb: -6 };
    expect(bandMagnitudeDb(cut, 1000)).toBeCloseTo(-6, 1);
  });
});

describe("bandMagnitudeDb — low shelf", () => {
  it("+6 dB low-shelf with fc=100 Hz is near +6 dB far below shelf freq", () => {
    const band: Band = { kind: "lowshelf", freqHz: 100, q: 0.707, gainDb: 6 };
    // 20 Hz is well below the shelf — should be close to +6 dB
    const gain = bandMagnitudeDb(band, 20);
    expect(gain).toBeGreaterThan(4);
    expect(gain).toBeLessThanOrEqual(7);
  });

  it("+6 dB low-shelf is near 0 dB far above the shelf freq", () => {
    const band: Band = { kind: "lowshelf", freqHz: 100, q: 0.707, gainDb: 6 };
    const gain = bandMagnitudeDb(band, 10000);
    expect(Math.abs(gain)).toBeLessThan(1.5);
  });
});

describe("bandMagnitudeDb — high shelf", () => {
  it("+6 dB high-shelf with fc=8 kHz is near +6 dB far above shelf freq", () => {
    const band: Band = { kind: "highshelf", freqHz: 8000, q: 0.707, gainDb: 6 };
    const gain = bandMagnitudeDb(band, 18000);
    expect(gain).toBeGreaterThan(4);
    expect(gain).toBeLessThanOrEqual(7);
  });

  it("+6 dB high-shelf is near 0 dB far below the shelf freq", () => {
    const band: Band = { kind: "highshelf", freqHz: 8000, q: 0.707, gainDb: 6 };
    const gain = bandMagnitudeDb(band, 200);
    expect(Math.abs(gain)).toBeLessThan(1.5);
  });
});

// ---------------------------------------------------------------------------
// summedCurveDb
// ---------------------------------------------------------------------------

describe("summedCurveDb", () => {
  it("returns an array of the same length as freqs", () => {
    const freqs = [100, 500, 1000, 5000];
    const result = summedCurveDb([], freqs);
    expect(result).toHaveLength(freqs.length);
  });

  it("empty bands array → 0 dB at all frequencies", () => {
    const freqs = [100, 500, 1000, 5000];
    const result = summedCurveDb([], freqs);
    result.forEach((v) => expect(v).toBeCloseTo(0, 5));
  });

  it("multiple flat bands still sum to ~0 dB", () => {
    const flat: Band = { kind: "peaking", freqHz: 1000, q: 1, gainDb: 0 };
    const result = summedCurveDb([flat, flat, flat], [1000]);
    expect(result[0]).toBeCloseTo(0, 4);
  });

  it("sums two peaks: two +3 dB peaks at same freq ≈ +6 dB total", () => {
    const b1: Band = { kind: "peaking", freqHz: 1000, q: 1.0, gainDb: 3 };
    const b2: Band = { kind: "peaking", freqHz: 1000, q: 1.0, gainDb: 3 };
    const result = summedCurveDb([b1, b2], [1000]);
    expect(result[0]).toBeCloseTo(6, 0);
  });
});

// ---------------------------------------------------------------------------
// logFreqAxis
// ---------------------------------------------------------------------------

describe("logFreqAxis", () => {
  it("first element is FREQ_MIN", () => {
    expect(logFreqAxis(100)[0]).toBeCloseTo(FREQ_MIN);
  });

  it("last element is FREQ_MAX", () => {
    const axis = logFreqAxis(100);
    expect(axis[axis.length - 1]).toBeCloseTo(FREQ_MAX, 3);
  });

  it("returns the requested number of samples", () => {
    expect(logFreqAxis(256)).toHaveLength(256);
  });

  it("values are strictly increasing", () => {
    const axis = logFreqAxis(50);
    for (let i = 1; i < axis.length; i++) {
      expect(axis[i]).toBeGreaterThan(axis[i - 1]);
    }
  });
});
