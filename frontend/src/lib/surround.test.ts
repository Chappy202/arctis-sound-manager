/**
 * surround.test.ts — Pure-helper unit tests for surround.ts.
 *
 * Mirrors mic.test.ts pattern: no DOM, no Svelte, no IPC. Logic is in pure
 * helpers tested with vitest.
 */

import { describe, it, expect } from "vitest";
import { hrirDisplayName, channelChecked, toggleChannel } from "./surround.js";

// ---------------------------------------------------------------------------
// hrirDisplayName
// ---------------------------------------------------------------------------

describe("hrirDisplayName", () => {
  it("strips a two-digit leading prefix and converts dashes to spaces", () => {
    expect(hrirDisplayName("02-dh-dolby-headphone")).toBe("Dh Dolby Headphone");
  });

  it("strips the 00-default-asm prefix correctly", () => {
    expect(hrirDisplayName("00-default-asm")).toBe("Default Asm");
  });

  it("handles a stem with no numeric prefix", () => {
    expect(hrirDisplayName("flat")).toBe("Flat");
  });

  it("handles dashes without numeric prefix", () => {
    expect(hrirDisplayName("custom-hrir-v2")).toBe("Custom Hrir V2");
  });

  it("title-cases each word (first char upper, rest lower)", () => {
    expect(hrirDisplayName("01-ALLCAPS-word")).toBe("Allcaps Word");
  });

  it("handles a single-word stem after stripping prefix", () => {
    expect(hrirDisplayName("05-dolby")).toBe("Dolby");
  });

  it("handles multi-digit numeric prefix (e.g. 12-)", () => {
    expect(hrirDisplayName("12-room-reverb")).toBe("Room Reverb");
  });

  it("handles stem that is only a number-prefixed word", () => {
    expect(hrirDisplayName("1-flat")).toBe("Flat");
  });

  it("returns empty string for empty input", () => {
    expect(hrirDisplayName("")).toBe("");
  });
});

// ---------------------------------------------------------------------------
// channelChecked
// ---------------------------------------------------------------------------

describe("channelChecked", () => {
  it("returns true when channel is in the list", () => {
    expect(channelChecked("game", ["game", "media"])).toBe(true);
    expect(channelChecked("media", ["game", "media"])).toBe(true);
  });

  it("returns false when channel is not in the list", () => {
    expect(channelChecked("chat", ["game", "media"])).toBe(false);
  });

  it("returns false for an empty list", () => {
    expect(channelChecked("game", [])).toBe(false);
  });

  it("returns false when only a different channel is present", () => {
    expect(channelChecked("game", ["chat"])).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// toggleChannel
// ---------------------------------------------------------------------------

describe("toggleChannel", () => {
  it("removes a channel that is already in the list", () => {
    expect(toggleChannel("game", ["game", "media"])).toEqual(["media"]);
  });

  it("adds a channel that is not in the list", () => {
    expect(toggleChannel("chat", ["game", "media"])).toEqual(["game", "media", "chat"]);
  });

  it("preserves order of remaining channels when removing", () => {
    expect(toggleChannel("media", ["game", "chat", "media"])).toEqual(["game", "chat"]);
  });

  it("handles toggling on an empty list (adds the channel)", () => {
    expect(toggleChannel("game", [])).toEqual(["game"]);
  });

  it("handles removing the last channel (returns empty list)", () => {
    expect(toggleChannel("game", ["game"])).toEqual([]);
  });

  it("does not mutate the original array", () => {
    const original = ["game", "media"];
    toggleChannel("game", original);
    expect(original).toEqual(["game", "media"]);
  });

  it("appends newly added channel at the end", () => {
    const result = toggleChannel("chat", ["game"]);
    expect(result[result.length - 1]).toBe("chat");
  });
});
