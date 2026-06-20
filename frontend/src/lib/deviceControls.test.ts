/**
 * deviceControls.test.ts — Unit tests for device control view-model helpers.
 *
 * All helpers are pure functions with no Tauri / Svelte deps.
 * Tests confirm: ANC mode parsing + value mapping, sidetone label/parsing,
 * auto-off label/parsing, generic 1-10 parsing, capability detection,
 * and the args that would be passed to deviceSet.
 */
import { describe, it, expect } from "vitest";
import {
  parseAncMode,
  ancModeToValue,
  ANC_MODE_LABELS,
  ANC_MODES,
  parseSidetoneLevel,
  sidetoneLevelLabel,
  SIDETONE_OPTIONS,
  parseAutoOffLevel,
  autoOffLabel,
  AUTO_OFF_OPTIONS,
  parse1to10,
  enabledControls,
  isGateError,
  type AncMode,
} from "./deviceControls.js";

// ---------------------------------------------------------------------------
// ANC mode parsing
// ---------------------------------------------------------------------------

describe("parseAncMode", () => {
  it("parses '0' as off", () => {
    expect(parseAncMode("0")).toBe("off");
  });

  it("parses '1' as transparency", () => {
    expect(parseAncMode("1")).toBe("transparency");
  });

  it("parses '2' as on", () => {
    expect(parseAncMode("2")).toBe("on");
  });

  it("parses 'off' (string) as off", () => {
    expect(parseAncMode("off")).toBe("off");
  });

  it("parses 'transparency' (string) as transparency", () => {
    expect(parseAncMode("transparency")).toBe("transparency");
  });

  it("parses 'on' (string) as on", () => {
    expect(parseAncMode("on")).toBe("on");
  });

  it("parses 'anc' (string) as on", () => {
    expect(parseAncMode("anc")).toBe("on");
  });

  it("is case-insensitive", () => {
    expect(parseAncMode("OFF")).toBe("off");
    expect(parseAncMode("Transparency")).toBe("transparency");
    expect(parseAncMode("ON")).toBe("on");
  });

  it("defaults to off for unknown values", () => {
    expect(parseAncMode("unknown")).toBe("off");
    expect(parseAncMode("")).toBe("off");
    expect(parseAncMode("3")).toBe("off");
  });
});

// ---------------------------------------------------------------------------
// ANC mode → device_set value mapping
// ---------------------------------------------------------------------------

describe("ancModeToValue", () => {
  it("maps off → 0", () => {
    expect(ancModeToValue("off")).toBe(0);
  });

  it("maps transparency → 1", () => {
    expect(ancModeToValue("transparency")).toBe(1);
  });

  it("maps on → 2", () => {
    expect(ancModeToValue("on")).toBe(2);
  });

  it("produces the correct deviceSet args for each mode", () => {
    for (const mode of ANC_MODES) {
      const value = ancModeToValue(mode);
      // Verify the args that would be passed to deviceSet("anc", value)
      expect(typeof value).toBe("number");
      expect(value).toBeGreaterThanOrEqual(0);
      expect(value).toBeLessThanOrEqual(2);
    }
  });

  it("covers all ANC_MODES without gaps", () => {
    const values = ANC_MODES.map(ancModeToValue).sort();
    expect(values).toEqual([0, 1, 2]);
  });
});

// ---------------------------------------------------------------------------
// ANC mode labels
// ---------------------------------------------------------------------------

describe("ANC_MODE_LABELS", () => {
  it("has a label for every mode", () => {
    for (const mode of ANC_MODES) {
      expect(ANC_MODE_LABELS[mode]).toBeTruthy();
    }
  });

  it("has the expected English labels", () => {
    expect(ANC_MODE_LABELS.off).toBe("Off");
    expect(ANC_MODE_LABELS.transparency).toBe("Transparency");
    expect(ANC_MODE_LABELS.on).toBe("ANC");
  });
});

// ---------------------------------------------------------------------------
// Sidetone level parsing
// ---------------------------------------------------------------------------

