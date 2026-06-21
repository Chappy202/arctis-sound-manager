/**
 * meter.test.ts — Unit tests for the pure level-meter helper module.
 *
 * These run in vitest (Node, no DOM / Tauri).  All functions under test are
 * pure transformations so they can be exercised without any runtime.
 *
 * What the meters actually show (honesty note):
 *   The `levels` event carries the *configured software volume* (0.0–1.0
 *   linear) sampled from pw-dump's Props.channelVolumes for each Arctis sink /
 *   source.  This is NOT a real-time audio signal peak or RMS — it reflects
 *   the user's volume setting, not signal activity.  True peak metering
 *   requires a native pipewire-rs capture stream (documented as a follow-up).
 */
import { describe, expect, it } from "vitest";
import {
  clampLevel,
  linearToPercent,
  smoothLevel,
  levelToBarStyle,
  type LevelsPayload,
  buildLevelsPayload,
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
// smoothLevel — exponential smoothing
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
// buildLevelsPayload — maps node_name → linear volume from pw-dump data
// ---------------------------------------------------------------------------

describe("buildLevelsPayload", () => {
  // Simulate the JSON structure pw-dump emits for a node with Props.channelVolumes
  const fakePwDump = [
    {
      type: "PipeWire:Interface:Node",
      id: 101,
      info: {
        props: { "node.name": "Arctis_Game" },
        params: {
          Props: [{ channelVolumes: [0.75, 0.75] }],
        },
      },
    },
    {
      type: "PipeWire:Interface:Node",
      id: 102,
      info: {
        props: { "node.name": "Arctis_Chat" },
        params: {
          Props: [{ channelVolumes: [0.5] }],
        },
      },
    },
    {
      type: "PipeWire:Interface:Node",
      id: 103,
      info: {
        props: { "node.name": "Arctis_Media" },
        params: {
          Props: [{ channelVolumes: [1.0, 1.0] }],
        },
      },
    },
    {
      type: "PipeWire:Interface:Node",
      id: 104,
      info: {
        props: { "node.name": "arctis_clean_mic" },
        params: {
          Props: [{ channelVolumes: [0.9] }],
        },
      },
    },
    {
      // Non-Arctis node — must be ignored
      type: "PipeWire:Interface:Node",
      id: 50,
      info: {
        props: { "node.name": "alsa_output.pci-0000" },
        params: {
          Props: [{ channelVolumes: [0.3, 0.3] }],
        },
      },
    },
    {
      // Non-node entry — must be ignored
      type: "PipeWire:Interface:Link",
      id: 200,
    },
  ];

  const TARGET_NODES = ["Arctis_Game", "Arctis_Chat", "Arctis_Media", "arctis_clean_mic"];

  it("extracts levels for all target Arctis nodes", () => {
    const payload = buildLevelsPayload(fakePwDump, TARGET_NODES);
    expect(Object.keys(payload).sort()).toEqual(TARGET_NODES.slice().sort());
  });

  it("averages multi-channel volumes to a single scalar", () => {
    const payload = buildLevelsPayload(fakePwDump, TARGET_NODES);
    expect(payload["Arctis_Game"]).toBeCloseTo(0.75);
    expect(payload["Arctis_Media"]).toBeCloseTo(1.0);
  });

  it("passes through single-channel volumes unchanged", () => {
    const payload = buildLevelsPayload(fakePwDump, TARGET_NODES);
    expect(payload["Arctis_Chat"]).toBeCloseTo(0.5);
    expect(payload["arctis_clean_mic"]).toBeCloseTo(0.9);
  });

  it("omits non-target nodes from the payload", () => {
    const payload = buildLevelsPayload(fakePwDump, TARGET_NODES);
    expect("alsa_output.pci-0000" in payload).toBe(false);
  });

  it("returns empty object when no target nodes are found", () => {
    const payload = buildLevelsPayload([], TARGET_NODES);
    expect(payload).toEqual({});
  });

  it("returns empty object for node without Props params", () => {
    const noProps = [
      {
        type: "PipeWire:Interface:Node",
        id: 101,
        info: { props: { "node.name": "Arctis_Game" }, params: {} },
      },
    ];
    const payload = buildLevelsPayload(noProps, TARGET_NODES);
    expect(payload).toEqual({});
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
