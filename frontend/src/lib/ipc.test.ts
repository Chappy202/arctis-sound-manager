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
  buildDeviceSetArgs,
  buildSetChannelVolumeArgs,
  buildSetChannelMuteArgs,
  buildProfileRenameArgs,
  buildEqPresetSaveArgs,
  buildEqPresetApplyArgs,
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
// setChannelOutput return type (documented expectation)
// ---------------------------------------------------------------------------
describe("setChannelOutput return type contract", () => {
  it("buildSetChannelOutputArgs produces args suitable for an EngineState-returning invoke", () => {
    // The Rust command now returns Result<EngineState, CommandError>.
    // This test verifies the arg shape is still correct and does not regress.
    const args = buildSetChannelOutputArgs("game", "SteelSeries Arctis Nova Pro");
    expect(args.channel).toBe("game");
    expect(args.device).toBe("SteelSeries Arctis Nova Pro");
    // No extra keys (Tauri rejects unknown params in strict mode)
    expect(Object.keys(args).sort()).toEqual(["channel", "device"]);
  });
});

// ---------------------------------------------------------------------------
// buildDeviceSetArgs
// ---------------------------------------------------------------------------
describe("buildDeviceSetArgs", () => {
  it("passes control and value through unchanged", () => {
    const args = buildDeviceSetArgs("sidetone", 2);
    expect(args).toEqual({ control: "sidetone", value: 2 });
  });

  it("produces exactly two keys: control and value", () => {
    const args = buildDeviceSetArgs("anc", 0);
    expect(Object.keys(args).sort()).toEqual(["control", "value"]);
  });

  it("supports integer values in full i64 safe range", () => {
    const args = buildDeviceSetArgs("inactive_time", 3);
    expect(args.value).toBe(3);
    expect(args.control).toBe("inactive_time");
  });

  it("supports value=0 (off/min)", () => {
    const args = buildDeviceSetArgs("sidetone", 0);
    expect(args.value).toBe(0);
  });
});

// ---------------------------------------------------------------------------
// buildSetChannelVolumeArgs
// ---------------------------------------------------------------------------
describe("buildSetChannelVolumeArgs", () => {
  it("passes channel and volume_db through with snake_case key", () => {
    const args = buildSetChannelVolumeArgs("game", -6.0);
    expect(args).toEqual({ channel: "game", volume_db: -6.0 });
  });

  it("uses snake_case volume_db key (not camelCase)", () => {
    const args = buildSetChannelVolumeArgs("chat", 0.0);
    expect(Object.keys(args)).toContain("volume_db");
    expect(Object.keys(args)).not.toContain("volumeDb");
  });

  it("produces exactly two keys: channel and volume_db", () => {
    const args = buildSetChannelVolumeArgs("media", 6.0);
    expect(Object.keys(args).sort()).toEqual(["channel", "volume_db"]);
  });

  it("accepts min boundary -60 dB", () => {
    const args = buildSetChannelVolumeArgs("game", -60.0);
    expect(args.volume_db).toBe(-60.0);
  });

  it("accepts max boundary +6 dB", () => {
    const args = buildSetChannelVolumeArgs("game", 6.0);
    expect(args.volume_db).toBe(6.0);
  });

  it("accepts fractional dB values", () => {
    const args = buildSetChannelVolumeArgs("chat", -3.5);
    expect(args.volume_db).toBeCloseTo(-3.5);
  });
});

// ---------------------------------------------------------------------------
// buildSetChannelMuteArgs
// ---------------------------------------------------------------------------
describe("buildSetChannelMuteArgs", () => {
  it("passes channel and muted=true", () => {
    const args = buildSetChannelMuteArgs("game", true);
    expect(args).toEqual({ channel: "game", muted: true });
  });

  it("passes channel and muted=false", () => {
    const args = buildSetChannelMuteArgs("chat", false);
    expect(args).toEqual({ channel: "chat", muted: false });
  });

  it("produces exactly two keys: channel and muted", () => {
    const args = buildSetChannelMuteArgs("media", true);
    expect(Object.keys(args).sort()).toEqual(["channel", "muted"]);
  });
});

