import { describe, it, expect } from "vitest";
import { groupPresets } from "./eqPresetUtils.js";
import type { EqPresetSnapshot } from "../ipc.js";

const mkPreset = (name: string, band_count: number): EqPresetSnapshot => ({ name, band_count });

describe("groupPresets", () => {
  it("returns empty groups when both arrays are empty", () => {
    const result = groupPresets([], []);
    expect(result.builtin).toEqual([]);
    expect(result.saved).toEqual([]);
  });

  it("places factory presets in builtin and saved presets in saved", () => {
    const factory = [mkPreset("Flat", 10), mkPreset("Bass Boost", 10)];
    const saved = [mkPreset("My Custom", 10)];
    const result = groupPresets(factory, saved);
    expect(result.builtin).toEqual(factory);
    expect(result.saved).toEqual(saved);
  });

  it("builtin does not contain saved presets and saved does not contain builtin presets", () => {
    const factory = [mkPreset("Treble", 10)];
    const saved = [mkPreset("Custom", 10)];
    const result = groupPresets(factory, saved);
    expect(result.builtin).not.toContainEqual(saved[0]);
    expect(result.saved).not.toContainEqual(factory[0]);
  });

  it("returns independent copies so mutations to input do not affect output", () => {
    const factory = [mkPreset("Flat", 10)];
    const saved = [mkPreset("Custom", 10)];
    const result = groupPresets(factory, saved);
    // Mutating the original arrays should not affect the returned groups
    factory.push(mkPreset("Added Later", 10));
    saved.push(mkPreset("Also Added", 10));
    expect(result.builtin).toHaveLength(1);
    expect(result.saved).toHaveLength(1);
  });

  it("handles factory-only (no saved presets)", () => {
    const factory = [mkPreset("Flat", 10), mkPreset("Gaming", 10)];
    const result = groupPresets(factory, []);
    expect(result.builtin).toHaveLength(2);
    expect(result.saved).toHaveLength(0);
  });

  it("handles saved-only (no factory presets)", () => {
    const saved = [mkPreset("My EQ", 10)];
    const result = groupPresets([], saved);
    expect(result.builtin).toHaveLength(0);
    expect(result.saved).toHaveLength(1);
  });

  it("preserves name and band_count for each preset", () => {
    const factory = [mkPreset("Studio", 10)];
    const result = groupPresets(factory, []);
    expect(result.builtin[0].name).toBe("Studio");
    expect(result.builtin[0].band_count).toBe(10);
  });
});
