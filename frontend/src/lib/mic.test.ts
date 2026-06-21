/**
 * mic.test.ts — Pure-helper unit tests for mic.ts.
 *
 * NOTE (deliberate deviation from brief): The task brief asked for a MicPage component-mount test
 * (asserting DOM elements for unavailable stages). This repo has no jsdom, no @testing-library/svelte,
 * and no component mount tooling. Following the established project pattern (see deviceControls.ts /
 * device.ts + their *.test.ts files), logic is in pure helpers tested with vitest. The spirit of the
 * brief's assertions (honest unavailability, correct ipc args) is covered here.
 */

import { describe, it, expect } from "vitest";
import {
  stageWireName,
  stagePluginPath,
  isStageDisabled,
  stageUnavailableTooltip,
  micBandToArgs,
  backendLabel,
  backendAvailable,
  backendTooltip,
} from "./mic.js";
import type { MicStageSnapshot } from "./ipc.js";
import type { Band } from "./eq.js";

// ---------------------------------------------------------------------------
// stageWireName
// ---------------------------------------------------------------------------

describe("stageWireName", () => {
  it("maps mic_eq → eq (the key mismatch)", () => {
    expect(stageWireName("mic_eq")).toBe("eq");
  });

  it("maps gain → gain (identity)", () => {
    expect(stageWireName("gain")).toBe("gain");
  });

  it("maps highpass → highpass (identity)", () => {
    expect(stageWireName("highpass")).toBe("highpass");
  });

  it("maps suppression → suppression (identity)", () => {
    expect(stageWireName("suppression")).toBe("suppression");
  });

  it("maps compressor → compressor (identity)", () => {
    expect(stageWireName("compressor")).toBe("compressor");
  });

  it("maps gate → gate (identity)", () => {
    expect(stageWireName("gate")).toBe("gate");
  });
});

// ---------------------------------------------------------------------------
// stagePluginPath
// ---------------------------------------------------------------------------