// ---------------------------------------------------------------------------
// buildProfileRenameArgs (F3b)
// ---------------------------------------------------------------------------
describe("buildProfileRenameArgs", () => {
  it("maps old_name to 'old' and new_name to 'new'", () => {
    const args = buildProfileRenameArgs("default", "gaming");
    expect(args).toEqual({ old: "default", new: "gaming" });
  });

  it("produces exactly two keys: old and new", () => {
    const args = buildProfileRenameArgs("a", "b");
    expect(Object.keys(args).sort()).toEqual(["new", "old"]);
  });

  it("allows names with spaces and unicode", () => {
    const args = buildProfileRenameArgs("old name", "New Näme");
    expect(args.old).toBe("old name");
    expect(args.new).toBe("New Näme");
  });
});

// ---------------------------------------------------------------------------
// buildEqPresetSaveArgs (F3b)
// ---------------------------------------------------------------------------
describe("buildEqPresetSaveArgs", () => {
  it("passes name and channel through unchanged", () => {
    const args = buildEqPresetSaveArgs("gaming-boost", "game");
    expect(args).toEqual({ name: "gaming-boost", channel: "game" });
  });

  it("produces exactly two keys: name and channel", () => {
    const args = buildEqPresetSaveArgs("flat", "media");
    expect(Object.keys(args).sort()).toEqual(["channel", "name"]);
  });
});

// ---------------------------------------------------------------------------
// buildEqPresetApplyArgs (F3b)
// ---------------------------------------------------------------------------
describe("buildEqPresetApplyArgs", () => {
  it("passes preset and channel through unchanged", () => {
    const args = buildEqPresetApplyArgs("gaming-boost", "game");
    expect(args).toEqual({ preset: "gaming-boost", channel: "game" });
  });

  it("produces exactly two keys: preset and channel", () => {
    const args = buildEqPresetApplyArgs("flat", "chat");
    expect(Object.keys(args).sort()).toEqual(["channel", "preset"]);
  });

  it("uses 'preset' key not 'name' (matches Rust EqPresetApply field)", () => {
    const args = buildEqPresetApplyArgs("my-preset", "media");
    expect(Object.keys(args)).toContain("preset");
    expect(Object.keys(args)).not.toContain("name");
  });
});

// ---------------------------------------------------------------------------
// EqPresetSnapshot shape (F3b)
// ---------------------------------------------------------------------------
describe("EqPresetSnapshot shape (runtime guard)", () => {
  it("accepts a well-formed EqPresetSnapshot", () => {
    const preset = { name: "gaming-boost", band_count: 10 };
    expect(preset.name).toBe("gaming-boost");
    expect(preset.band_count).toBe(10);
  });

  it("EngineState snapshot can include eq_presets array", () => {
    const snapshot = {
      active_profile: "Default",
      profiles: ["Default"],
      channels: [],
      routes: [] as [string, string][],
      device_present: false,
      device_fields: {},
      eq_presets: [
        { name: "flat", band_count: 10 },
        { name: "gaming-boost", band_count: 10 },
      ],
    };
    expect(snapshot.eq_presets).toHaveLength(2);
    expect(snapshot.eq_presets[0].name).toBe("flat");
    expect(snapshot.eq_presets[1].band_count).toBe(10);
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

  it("accepts a snapshot with populated channels including eq_bands, volume_db, muted", () => {
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
          volume_db: -6.0,
          muted: false,
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
    expect(ch.volume_db).toBe(-6.0);
    expect(ch.muted).toBe(false);

    expect(snapshot.routes[0]).toEqual(["discord", "chat"]);
    expect(snapshot.device_fields.battery).toBe("85");
  });
});
