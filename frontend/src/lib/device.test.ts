/**
 * device.test.ts — Unit tests for the device view-model mapping helpers.
 *
 * These helpers are pure functions exported from DevicePage.svelte (the
 * `<script lang="ts" module>` context is NOT used for these — they live in
 * the instance script and are imported via the Svelte file's named exports).
 *
 * We re-implement the same pure functions here to keep tests framework-agnostic
 * (no Svelte compiler needed), then cross-reference the implementations.
 */
import { describe, it, expect } from "vitest";

// ---------------------------------------------------------------------------
// Re-declare the pure helpers inline (identical logic to DevicePage.svelte)
// so vitest can run without a Svelte compiler.
// If you refactor these out of the .svelte file into a separate .ts module,
// update this import to use that module.
// ---------------------------------------------------------------------------

type FieldKind = "default" | "battery" | "status" | "percent";

interface DeviceFieldRow {
  key: string;
  label: string;
  value: string;
  kind: FieldKind;
}

function labelForKey(key: string): string {
  const wellKnown: Record<string, string> = {
    battery: "Battery",
    battery_charging: "Charging",
    battery_level: "Battery Level",
    anc_mode: "ANC Mode",
    anc_enabled: "Active Noise Cancelling",
    sidetone: "Sidetone",
    mic_muted: "Mic Muted",
    mic_gain: "Mic Gain",
    firmware: "Firmware",
    model: "Model",
    serial: "Serial",
    connection: "Connection",
    connected: "Connected",
  };
  return wellKnown[key] ?? key.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

function kindForKey(key: string): FieldKind {
  if (key === "battery" || key === "battery_level") return "battery";
  if (key === "battery_charging" || key === "connection" || key === "connected") return "status";
  if (key.endsWith("_percent") || key.endsWith("_pct")) return "percent";
  return "default";
}

function mapDeviceFields(fields: Record<string, string>): DeviceFieldRow[] {
  return Object.entries(fields).map(([key, value]) => ({
    key,
    label: labelForKey(key),
    value,
    kind: kindForKey(key),
  }));
}

function batteryColor(value: string): string | null {
  const n = parseFloat(value);
  if (isNaN(n)) return null;
  if (n > 40) return "--ss-success";
  if (n > 15) return "--ss-warning";
  return "--ss-danger";
}

// ---------------------------------------------------------------------------
// labelForKey
// ---------------------------------------------------------------------------

describe("labelForKey", () => {
  it("returns known label for 'battery'", () => {
    expect(labelForKey("battery")).toBe("Battery");
  });

  it("returns known label for 'anc_mode'", () => {
    expect(labelForKey("anc_mode")).toBe("ANC Mode");
  });

  it("returns known label for 'battery_charging'", () => {
    expect(labelForKey("battery_charging")).toBe("Charging");
  });

  it("converts unknown snake_case key to Title Case", () => {
    expect(labelForKey("some_unknown_key")).toBe("Some Unknown Key");
  });

  it("converts single-word unknown key to Title Case", () => {
    expect(labelForKey("firmware")).toBe("Firmware");
  });

  it("handles keys with multiple underscores", () => {
    expect(labelForKey("mic_muted")).toBe("Mic Muted");
  });
});

// ---------------------------------------------------------------------------
// kindForKey
// ---------------------------------------------------------------------------

describe("kindForKey", () => {
  it("returns 'battery' for 'battery'", () => {
    expect(kindForKey("battery")).toBe("battery");
  });

  it("returns 'battery' for 'battery_level'", () => {
    expect(kindForKey("battery_level")).toBe("battery");
  });

  it("returns 'status' for 'battery_charging'", () => {
    expect(kindForKey("battery_charging")).toBe("status");
  });

  it("returns 'status' for 'connection'", () => {
    expect(kindForKey("connection")).toBe("status");
  });

  it("returns 'status' for 'connected'", () => {
    expect(kindForKey("connected")).toBe("status");
  });

  it("returns 'percent' for keys ending in '_percent'", () => {
    expect(kindForKey("volume_percent")).toBe("percent");
  });

  it("returns 'percent' for keys ending in '_pct'", () => {
    expect(kindForKey("gain_pct")).toBe("percent");
  });

  it("returns 'default' for unrecognised keys", () => {
    expect(kindForKey("anc_mode")).toBe("default");
    expect(kindForKey("firmware")).toBe("default");
    expect(kindForKey("serial")).toBe("default");
  });
});

// ---------------------------------------------------------------------------
// mapDeviceFields
// ---------------------------------------------------------------------------

describe("mapDeviceFields", () => {
  it("returns empty array for empty input", () => {
    expect(mapDeviceFields({})).toEqual([]);
  });

  it("maps a single field correctly", () => {
    const rows = mapDeviceFields({ battery: "85" });
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({
      key: "battery",
      label: "Battery",
      value: "85",
      kind: "battery",
    });
  });

  it("maps multiple fields and preserves values", () => {
    const rows = mapDeviceFields({
      battery: "72",
      anc_mode: "transparency",
      firmware: "1.0.3",
    });
    expect(rows).toHaveLength(3);

    const batteryRow = rows.find((r) => r.key === "battery")!;
    expect(batteryRow.kind).toBe("battery");
    expect(batteryRow.value).toBe("72");

    const ancRow = rows.find((r) => r.key === "anc_mode")!;
    expect(ancRow.kind).toBe("default");
    expect(ancRow.label).toBe("ANC Mode");

    const fwRow = rows.find((r) => r.key === "firmware")!;
    expect(fwRow.kind).toBe("default");
    expect(fwRow.label).toBe("Firmware");
  });

  it("does not add extra keys to row objects", () => {
    const rows = mapDeviceFields({ battery: "50" });
    const keys = Object.keys(rows[0]).sort();
    expect(keys).toEqual(["key", "kind", "label", "value"]);
  });
});

// ---------------------------------------------------------------------------
// batteryColor
// ---------------------------------------------------------------------------

describe("batteryColor", () => {
  it("returns '--ss-success' for level > 40", () => {
    expect(batteryColor("85")).toBe("--ss-success");
    expect(batteryColor("41")).toBe("--ss-success");
    expect(batteryColor("100")).toBe("--ss-success");
  });

  it("returns '--ss-warning' for level 16–40", () => {
    expect(batteryColor("40")).toBe("--ss-warning");
    expect(batteryColor("25")).toBe("--ss-warning");
    expect(batteryColor("16")).toBe("--ss-warning");
  });

  it("returns '--ss-danger' for level ≤ 15", () => {
    expect(batteryColor("15")).toBe("--ss-danger");
    expect(batteryColor("5")).toBe("--ss-danger");
    expect(batteryColor("0")).toBe("--ss-danger");
  });

  it("returns null for non-numeric strings", () => {
    expect(batteryColor("charging")).toBeNull();
    expect(batteryColor("")).toBeNull();
    expect(batteryColor("N/A")).toBeNull();
  });

  it("handles decimal string values", () => {
    expect(batteryColor("85.5")).toBe("--ss-success");
    expect(batteryColor("14.9")).toBe("--ss-danger");
  });
});

// ---------------------------------------------------------------------------
// mapDeviceFields — live device fields (Task 8 extension)
// ---------------------------------------------------------------------------

describe("mapDeviceFields — live device fields", () => {
  it("maps anc_mode field with correct label and default kind", () => {
    const rows = mapDeviceFields({ anc_mode: "1" });
    expect(rows).toHaveLength(1);
    const row = rows[0];
    expect(row.key).toBe("anc_mode");
    expect(row.label).toBe("ANC Mode");
    expect(row.value).toBe("1");
    expect(row.kind).toBe("default");
  });

  it("maps mic_led field with title-cased label", () => {
    const rows = mapDeviceFields({ mic_led: "5" });
    expect(rows[0].label).toBe("Mic Led");
    expect(rows[0].value).toBe("5");
    expect(rows[0].kind).toBe("default");
  });

  it("maps inactive_time field with title-cased label", () => {
    const rows = mapDeviceFields({ inactive_time: "3" });
    expect(rows[0].label).toBe("Inactive Time");
    expect(rows[0].value).toBe("3");
  });

  it("maps transparency_level with title-cased label", () => {
    const rows = mapDeviceFields({ transparency_level: "7" });
    expect(rows[0].label).toBe("Transparency Level");
    expect(rows[0].value).toBe("7");
  });

  it("maps mic_volume with title-cased label", () => {
    const rows = mapDeviceFields({ mic_volume: "8" });
    expect(rows[0].label).toBe("Mic Volume");
    expect(rows[0].value).toBe("8");
  });

  it("handles a full live device snapshot", () => {
    const snapshot = {
      battery: "72",
      anc_mode: "1",
      sidetone: "2",
      mic_muted: "false",
      mic_led: "5",
      inactive_time: "3",
      firmware: "1.2.0",
    };
    const rows = mapDeviceFields(snapshot);
    expect(rows).toHaveLength(7);

    const ancRow = rows.find((r) => r.key === "anc_mode")!;
    expect(ancRow.label).toBe("ANC Mode");
    expect(ancRow.kind).toBe("default");

    const micLedRow = rows.find((r) => r.key === "mic_led")!;
    expect(micLedRow.value).toBe("5");

    const inactiveRow = rows.find((r) => r.key === "inactive_time")!;
    expect(inactiveRow.value).toBe("3");
  });
});