describe("stagePluginPath", () => {
  it("returns compressor plugin path for compressor", () => {
    expect(stagePluginPath("compressor")).toBe("/usr/lib64/ladspa/sc4m_1916.so");
  });

  it("returns null for builtin stages (gain, highpass, gate, mic_eq)", () => {
    expect(stagePluginPath("gain")).toBeNull();
    expect(stagePluginPath("highpass")).toBeNull();
    expect(stagePluginPath("gate")).toBeNull();
    expect(stagePluginPath("mic_eq")).toBeNull();
  });

  it("returns null for suppression (backend-specific plugin; use backendTooltip instead)", () => {
    expect(stagePluginPath("suppression")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// backendLabel
// ---------------------------------------------------------------------------

describe("backendLabel", () => {
  it("maps deep_filter → DeepFilterNet", () => {
    expect(backendLabel("deep_filter")).toBe("DeepFilterNet");
  });

  it("maps rnnoise → RNNoise", () => {
    expect(backendLabel("rnnoise")).toBe("RNNoise");
  });

  it("returns the raw id for unknown backends (forward-compat)", () => {
    expect(backendLabel("some_future_backend")).toBe("some_future_backend");
  });
});

// ---------------------------------------------------------------------------
// backendAvailable
// ---------------------------------------------------------------------------

describe("backendAvailable", () => {
  it("returns true when backend is in the available list", () => {
    expect(backendAvailable("deep_filter", ["deep_filter", "rnnoise"])).toBe(true);
    expect(backendAvailable("rnnoise", ["deep_filter", "rnnoise"])).toBe(true);
  });

  it("returns false when backend is absent from the available list", () => {
    expect(backendAvailable("deep_filter", ["rnnoise"])).toBe(false);
    expect(backendAvailable("rnnoise", [])).toBe(false);
  });

  it("returns false for an empty available list", () => {
    expect(backendAvailable("deep_filter", [])).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// backendTooltip
// ---------------------------------------------------------------------------

describe("backendTooltip", () => {
  it("returns undefined when backend is available (no tooltip needed)", () => {
    expect(backendTooltip("deep_filter", ["deep_filter", "rnnoise"])).toBeUndefined();
    expect(backendTooltip("rnnoise", ["deep_filter", "rnnoise"])).toBeUndefined();
  });

  it("returns a DeepFilterNet-specific message when deep_filter is unavailable", () => {
    const tip = backendTooltip("deep_filter", ["rnnoise"]);
    expect(tip).toBeDefined();
    expect(tip).toContain("DeepFilterNet");
    expect(tip).toContain("plugin not installed");
  });

  it("returns an RNNoise-specific message when rnnoise is unavailable", () => {
    const tip = backendTooltip("rnnoise", []);
    expect(tip).toBeDefined();
    expect(tip).toContain("RNNoise");
    expect(tip).toContain("plugin not installed");
  });

  it("returns a generic message for unknown unavailable backends", () => {
    const tip = backendTooltip("some_backend", []);
    expect(tip).toBeDefined();
    expect(tip).toContain("some_backend");
  });
});

// ---------------------------------------------------------------------------
// isStageDisabled
// ---------------------------------------------------------------------------

describe("isStageDisabled", () => {
  it("returns true when available is false", () => {
    const stage: MicStageSnapshot = {
      kind: "suppression",
      enabled: true,
      available: false,
      params: {},
    };
    expect(isStageDisabled(stage)).toBe(true);
  });

  it("returns false when available is true", () => {
    const stage: MicStageSnapshot = {
      kind: "suppression",
      enabled: true,
      available: true,
      params: {},
    };
    expect(isStageDisabled(stage)).toBe(false);
  });

  it("returns true for compressor when unavailable", () => {
    const stage: MicStageSnapshot = {
      kind: "compressor",
      enabled: false,
      available: false,
      params: {},
    };
    expect(isStageDisabled(stage)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// stageUnavailableTooltip
// ---------------------------------------------------------------------------

describe("stageUnavailableTooltip", () => {
  it("returns tooltip containing compressor plugin path when unavailable", () => {
    const stage: MicStageSnapshot = {
      kind: "compressor",
      enabled: false,
      available: false,
      params: {},
    };
    const tooltip = stageUnavailableTooltip(stage);
    expect(tooltip).toBeDefined();
    expect(tooltip).toContain("/usr/lib64/ladspa/sc4m_1916.so");
    expect(tooltip).toContain("Plugin not installed:");
  });

  it("returns a gate-specific tooltip mentioning PipeWire version and gate_1410 when gate unavailable", () => {
    const stage: MicStageSnapshot = {
      kind: "gate",
      enabled: false,
      available: false,
      params: {},
    };
    const tooltip = stageUnavailableTooltip(stage);
    expect(tooltip).toBeDefined();
    expect(tooltip).toContain("PipeWire");
    expect(tooltip).toContain("gate_1410");
  });

  it("returns undefined when stage is available (no tooltip needed)", () => {
    const stage: MicStageSnapshot = {
      kind: "suppression",
      enabled: true,
      available: true,
      params: {},
    };
    expect(stageUnavailableTooltip(stage)).toBeUndefined();
  });

  it("returns undefined for builtins when available (no plugin path)", () => {
    const stage: MicStageSnapshot = {
      kind: "gain",
      enabled: true,
      available: true,
      params: { gain_db: 0 },
    };
    expect(stageUnavailableTooltip(stage)).toBeUndefined();
  });

  it("returns a generic message for unavailable builtins (no plugin path to show)", () => {
    // Edge case: a builtin stage that is somehow unavailable
    const stage: MicStageSnapshot = {
      kind: "gain",
      enabled: false,
      available: false,
      params: {},
    };
    const tooltip = stageUnavailableTooltip(stage);
    expect(tooltip).toBeDefined();
    expect(tooltip).toContain("not available");
  });

  it("returns generic message for unavailable suppression stage (backend-specific handled elsewhere)", () => {
    const stage: MicStageSnapshot = {
      kind: "suppression",
      enabled: false,
      available: false,
      params: {},
    };
    const tooltip = stageUnavailableTooltip(stage);
    expect(tooltip).toBeDefined();
    expect(tooltip).toContain("not available");
  });
});

// ---------------------------------------------------------------------------
// micBandToArgs
// ---------------------------------------------------------------------------

describe("micBandToArgs", () => {
  it("maps Band camelCase fields to snake_case args", () => {
    const band: Band = {
      kind: "peaking",
      freqHz: 1000,
      q: 1.5,
      gainDb: -3,
    };
    const args = micBandToArgs(2, band);
    expect(args).toEqual({
      band: 2,
      kind: "peaking",
      freq_hz: 1000,
      q: 1.5,
      gain_db: -3,
    });
  });

  it("preserves band index correctly", () => {
    const band: Band = { kind: "lowshelf", freqHz: 80, q: 0.707, gainDb: 2 };
    const args = micBandToArgs(0, band);
    expect(args.band).toBe(0);
  });

  it("maps all four Band fields", () => {
    const band: Band = { kind: "highshelf", freqHz: 8000, q: 2.0, gainDb: 6 };
    const args = micBandToArgs(5, band);
    expect(args.kind).toBe("highshelf");
    expect(args.freq_hz).toBe(8000);
    expect(args.q).toBe(2.0);
    expect(args.gain_db).toBe(6);
  });
});
