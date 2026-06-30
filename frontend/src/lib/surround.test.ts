/**
 * surround.test.ts — Pure-helper unit tests for surround.ts.
 *
 * Mirrors mic.test.ts pattern: no DOM, no Svelte, no IPC. Logic is in pure
 * helpers tested with vitest.
 */

import { describe, it, expect } from "vitest";
import { hrirDisplayName, channelChecked, toggleChannel, buildSinkOptions, sinkSelectValue, sinkValueToHwSink } from "./surround.js";
import { groupHrirOptionsByTonality, factoryProfileLabel, formatBlocksize } from "./surround.js";
import type { OutputDeviceSnapshot, HrirEntrySnapshot, FactoryProfileInfo } from "./ipc.js";

// ---------------------------------------------------------------------------
// hrirDisplayName
// ---------------------------------------------------------------------------

describe("hrirDisplayName", () => {
  it("strips a two-digit leading prefix and converts dashes to spaces", () => {
    expect(hrirDisplayName("02-dh-dolby-headphone")).toBe("Dh Dolby Headphone");
  });

  it("strips the 00-default-asm prefix correctly", () => {
    expect(hrirDisplayName("00-default-asm")).toBe("Default Asm");
  });

  it("handles a stem with no numeric prefix", () => {
    expect(hrirDisplayName("flat")).toBe("Flat");
  });

  it("handles dashes without numeric prefix", () => {
    expect(hrirDisplayName("custom-hrir-v2")).toBe("Custom Hrir V2");
  });

  it("title-cases each word (first char upper, rest lower)", () => {
    expect(hrirDisplayName("01-ALLCAPS-word")).toBe("Allcaps Word");
  });

  it("handles a single-word stem after stripping prefix", () => {
    expect(hrirDisplayName("05-dolby")).toBe("Dolby");
  });

  it("handles multi-digit numeric prefix (e.g. 12-)", () => {
    expect(hrirDisplayName("12-room-reverb")).toBe("Room Reverb");
  });

  it("handles stem that is only a number-prefixed word", () => {
    expect(hrirDisplayName("1-flat")).toBe("Flat");
  });

  it("returns empty string for empty input", () => {
    expect(hrirDisplayName("")).toBe("");
  });
});

// ---------------------------------------------------------------------------
// channelChecked
// ---------------------------------------------------------------------------

