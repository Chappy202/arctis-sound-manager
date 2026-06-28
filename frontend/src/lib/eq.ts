/**
 * eq.ts — Pure parametric-EQ math for Arctis Sound Manager.
 *
 * All functions are pure (no side effects, no DOM, no Tauri) so they can be
 * unit-tested with vitest in a Node environment.
 *
 * Transfer-function math follows the RBJ Audio EQ Cookbook (R. Bristow-Johnson).
 * This module is display-only: it computes the visual frequency-response curve.
 * The actual DSP runs inside the daemon at 48 kHz.
 */

// ---------------------------------------------------------------------------
// Public types & constants
// ---------------------------------------------------------------------------

export interface Band {
  kind: "peaking" | "lowshelf" | "highshelf";
  freqHz: number;
  q: number;
  gainDb: number;
}

export const FREQ_MIN = 20;
export const FREQ_MAX = 20000;
export const GAIN_MIN = -12;
export const GAIN_MAX = 12;
export const Q_MIN = 0.3;
export const Q_MAX = 10;
export const DEFAULT_SAMPLE_RATE = 48000;

/** Standard 10-band center frequencies (logarithmically spaced, 31 Hz – 16 kHz), matching the engine's EqModel::default_10band(). */
export const DEFAULT_BAND_FREQS: readonly number[] = [
  31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000,
];

// ---------------------------------------------------------------------------
// Coordinate mapping (log-freq X, linear-gain Y)
// ---------------------------------------------------------------------------

/**
 * Map a frequency (Hz) to an X pixel coordinate on a canvas of the given width.
 * Uses a logarithmic scale from FREQ_MIN to FREQ_MAX.
 */
export function freqToX(freqHz: number, width: number): number {
  return (
    (Math.log10(freqHz / FREQ_MIN) / Math.log10(FREQ_MAX / FREQ_MIN)) * width
  );
}

/**
 * Inverse of freqToX: pixel X → frequency (Hz).
 */
export function xToFreq(x: number, width: number): number {
  const t = x / width;
  return FREQ_MIN * Math.pow(FREQ_MAX / FREQ_MIN, t);
}

/**
 * Map a gain value (dB) to a Y pixel coordinate.
 * gainDb=GAIN_MAX → y=0 (top), gainDb=GAIN_MIN → y=height (bottom).
 * 0 dB maps to the vertical centre (y = height/2).
 */
export function gainToY(gainDb: number, height: number): number {
  return ((GAIN_MAX - gainDb) / (GAIN_MAX - GAIN_MIN)) * height;
}

/**
 * Inverse of gainToY: pixel Y → gain (dB).
 */
export function yToGain(y: number, height: number): number {
  return GAIN_MAX - (y / height) * (GAIN_MAX - GAIN_MIN);
}

// ---------------------------------------------------------------------------
// Clamping
// ---------------------------------------------------------------------------

/** Return a copy of band with freq/gain/Q clamped to legal ranges. */
export function clampBand(b: Band): Band {
  return {
    kind: b.kind,
    freqHz: Math.min(FREQ_MAX, Math.max(FREQ_MIN, b.freqHz)),
    q: Math.min(Q_MAX, Math.max(Q_MIN, b.q)),
    gainDb: Math.min(GAIN_MAX, Math.max(GAIN_MIN, b.gainDb)),
  };
}

// ---------------------------------------------------------------------------
// Biquad magnitude response (RBJ Audio EQ Cookbook)
// ---------------------------------------------------------------------------

/**
 * Compute the magnitude response (in dB) of a single biquad filter at a
 * given evaluation frequency. sampleRate defaults to 48000 Hz (daemon rate).
 *
 * Uses the RBJ Audio EQ Cookbook formulas for:
 *   - peaking EQ  (bandMagnitudeDb, kind="peaking")
 *   - low shelf   (kind="lowshelf")
 *   - high shelf  (kind="highshelf")
 *
 * The |H(e^jω)| is computed from the filter coefficients (b0,b1,b2,a0,a1,a2)
 * via direct evaluation of the z-domain transfer function on the unit circle:
 *   |H(e^jω)| = |B(e^jω)| / |A(e^jω)|
 * where B, A are polynomials in e^{-jω} evaluated at ω = 2π*f/fs.
 */
