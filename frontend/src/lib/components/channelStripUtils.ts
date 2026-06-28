/**
 * channelStripUtils.ts — Pure helper functions for ChannelStrip.svelte.
 *
 * Extracted so they can be unit-tested without mounting the Svelte component.
 */
import type { ChannelSnapshot, OutputDeviceSnapshot } from "../ipc.js";

// ---------------------------------------------------------------------------
// Output-device Select adapter
//
// The device-options model uses `value: string | null` (null = "Default").
// The C0a Select component requires `value: string`. These helpers bridge the
// two via a sentinel constant.
// ---------------------------------------------------------------------------

/** Sentinel value representing the "Default (follow system)" output device. */
export const DEFAULT_OUTPUT_VALUE = "__default__";

/** Map DeviceOption[] (value: string|null) → SelectOption[] (value: string). */
export function toSelectOptions(
  opts: DeviceOption[],
): { value: string; label: string }[] {
  return opts.map((o) => ({
    value: o.value === null ? DEFAULT_OUTPUT_VALUE : o.value,
    label: o.label,
  }));
}

/**
 * The Select's current string value for a channel's output_device.
 * Maps null (= "follow system") → the sentinel.
 */
export function outputToSelectValue(output_device: string | null): string {
  return output_device === null ? DEFAULT_OUTPUT_VALUE : output_device;
}

/**
 * Inverse: a Select string value back to the IPC value.
 * Maps the sentinel → null; all other values pass through unchanged.
 */
export function selectValueToOutput(value: string): string | null {
  return value === DEFAULT_OUTPUT_VALUE ? null : value;
}

/**
 * Extract a human-readable message from an unknown catch value.
 * Used in the channel strip's write-failure catch blocks so the same
 * message is passed to `console.error` and to the `onError` callback.
 */
export function toErrorMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

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
