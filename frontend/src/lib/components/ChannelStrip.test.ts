import { describe, it, expect } from "vitest";
import { buildDeviceOptions, toErrorMsg } from "./channelStripUtils.js";
import type { ChannelSnapshot, OutputDeviceSnapshot } from "../ipc.js";

// Minimal ChannelSnapshot fixture
const mkChannel = (output_device: string | null): ChannelSnapshot => ({
  id: "game",
  node_name: "Arctis_Game",
  output_device,
  eq_bands: [],
  volume_pct: 100,
  muted: false,
});

// Minimal OutputDeviceSnapshot fixture
const mkDevice = (node_name: string, description: string, is_default: boolean): OutputDeviceSnapshot => ({
  node_name,
  description,
  is_default,
});

// ---------------------------------------------------------------------------
// toErrorMsg — the helper used in every ChannelStrip write-failure catch block
// to produce the message forwarded to the `onError` prop.
// ---------------------------------------------------------------------------
describe("toErrorMsg", () => {
  it("returns err.message for an Error instance", () => {
    expect(toErrorMsg(new Error("setChannelVolume rejected"))).toBe("setChannelVolume rejected");
  });

  it("returns String(err) for a plain string throw", () => {
    expect(toErrorMsg("daemon hiccup")).toBe("daemon hiccup");
  });

  it("returns String(err) for a numeric throw", () => {
    expect(toErrorMsg(42)).toBe("42");
  });

  it("returns String(err) for null / undefined", () => {
    expect(toErrorMsg(null)).toBe("null");
    expect(toErrorMsg(undefined)).toBe("undefined");
  });

  // Confirm onError integration: simulate the pattern used in every catch block.
  // This is the closest we can get to a component-level assertion without a DOM
  // harness (no jsdom / happy-dom in this project).
  it("when used with an onError callback, forwards the error message correctly", () => {
    const received: string[] = [];
    const onError = (msg: string) => received.push(msg);
    const err = new Error("write failed");
    onError(toErrorMsg(err));
    expect(received).toEqual(["write failed"]);
  });

  it("handles mute / output failures the same way", () => {
    const muteErr = new Error("setChannelMute rejected");
    expect(toErrorMsg(muteErr)).toBe("setChannelMute rejected");

    const outputErr = new Error("setChannelOutput rejected");
    expect(toErrorMsg(outputErr)).toBe("setChannelOutput rejected");
  });
});

describe("buildDeviceOptions", () => {
  it("returns Default first, then one option per device; system-default device is labelled '… (system default)'", () => {
    const devices: OutputDeviceSnapshot[] = [
      mkDevice("alsa_output.pci.analog", "Built-in Audio", false),
      mkDevice("arctis_output", "Arctis Nova Pro", true),
    ];
    const opts = buildDeviceOptions(mkChannel(null), devices);

    expect(opts[0]).toEqual({ value: null, label: "Default (follow system)" });
    expect(opts[1]).toEqual({ value: "alsa_output.pci.analog", label: "Built-in Audio" });
    expect(opts[2]).toEqual({ value: "arctis_output", label: "Arctis Nova Pro (system default)" });
    expect(opts).toHaveLength(3);
  });

  it("appends a channel.output_device not in the device list as '… (unavailable)'", () => {
    const devices: OutputDeviceSnapshot[] = [
      mkDevice("alsa_output.pci.analog", "Built-in Audio", false),
    ];
    const ch = mkChannel("old_device_gone");
    const opts = buildDeviceOptions(ch, devices);

    // Default + 1 real device + 1 unavailable
    expect(opts).toHaveLength(3);
    expect(opts[2]).toEqual({ value: "old_device_gone", label: "old_device_gone (unavailable)" });
  });

  it("empty device list yields just the Default option (plus any unavailable current selection)", () => {
    // No current selection, no devices → just Default
    const opts1 = buildDeviceOptions(mkChannel(null), []);
    expect(opts1).toHaveLength(1);
    expect(opts1[0]).toEqual({ value: null, label: "Default (follow system)" });

    // Current selection set but no devices → Default + unavailable
    const opts2 = buildDeviceOptions(mkChannel("missing_device"), []);
    expect(opts2).toHaveLength(2);
    expect(opts2[1]).toEqual({ value: "missing_device", label: "missing_device (unavailable)" });
  });
});