export function bandMagnitudeDb(
  band: Band,
  evalFreqHz: number,
  sampleRate = DEFAULT_SAMPLE_RATE,
): number {
  const { kind, freqHz, q, gainDb } = band;

  // Corner case: flat band → 0 dB everywhere.
  if (gainDb === 0 && kind === "peaking") return 0;

  const A = Math.pow(10, gainDb / 40); // sqrt of linear gain amplitude
  const w0 = (2 * Math.PI * freqHz) / sampleRate;
  const cosW0 = Math.cos(w0);
  const sinW0 = Math.sin(w0);
  const alpha = sinW0 / (2 * q);

  let b0: number, b1: number, b2: number, a0: number, a1: number, a2: number;

  switch (kind) {
    case "peaking":
      b0 = 1 + alpha * A;
      b1 = -2 * cosW0;
      b2 = 1 - alpha * A;
      a0 = 1 + alpha / A;
      a1 = -2 * cosW0;
      a2 = 1 - alpha / A;
      break;

    case "lowshelf": {
      const sqrtA = Math.sqrt(A);
      const alphaMul = 2 * sqrtA * alpha;
      b0 = A * ((A + 1) - (A - 1) * cosW0 + alphaMul);
      b1 = 2 * A * ((A - 1) - (A + 1) * cosW0);
      b2 = A * ((A + 1) - (A - 1) * cosW0 - alphaMul);
      a0 = (A + 1) + (A - 1) * cosW0 + alphaMul;
      a1 = -2 * ((A - 1) + (A + 1) * cosW0);
      a2 = (A + 1) + (A - 1) * cosW0 - alphaMul;
      break;
    }

    case "highshelf": {
      const sqrtA = Math.sqrt(A);
      const alphaMul = 2 * sqrtA * alpha;
      b0 = A * ((A + 1) + (A - 1) * cosW0 + alphaMul);
      b1 = -2 * A * ((A - 1) + (A + 1) * cosW0);
      b2 = A * ((A + 1) + (A - 1) * cosW0 - alphaMul);
      a0 = (A + 1) - (A - 1) * cosW0 + alphaMul;
      a1 = 2 * ((A - 1) - (A + 1) * cosW0);
      a2 = (A + 1) - (A - 1) * cosW0 - alphaMul;
      break;
    }
  }

  // Evaluate H(z) on the unit circle at z = e^{jω_eval}
  const wEval = (2 * Math.PI * evalFreqHz) / sampleRate;
  const cosE = Math.cos(wEval);
  const cos2E = Math.cos(2 * wEval);
  const sinE = Math.sin(wEval);
  const sin2E = Math.sin(2 * wEval);

  // Numerator: B(e^{jω}) = b0 + b1·e^{-jω} + b2·e^{-2jω}
  const bRe = b0 + b1 * cosE + b2 * cos2E;
  const bIm = -(b1 * sinE + b2 * sin2E);

  // Denominator: A(e^{jω}) = a0 + a1·e^{-jω} + a2·e^{-2jω}
  const aRe = a0 + a1 * cosE + a2 * cos2E;
  const aIm = -(a1 * sinE + a2 * sin2E);

  const bMag2 = bRe * bRe + bIm * bIm;
  const aMag2 = aRe * aRe + aIm * aIm;

  if (aMag2 === 0) return 0;

  return 10 * Math.log10(bMag2 / aMag2);
}

// ---------------------------------------------------------------------------
// Multi-band summed curve
// ---------------------------------------------------------------------------

/**
 * Compute the summed magnitude-response curve (dB) for an array of bands at
 * the given frequency sample points.
 *
 * In dB domain, per-band contributions are additive (linear multiplication
 * becomes dB addition). Returns an array of the same length as `freqs`.
 */
export function summedCurveDb(
  bands: Band[],
  freqs: number[],
  sampleRate = DEFAULT_SAMPLE_RATE,
): number[] {
  return freqs.map((f) => {
    let total = 0;
    for (const band of bands) {
      total += bandMagnitudeDb(band, f, sampleRate);
    }
    return total;
  });
}

// ---------------------------------------------------------------------------
// Frequency axis helper
// ---------------------------------------------------------------------------

/**
 * Return `samples` logarithmically-spaced frequency values from FREQ_MIN to
 * FREQ_MAX (inclusive). Used to sample the curve for canvas rendering.
 */
export function logFreqAxis(samples: number): number[] {
  if (samples < 2) return [FREQ_MIN];
  const result: number[] = [];
  for (let i = 0; i < samples; i++) {
    const t = i / (samples - 1);
    result.push(FREQ_MIN * Math.pow(FREQ_MAX / FREQ_MIN, t));
  }
  return result;
}

// ---------------------------------------------------------------------------
// Default band factory
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// State reconciliation
// ---------------------------------------------------------------------------

/** Deep value-equality for two band arrays (kind/freq/Q/gain). Pure. */
export function bandsEqual(a: Band[], b: Band[]): boolean {
  if (a === b) return true;
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i];
    const y = b[i];
    if (
      x.kind !== y.kind ||
      x.freqHz !== y.freqHz ||
      x.q !== y.q ||
      x.gainDb !== y.gainDb
    ) {
      return false;
    }
  }
  return true;
}

/**
 * Reconcile the locally-edited band array against an incoming engine snapshot.
 * While the user is editing (any modality), keep the local array unchanged so a
 * background state refresh can never clobber an in-progress edit. When idle,
 * adopt the incoming array — but only when it actually differs in value, so an
 * unchanged snapshot returns the SAME `local` reference. That referential
 * stability is what stops the EQ curve (and every band-derived value) from
 * recomputing on each background state refresh. Pure — unit-testable.
 */
export function reconcileBands(local: Band[], incoming: Band[], editing: boolean): Band[] {
  if (editing) return local;
  return bandsEqual(local, incoming) ? local : incoming;
}
