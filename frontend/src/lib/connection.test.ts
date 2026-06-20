/**
 * connection.test.ts — Unit tests for daemon-reachability status helpers.
 *
 * The dot in the top-bar must reflect whether the daemon is reachable,
 * NOT whether a headset HID device is present.
 */
import { describe, it, expect } from "vitest";
import { deriveConnectionStatus, connectionLabel } from "./connection.js";

// ---------------------------------------------------------------------------
// deriveConnectionStatus
// ---------------------------------------------------------------------------
describe("deriveConnectionStatus", () => {
  it("returns 'disconnected' when loadError is set", () => {
    expect(deriveConnectionStatus("Daemon unavailable", null)).toBe("disconnected");
    expect(deriveConnectionStatus("timeout", {})).toBe("disconnected");
  });

  it("returns 'connecting' when no error and engineState is null", () => {
    expect(deriveConnectionStatus(null, null)).toBe("connecting");
  });

  it("returns 'connected' when engineState is a non-null object", () => {
    const state = { device_present: false, active_profile: "Default", profiles: [], channels: [], routes: [], device_fields: {} };
    expect(deriveConnectionStatus(null, state)).toBe("connected");
  });

  it("returns 'connected' even when device_present is false", () => {
    // The key regression guard: daemon reachable + no HID device → still 'connected'.
    const stateNoDevice = { device_present: false };
    expect(deriveConnectionStatus(null, stateNoDevice)).toBe("connected");
  });

  it("returns 'connected' when device_present is true (sanity check)", () => {
    const stateWithDevice = { device_present: true };
    expect(deriveConnectionStatus(null, stateWithDevice)).toBe("connected");
  });
});

// ---------------------------------------------------------------------------
// connectionLabel
// ---------------------------------------------------------------------------
describe("connectionLabel", () => {
  it("labels 'connected' as 'Daemon connected'", () => {
    expect(connectionLabel("connected")).toBe("Daemon connected");
  });

  it("labels 'connecting' as 'Connecting…'", () => {
    expect(connectionLabel("connecting")).toBe("Connecting…");
  });

  it("labels 'disconnected' as 'Daemon offline'", () => {
    expect(connectionLabel("disconnected")).toBe("Daemon offline");
  });
});
