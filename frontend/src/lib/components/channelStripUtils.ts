/**
 * channelStripUtils.ts — Pure helper functions for ChannelStrip.svelte.
 *
 * Extracted so they can be unit-tested without mounting the Svelte component.
 */
import type { ChannelSnapshot, OutputDeviceSnapshot } from "../ipc.js";

export interface DeviceOption {
  value: string | null;
  label: string;
}

/**
 * Build the list of `<option>` entries for the channel output selector.
 *
 * Always starts with { value: null, label: "Default (follow system)" }.
 * Then one entry per known output device; the system-default device is
 * labelled "… (system default)".
 * If the channel already has an `output_device` that is absent from `devices`
 * (e.g. it was unplugged), it is appended as "… (unavailable)" so the
 * selector still accurately reflects the current state.
 */
export function buildDeviceOptions(
  ch: ChannelSnapshot,
  devices: OutputDeviceSnapshot[],
): DeviceOption[] {
  const opts: DeviceOption[] = [
    { value: null, label: "Default (follow system)" },
    ...devices.map((d) => ({
      value: d.node_name,
      label: d.is_default ? d.description + " (system default)" : d.description,
    })),
  ];

  // Append the currently-set device if it is not in the fetched list.
  if (ch.output_device && !devices.some((d) => d.node_name === ch.output_device)) {
    opts.push({ value: ch.output_device, label: ch.output_device + " (unavailable)" });
  }

  return opts;
}