describe("channelChecked", () => {
  it("returns true when channel is in the list", () => {
    expect(channelChecked("game", ["game", "media"])).toBe(true);
    expect(channelChecked("media", ["game", "media"])).toBe(true);
  });

  it("returns false when channel is not in the list", () => {
    expect(channelChecked("chat", ["game", "media"])).toBe(false);
  });

  it("returns false for an empty list", () => {
    expect(channelChecked("game", [])).toBe(false);
  });

  it("returns false when only a different channel is present", () => {
    expect(channelChecked("game", ["chat"])).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// toggleChannel
// ---------------------------------------------------------------------------

describe("toggleChannel", () => {
  it("removes a channel that is already in the list", () => {
    expect(toggleChannel("game", ["game", "media"])).toEqual(["media"]);
  });

  it("adds a channel that is not in the list", () => {
    expect(toggleChannel("chat", ["game", "media"])).toEqual(["game", "media", "chat"]);
  });

  it("preserves order of remaining channels when removing", () => {
    expect(toggleChannel("media", ["game", "chat", "media"])).toEqual(["game", "chat"]);
  });

  it("handles toggling on an empty list (adds the channel)", () => {
    expect(toggleChannel("game", [])).toEqual(["game"]);
  });

  it("handles removing the last channel (returns empty list)", () => {
    expect(toggleChannel("game", ["game"])).toEqual([]);
  });

  it("does not mutate the original array", () => {
    const original = ["game", "media"];
    toggleChannel("game", original);
    expect(original).toEqual(["game", "media"]);
  });

  it("appends newly added channel at the end", () => {
    const result = toggleChannel("chat", ["game"]);
    expect(result[result.length - 1]).toBe("chat");
  });
});

// ---------------------------------------------------------------------------
// buildSinkOptions
// ---------------------------------------------------------------------------

describe("buildSinkOptions", () => {
  const devices: OutputDeviceSnapshot[] = [
    { node_name: "alsa_output.arctis", description: "Arctis Nova Pro", is_default: true },
    { node_name: "alsa_output.hdmi", description: "HDMI", is_default: false },
  ];

  it("first entry is Auto-detect with value ''", () => {
    const opts = buildSinkOptions(devices, null);
    expect(opts[0].value).toBe("");
    expect(opts[0].label).toBe("Auto-detect");
  });

  it("includes resolved name in Auto-detect label when provided", () => {
    const opts = buildSinkOptions(devices, "alsa_output.arctis");
    expect(opts[0].label).toBe("Auto-detect (alsa_output.arctis)");
  });

  it("maps node_name to value for each device", () => {
    const opts = buildSinkOptions(devices, null);
    expect(opts[1].value).toBe("alsa_output.arctis");
    expect(opts[2].value).toBe("alsa_output.hdmi");
  });

  it("maps description to label for each device", () => {
    const opts = buildSinkOptions(devices, null);
    expect(opts[1].label).toBe("Arctis Nova Pro");
    expect(opts[2].label).toBe("HDMI");
  });

  it("falls back to node_name as label when description is empty", () => {
    const noDesc: OutputDeviceSnapshot[] = [
      { node_name: "alsa_output.unknown", description: "", is_default: false },
    ];
    const opts = buildSinkOptions(noDesc, null);
    expect(opts[1].label).toBe("alsa_output.unknown");
  });

  it("returns only the Auto-detect entry when outputs list is empty", () => {
    const opts = buildSinkOptions([], null);
    expect(opts).toHaveLength(1);
    expect(opts[0].value).toBe("");
  });

  it("total length is outputs.length + 1 (Auto-detect + all devices)", () => {
    const opts = buildSinkOptions(devices, null);
    expect(opts).toHaveLength(devices.length + 1);
  });
});

// ---------------------------------------------------------------------------
// sinkSelectValue
// ---------------------------------------------------------------------------

describe("sinkSelectValue", () => {
  it("returns '' for null", () => {
    expect(sinkSelectValue(null)).toBe("");
  });

  it("returns '' for undefined", () => {
    expect(sinkSelectValue(undefined)).toBe("");
  });

  it("returns the node name for a non-null value", () => {
    expect(sinkSelectValue("alsa_output.arctis")).toBe("alsa_output.arctis");
  });
});

// ---------------------------------------------------------------------------
// sinkValueToHwSink
// ---------------------------------------------------------------------------

describe("sinkValueToHwSink", () => {
  it("returns null for empty string (Auto-detect)", () => {
    expect(sinkValueToHwSink("")).toBeNull();
  });

  it("returns the node name for a non-empty string", () => {
    expect(sinkValueToHwSink("alsa_output.arctis")).toBe("alsa_output.arctis");
  });
});

// ---------------------------------------------------------------------------
// Round-trip: sinkValueToHwSink(sinkSelectValue(x))
// ---------------------------------------------------------------------------

describe("sinkSelectValue / sinkValueToHwSink round-trip", () => {
  it("null → '' → null", () => {
    expect(sinkValueToHwSink(sinkSelectValue(null))).toBeNull();
  });

  it("'' (empty string treated as null source) → '' → null", () => {
    // An empty string from hw_sink is treated as auto-detect
    expect(sinkValueToHwSink(sinkSelectValue(""))).toBeNull();
  });

  it("node_name → node_name → node_name", () => {
    const name = "alsa_output.arctis";
    expect(sinkValueToHwSink(sinkSelectValue(name))).toBe(name);
  });
});

// ---------------------------------------------------------------------------
// groupHrirOptionsByTonality / output EQ mapping / factoryProfileLabel (Task 12)
// ---------------------------------------------------------------------------

describe("groupHrirOptionsByTonality", () => {
  const entries: HrirEntrySnapshot[] = [
    { stem: "a", display: "A", group: "G", tonality: "Roomy" },
    { stem: "b", display: "B", group: "G", tonality: "Dry" },
    { stem: "c", display: "C", group: "G", tonality: "Neutral" },
  ];
  it("orders Dry, then Neutral, then Roomy", () => {
    const opts = groupHrirOptionsByTonality(entries);
    const stems = opts.map((o) => o.value);
    expect(stems).toEqual(["b", "c", "a"]);
  });
  it("falls back to display when group is empty", () => {
    const opts = groupHrirOptionsByTonality([{ stem: "x", display: "X", group: "", tonality: "Dry" }]);
    expect(opts[0].label).toBe("X");
  });
});

describe("factoryProfileLabel", () => {
  it("shows name and hrir", () => {
    const info: FactoryProfileInfo = { name: "DayZ", hrir: "04-gsx-sennheiser-gsx", mode: "hrir71" };
    expect(factoryProfileLabel(info)).toContain("DayZ");
  });
  it("shows just the name when hrir is null", () => {
    const info: FactoryProfileInfo = { name: "Plain", hrir: null, mode: "stereo_bypass" };
    expect(factoryProfileLabel(info)).toBe("Plain");
  });
});

describe("formatBlocksize", () => {
  it("shows 'auto' for null/undefined (PipeWire default)", () => {
    expect(formatBlocksize(null)).toBe("auto");
    expect(formatBlocksize(undefined)).toBe("auto");
  });
  it("shows the sample count for a pinned blocksize", () => {
    expect(formatBlocksize(128)).toBe("128");
    expect(formatBlocksize(0)).toBe("0");
  });
});