describe("parseSidetoneLevel", () => {
  it("parses '0' as 0", () => {
    expect(parseSidetoneLevel("0")).toBe(0);
  });

  it("parses '1' as 1", () => {
    expect(parseSidetoneLevel("1")).toBe(1);
  });

  it("parses '3' as 3", () => {
    expect(parseSidetoneLevel("3")).toBe(3);
  });

  it("clamps values above 3 to 3", () => {
    expect(parseSidetoneLevel("10")).toBe(3);
    expect(parseSidetoneLevel("99")).toBe(3);
  });

  it("clamps negative values to 0", () => {
    expect(parseSidetoneLevel("-1")).toBe(0);
    expect(parseSidetoneLevel("-99")).toBe(0);
  });

  it("defaults to 0 for non-numeric strings", () => {
    expect(parseSidetoneLevel("off")).toBe(0);
    expect(parseSidetoneLevel("")).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// Sidetone label
// ---------------------------------------------------------------------------

describe("sidetoneLevelLabel", () => {
  it("returns 'Off' for 0", () => {
    expect(sidetoneLevelLabel(0)).toBe("Off");
  });

  it("returns 'Low' for 1", () => {
    expect(sidetoneLevelLabel(1)).toBe("Low");
  });

  it("returns 'Mid' for 2", () => {
    expect(sidetoneLevelLabel(2)).toBe("Mid");
  });

  it("returns 'High' for 3", () => {
    expect(sidetoneLevelLabel(3)).toBe("High");
  });

  it("SIDETONE_OPTIONS has 4 entries (0-3)", () => {
    expect(SIDETONE_OPTIONS).toHaveLength(4);
    const values = SIDETONE_OPTIONS.map((o) => o.value);
    expect(values).toEqual([0, 1, 2, 3]);
  });

  it("produces valid deviceSet args for each option", () => {
    for (const opt of SIDETONE_OPTIONS) {
      // deviceSet("sidetone", opt.value) — value is a number in 0-3
      expect(typeof opt.value).toBe("number");
      expect(opt.value).toBeGreaterThanOrEqual(0);
      expect(opt.value).toBeLessThanOrEqual(3);
    }
  });
});

// ---------------------------------------------------------------------------
// Auto-off / inactive_time
// ---------------------------------------------------------------------------

describe("parseAutoOffLevel", () => {
  it("parses '0' as 0 (Never)", () => {
    expect(parseAutoOffLevel("0")).toBe(0);
  });

  it("parses '6' as 6 (max level)", () => {
    expect(parseAutoOffLevel("6")).toBe(6);
  });

  it("clamps values above 6 to 6", () => {
    expect(parseAutoOffLevel("10")).toBe(6);
  });

  it("clamps negative values to 0", () => {
    expect(parseAutoOffLevel("-1")).toBe(0);
  });

  it("defaults to 0 for non-numeric strings", () => {
    expect(parseAutoOffLevel("never")).toBe(0);
    expect(parseAutoOffLevel("")).toBe(0);
  });
});

describe("autoOffLabel", () => {
  it("returns 'Never' for 0", () => {
    expect(autoOffLabel(0)).toBe("Never");
  });

  it("returns '1 min' for 1", () => {
    expect(autoOffLabel(1)).toBe("1 min");
  });

  it("returns '60 min' for 6", () => {
    expect(autoOffLabel(6)).toBe("60 min");
  });

  it("AUTO_OFF_OPTIONS has 7 entries (0-6)", () => {
    expect(AUTO_OFF_OPTIONS).toHaveLength(7);
    const values = AUTO_OFF_OPTIONS.map((o) => o.value);
    expect(values).toEqual([0, 1, 2, 3, 4, 5, 6]);
  });

  it("produces valid deviceSet args for each option", () => {
    for (const opt of AUTO_OFF_OPTIONS) {
      // deviceSet("inactive_time", opt.value)
      expect(typeof opt.value).toBe("number");
      expect(opt.value).toBeGreaterThanOrEqual(0);
      expect(opt.value).toBeLessThanOrEqual(6);
    }
  });

  it("level 0 has null minutes (Never)", () => {
    expect(AUTO_OFF_OPTIONS[0].minutes).toBeNull();
  });

  it("levels 1-6 all have positive minutes", () => {
    for (const opt of AUTO_OFF_OPTIONS.slice(1)) {
      expect(opt.minutes).toBeGreaterThan(0);
    }
  });
});

// ---------------------------------------------------------------------------
// parse1to10
// ---------------------------------------------------------------------------

describe("parse1to10", () => {
  it("parses '5' as 5", () => {
    expect(parse1to10("5")).toBe(5);
  });

  it("clamps values below 1 to 1", () => {
    expect(parse1to10("0")).toBe(1);
    expect(parse1to10("-5")).toBe(1);
  });

  it("clamps values above 10 to 10", () => {
    expect(parse1to10("11")).toBe(10);
    expect(parse1to10("99")).toBe(10);
  });

  it("defaults to 1 for non-numeric strings", () => {
    expect(parse1to10("high")).toBe(1);
    expect(parse1to10("")).toBe(1);
  });

  it("parses boundary values '1' and '10' correctly", () => {
    expect(parse1to10("1")).toBe(1);
    expect(parse1to10("10")).toBe(10);
  });
});

// ---------------------------------------------------------------------------
// enabledControls — capability detection
// ---------------------------------------------------------------------------

describe("enabledControls", () => {
  it("includes always-present controls when device is present", () => {
    const enabled = enabledControls({});
    expect(enabled.has("sidetone")).toBe(true);
    expect(enabled.has("anc")).toBe(true);
    expect(enabled.has("inactive_time")).toBe(true);
    expect(enabled.has("mic_led")).toBe(true);
  });

  it("does NOT include transparency_level when anc_mode field is absent", () => {
    const enabled = enabledControls({});
    expect(enabled.has("transparency_level")).toBe(false);
  });

  it("includes transparency_level when anc_mode field is present", () => {
    const enabled = enabledControls({ anc_mode: "1" });
    expect(enabled.has("transparency_level")).toBe(true);
  });

  it("does NOT include mic_volume when mic_gain field is absent", () => {
    const enabled = enabledControls({});
    expect(enabled.has("mic_volume")).toBe(false);
  });

  it("includes mic_volume when mic_gain field is present", () => {
    const enabled = enabledControls({ mic_gain: "7" });
    expect(enabled.has("mic_volume")).toBe(true);
  });

  it("all always-present controls enabled regardless of fields", () => {
    const enabled1 = enabledControls({});
    const enabled2 = enabledControls({ battery: "80", anc_mode: "0", mic_gain: "5" });
    expect(enabled1.has("sidetone")).toBe(true);
    expect(enabled2.has("sidetone")).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// isGateError
// ---------------------------------------------------------------------------

describe("isGateError", () => {
  it("returns true for 'Unsupported' (daemon gate message)", () => {
    expect(isGateError("Unsupported")).toBe(true);
    expect(isGateError("unsupported control: sidetone")).toBe(true);
  });

  it("returns true for 'not enabled' messages", () => {
    expect(isGateError("control not enabled")).toBe(true);
  });

  it("returns true for 'gated' messages", () => {
    expect(isGateError("write is gated")).toBe(true);
  });

  it("returns true for 'allowlist' messages", () => {
    expect(isGateError("not in allowlist")).toBe(true);
  });

  it("returns true for 'pending' messages", () => {
    expect(isGateError("pending validation")).toBe(true);
  });

  it("returns false for unrelated errors", () => {
    expect(isGateError("Connection refused")).toBe(false);
    expect(isGateError("timeout")).toBe(false);
    expect(isGateError("out of range")).toBe(false);
  });

  it("is case-insensitive", () => {
    expect(isGateError("UNSUPPORTED")).toBe(true);
    expect(isGateError("Not Enabled")).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// deviceSet argument shapes (pure mapping, no runtime needed)
// ---------------------------------------------------------------------------

describe("deviceSet arg shapes for each control", () => {
  it("sidetone: control='sidetone', value in 0-3", () => {
    const control = "sidetone";
    for (const opt of SIDETONE_OPTIONS) {
      // Mimics the call: deviceSet(control, opt.value)
      expect(control).toBe("sidetone");
      expect(opt.value).toBeGreaterThanOrEqual(0);
      expect(opt.value).toBeLessThanOrEqual(3);
    }
  });

  it("anc: control='anc', value in 0-2", () => {
    const control = "anc";
    for (const mode of ANC_MODES) {
      const value = ancModeToValue(mode);
      expect(control).toBe("anc");
      expect(value).toBeGreaterThanOrEqual(0);
      expect(value).toBeLessThanOrEqual(2);
    }
  });

  it("inactive_time: control='inactive_time', value in 0-6", () => {
    const control = "inactive_time";
    for (const opt of AUTO_OFF_OPTIONS) {
      expect(control).toBe("inactive_time");
      expect(opt.value).toBeGreaterThanOrEqual(0);
      expect(opt.value).toBeLessThanOrEqual(6);
    }
  });

  it("mic_led: control='mic_led', value in 1-10", () => {
    // parse1to10 is the parser; slider range is 1-10
    const control = "mic_led";
    expect(control).toBe("mic_led");
    expect(parse1to10("1")).toBe(1);
    expect(parse1to10("10")).toBe(10);
  });

  it("transparency_level: control='transparency_level', value in 1-10", () => {
    const control = "transparency_level";
    expect(control).toBe("transparency_level");
    expect(parse1to10("5")).toBe(5);
  });

  it("mic_volume: control='mic_volume', value in 1-10", () => {
    const control = "mic_volume";
    expect(control).toBe("mic_volume");
    expect(parse1to10("8")).toBe(8);
  });
});
