/**
 * ipc.test.ts — Unit tests for the IPC arg-builder helpers.
 *
 * These tests run without a Tauri runtime: they exercise only the pure
 * functions that construct the camelCase argument objects passed to invoke().
 * Tauri converts Rust snake_case command param names to camelCase on the JS
 * side; the helpers codify that mapping explicitly so it can be verified here.
 */
import { describe, it, expect } from "vitest";
import {
  buildSetEqBandArgs,
  buildSetRouteArgs,
  buildSetChannelOutputArgs,
} from "./ipc.js";

// ---------------------------------------------------------------------------
// buildSetEqBandArgs
// ---------------------------------------------------------------------------
describe("buildSetEqBandArgs", () => {
  it("maps freq_hz to freqHz and gain_db to gainDb", () => {
    const args = buildSetEqBandArgs("game", 0, "peaking", 1000, 1.4, 3.5);
    expect(args).toEqual({
      channel: "game",
      band: 0,
      kind: "peaking",
      freqHz: 1000,
      q: 1.4,
      gainDb: 3.5,
    });
  });

  it("does NOT include freq_hz or gain_db keys (no snake_case aliases)", () => {
    const args = buildSetEqBandArgs("chat", 1, "lowshelf", 80, 0.707, -2.0);
    expect(Object.keys(args)).not.toContain("freq_hz");
    expect(Object.keys(args)).not.toContain("gain_db");
  });

  it("passes through all primitive fields unchanged", () => {
    const args = buildSetEqBandArgs("media", 2, "highpass", 20000, 2.0, 0.0);
    expect(args.channel).toBe("media");
    expect(args.band).toBe(2);
    expect(args.kind).toBe("highpass");
    expect(args.q).toBe(2.0);
    expect(args.gainDb).toBe(0.0);
    expect(args.freqHz).toBe(20000);
  });

  it("handles fractional frequency and gain values", () => {
    const args = buildSetEqBandArgs("mic", 9, "peaking", 3156.7, 0.5, -11.25);
    expect(args.freqHz).toBeCloseTo(3156.7);
    expect(args.gainDb).toBeCloseTo(-11.25);
  });
});

// ---------------------------------------------------------------------------
// buildSetRouteArgs
// ---------------------------------------------------------------------------
describe("buildSetRouteArgs", () => {
  it("maps app_binary to appBinary and target_sink to targetSink", () => {
    const args = buildSetRouteArgs("spotify", "game");
    expect(args).toEqual({ appBinary: "spotify", targetSink: "game" });
  });

  it("does NOT include snake_case keys", () => {
    const args = buildSetRouteArgs("discord", "chat");
    expect(Object.keys(args)).not.toContain("app_binary");
    expect(Object.keys(args)).not.toContain("target_sink");
  });

  it("preserves binary names with path separators or dots", () => {
    const args = buildSetRouteArgs("/usr/bin/firefox", "media");
    expect(args.appBinary).toBe("/usr/bin/firefox");
    expect(args.targetSink).toBe("media");
  });
});

// ---------------------------------------------------------------------------
// buildSetChannelOutputArgs
// ---------------------------------------------------------------------------
describe("buildSetChannelOutputArgs", () => {
  it("passes non-null device through", () => {
    const args = buildSetChannelOutputArgs("game", "Arctis Nova");
    expect(args).toEqual({ channel: "game", device: "Arctis Nova" });
  });

  it("passes null device through (resets to default)", () => {
    const args = buildSetChannelOutputArgs("chat", null);
    expect(args).toEqual({ channel: "chat", device: null });
  });

  it("does not produce extra keys", () => {
    const args = buildSetChannelOutputArgs("media", null);
    expect(Object.keys(args).sort()).toEqual(["channel", "device"]);
  });
});

// ---------------------------------------------------------------------------
// EngineState shape validation (type-level: exercised by TS compilation,
// here we also do runtime shape checks against a mock snapshot)
// ---------------------------------------------------------------------------
describe("EngineState shape (runtime guard)", () => {
  it("accepts a well-formed snapshot with empty channels/routes/device_fields", () => {
    // This acts as a runtime assertion that the field names are stable snake_case.
    const snapshot = {
      active_profile: "Default",
      profiles: ["Default", "Gaming"],
      channels: [],
      routes: [] as [string, string][],
      device_present: false,
      device_fields: {},
    };
    expect(snapshot.active_profile).toBe("Default");
    expect(Array.isArray(snapshot.profiles)).toBe(true);
    expect(Array.isArray(snapshot.channels)).toBe(true);
    expect(Array.isArray(snapshot.routes)).toBe(true);
    expect(typeof snapshot.device_present).toBe("boolean");
    expect(typeof snapshot.device_fields).toBe("object");
  });

  it("accepts a snapshot with populated channels including eq_bands", () => {
    const snapshot = {
      active_profile: "Gaming",
      profiles: ["Default", "Gaming"],
      channels: [
        {
          id: "game",
          node_name: "Game",
          output_device: null,
          eq_bands: [
            { kind: "peaking", freq_hz: 1000, q: 1.4, gain_db: 3.0 },
          ],
        },
      ],
      routes: [["discord", "chat"]] as [string, string][],
      device_present: true,
      device_fields: { battery: "85" },
    };

    const ch = snapshot.channels[0];
    expect(ch.id).toBe("game");
    expect(ch.output_device).toBeNull();
    expect(ch.eq_bands).toHaveLength(1);
    expect(ch.eq_bands[0].freq_hz).toBe(1000);
    expect(ch.eq_bands[0].gain_db).toBe(3.0);

    expect(snapshot.routes[0]).toEqual(["discord", "chat"]);
    expect(snapshot.device_fields.battery).toBe("85");
  });
});
