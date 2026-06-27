import { describe, it, expect } from "vitest";
import { buildDeviceOptions } from "./channelStripUtils.js";
import type { ChannelSnapshot, OutputDeviceSnapshot } from "../ipc.js";

// Minimal ChannelSnapshot fixture
const mkChannel = (output_device: string | null): ChannelSnapshot => ({
  id: "game",
  node_name: "Arctis_Game",
  output_device,
  eq_bands: [],
  volume_db: 0,
  muted: false,
});

// Minimal OutputDeviceSnapshot fixture
const mkDevice = (node_name: string, description: string, is_default: boolean): OutputDeviceSnapshot => ({
  node_name,
  description,
  is_default,
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
