/**
 * deviceControls.ts — Pure view-model helpers for device control widgets.
 *
 * All functions are pure (no Tauri / Svelte deps) so they are unit-testable
 * without a runtime.  The control name strings here match the daemon's
 * descriptor keys exactly (used as the `control` arg in deviceSet calls).
 */

// ---------------------------------------------------------------------------
// ANC mode
// ---------------------------------------------------------------------------

/** Canonical ANC mode names (match daemon descriptor value labels). */
export type AncMode = "off" | "transparency" | "on";

/** Map a raw device_fields["anc_mode"] string to an AncMode. */
export function parseAncMode(raw: string): AncMode {
  switch (raw.toLowerCase().trim()) {
    case "0":
    case "off":
      return "off";
    case "1":
    case "transparency":
      return "transparency";
    case "2":
    case "on":
    case "anc":
      return "on";
    default:
      return "off";
  }
}

/** Map an AncMode to the integer value the daemon expects (device_set("anc", n)). */
export function ancModeToValue(mode: AncMode): number {
  switch (mode) {
    case "off":          return 0;
    case "transparency": return 1;
    case "on":           return 2;
  }
}

/** Human labels for each ANC mode (displayed in the segmented control). */
export const ANC_MODE_LABELS: Record<AncMode, string> = {
  off:          "Off",
  transparency: "Transparency",
  on:           "ANC",
};

export const ANC_MODES: AncMode[] = ["off", "transparency", "on"];

// ---------------------------------------------------------------------------
// Sidetone (0–3 int)
// ---------------------------------------------------------------------------

/** Valid sidetone levels (0 = off, 3 = max). */
export type SidetoneLevelValue = 0 | 1 | 2 | 3;

export interface SidetoneOption {
  value: SidetoneLevelValue;
  label: string;
}

export const SIDETONE_OPTIONS: SidetoneOption[] = [
  { value: 0, label: "Off" },
  { value: 1, label: "Low" },
  { value: 2, label: "Mid" },
  { value: 3, label: "High" },
];

/** Parse a raw device_fields["sidetone"] string into a sidetone level (clamped). */
export function parseSidetoneLevel(raw: string): SidetoneLevelValue {
  const n = parseInt(raw, 10);
  if (isNaN(n)) return 0;
  if (n <= 0) return 0;
  if (n >= 3) return 3;
  return n as SidetoneLevelValue;
}

/** Label for a sidetone integer value. */
export function sidetoneLevelLabel(value: number): string {
  return SIDETONE_OPTIONS.find((o) => o.value === value)?.label ?? String(value);
}

// ---------------------------------------------------------------------------
// Auto-off / inactive_time (0–6 int → minutes label)
// ---------------------------------------------------------------------------

/**
 * Auto-off level → minutes mapping.
 * Level 0 = Never, 1–6 map to the minute buckets used by HeadsetControl.
 * The exact minute values must be confirmed on HW (Task 7c); these match
 * the documented HeadsetControl buckets.
 */
export interface AutoOffOption {
  value: number;   // 0–6, passed to device_set("inactive_time", value)
  label: string;   // displayed in the UI
  minutes: number | null; // null = never
}

export const AUTO_OFF_OPTIONS: AutoOffOption[] = [
  { value: 0, label: "Never",    minutes: null },
  { value: 1, label: "1 min",    minutes: 1   },
  { value: 2, label: "5 min",    minutes: 5   },
  { value: 3, label: "10 min",   minutes: 10  },
  { value: 4, label: "15 min",   minutes: 15  },
  { value: 5, label: "30 min",   minutes: 30  },
  { value: 6, label: "60 min",   minutes: 60  },
];

/** Parse a raw device_fields["inactive_time"] string to an auto-off level. */
export function parseAutoOffLevel(raw: string): number {
  const n = parseInt(raw, 10);
  if (isNaN(n)) return 0;
  return Math.min(Math.max(0, n), 6);
}

/** Label for an auto-off level integer. */
export function autoOffLabel(value: number): string {
  return AUTO_OFF_OPTIONS.find((o) => o.value === value)?.label ?? String(value);
}

// ---------------------------------------------------------------------------
// Generic 1–10 slider helpers (mic_led, transparency_level, mic_volume)
// ---------------------------------------------------------------------------

/** Clamp and parse a 1–10 numeric field from device_fields. */
export function parse1to10(raw: string): number {
  const n = parseInt(raw, 10);
  if (isNaN(n)) return 1;
  return Math.min(Math.max(1, n), 10);
}

// ---------------------------------------------------------------------------
// Capability-detection: derive enabled controls from device_fields keys
// ---------------------------------------------------------------------------

/**
 * The set of device-control names that the UI always shows
 * (hardware write capability assumed unless explicitly absent from fields).
 * These are always-present write controls on the Nova Pro Wireless.
 */
export const ALWAYS_PRESENT_CONTROLS = new Set([
  "sidetone",
  "anc",
  "inactive_time",
  "mic_led",
]);

/**
 * These controls only render if their corresponding read-field appears in
 * device_fields (their feature flag), because they may not be validated yet.
 */
export const CONDITIONAL_CONTROLS: Record<string, string> = {
  transparency_level: "anc_mode", // show if ANC field exists
  mic_volume:         "mic_gain",  // show if mic_gain field exists
};

/**
 * Derive the set of control names to show from the available device_fields.
 * "Always-present" controls are always included when device is present.
 * "Conditional" controls are included only when their read-field key exists.
 */
export function enabledControls(deviceFields: Record<string, string>): Set<string> {
  const enabled = new Set<string>(ALWAYS_PRESENT_CONTROLS);
  for (const [ctrl, fieldKey] of Object.entries(CONDITIONAL_CONTROLS)) {
    if (fieldKey in deviceFields) {
      enabled.add(ctrl);
    }
  }
  return enabled;
}

// ---------------------------------------------------------------------------
// Gate-error classification
// ---------------------------------------------------------------------------

/**
 * Returns true when the error message looks like the daemon's write-gate
 * rejection (device_set returns Unsupported when control is not in
 * enabled_writes allowlist).
 */
export function isGateError(errMsg: string): boolean {
  const lower = errMsg.toLowerCase();
  return (
    lower.includes("unsupported") ||
    lower.includes("not enabled") ||
    lower.includes("gated") ||
    lower.includes("allowlist") ||
    lower.includes("pending") ||
    lower.includes("validation")
  );
}
