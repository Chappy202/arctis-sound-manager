import { describe, it, expect } from "vitest";
import { findMicPresetDescription } from "./micPresetUtils.js";
import type { MicPresetSnapshot } from "../ipc.js";

const mkPreset = (name: string, description: string): MicPresetSnapshot => ({ name, description });

describe("findMicPresetDescription", () => {
  it("returns null when name is empty", () => {
    const presets = [mkPreset("Clear Voice", "Optimised for speech clarity")];
    expect(findMicPresetDescription("", presets)).toBeNull();
  });

  it("returns null when presets array is empty", () => {
    expect(findMicPresetDescription("Clear Voice", [])).toBeNull();
  });

  it("returns the description for a matching preset name", () => {
    const presets = [
      mkPreset("Clear Voice", "Optimised for speech clarity"),
      mkPreset("Warm", "Adds warmth to vocal tone"),
    ];
    expect(findMicPresetDescription("Clear Voice", presets)).toBe(
      "Optimised for speech clarity",
    );
  });

  it("returns the correct description when multiple presets exist", () => {
    const presets = [
      mkPreset("Flat", "No processing"),
      mkPreset("Gaming", "Bright and punchy"),
      mkPreset("Broadcast", "Studio-style compression"),
    ];
    expect(findMicPresetDescription("Broadcast", presets)).toBe(
      "Studio-style compression",
    );
  });

  it("returns null when no preset matches the name", () => {
    const presets = [mkPreset("Clear Voice", "Optimised for speech clarity")];
    expect(findMicPresetDescription("Unknown", presets)).toBeNull();
  });

  it("is case-sensitive — 'flat' does not match 'Flat'", () => {
    const presets = [mkPreset("Flat", "No processing")];
    expect(findMicPresetDescription("flat", presets)).toBeNull();
  });

  it("returns description of the first matching preset (names should be unique, but guards against duplicates)", () => {
    const presets = [
      mkPreset("Flat", "First flat"),
      mkPreset("Flat", "Second flat"),
    ];
    expect(findMicPresetDescription("Flat", presets)).toBe("First flat");
  });
});
