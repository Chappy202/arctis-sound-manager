/**
 * mixerLayout.test.ts — Pure unit tests for the orderChannels helper.
 * No DOM harness (no jsdom / happy-dom); pure logic only.
 *
 * TDD: tests were written before the implementation exists — run was RED first.
 */
import { describe, it, expect } from "vitest";
import { orderChannels, CANONICAL_CHANNEL_ORDER } from "./mixerLayout.js";

type Ch = { id: string };
const ch = (id: string): Ch => ({ id });

describe("CANONICAL_CHANNEL_ORDER", () => {
  it("is exactly [game, chat, media, aux]", () => {
    expect(CANONICAL_CHANNEL_ORDER).toEqual(["game", "chat", "media", "aux"]);
  });
});

describe("orderChannels", () => {
  it("empty input returns empty output", () => {
    expect(orderChannels([])).toEqual([]);
  });

  it("single standard channel is returned unchanged", () => {
    expect(orderChannels([ch("game")])).toEqual([ch("game")]);
  });

  it("standard channels are sorted into canonical order regardless of input order", () => {
    const input = [ch("aux"), ch("media"), ch("chat"), ch("game")];
    expect(orderChannels(input)).toEqual([ch("game"), ch("chat"), ch("media"), ch("aux")]);
  });

  it("standard channels already in canonical order stay in order", () => {
    const input = [ch("game"), ch("chat"), ch("media"), ch("aux")];
    expect(orderChannels(input)).toEqual([ch("game"), ch("chat"), ch("media"), ch("aux")]);
  });

  it("a custom channel is appended after all standard channels", () => {
    const input = [ch("voip"), ch("game"), ch("media")];
    // standard: game, media; custom: voip
    expect(orderChannels(input)).toEqual([ch("game"), ch("media"), ch("voip")]);
  });

  it("multiple custom channels preserve their original relative order", () => {
    const input = [ch("voip"), ch("music"), ch("aux"), ch("discord"), ch("game")];
    // standard: game, aux; custom: voip, music, discord (original relative order)
    expect(orderChannels(input)).toEqual([
      ch("game"),
      ch("aux"),
      ch("voip"),
      ch("music"),
      ch("discord"),
    ]);
  });

  it("missing standard channels are absent (no placeholders)", () => {
    // Only chat and aux are present — game and media should not appear
    const input = [ch("aux"), ch("chat")];
    expect(orderChannels(input)).toEqual([ch("chat"), ch("aux")]);
    expect(orderChannels(input)).toHaveLength(2);
  });

  it("all standard channels in reverse canonical order are sorted correctly", () => {
    const input = [ch("aux"), ch("media"), ch("chat"), ch("game")];
    const result = orderChannels(input);
    expect(result.map((c) => c.id)).toEqual(["game", "chat", "media", "aux"]);
  });

  it("only custom channels: original relative order is preserved", () => {
    const input = [ch("voip"), ch("discord"), ch("spotify")];
    expect(orderChannels(input)).toEqual([ch("voip"), ch("discord"), ch("spotify")]);
  });

  it("preserves extra properties on channel objects", () => {
    type FullCh = { id: string; volume: number };
    const input: FullCh[] = [
      { id: "aux", volume: 80 },
      { id: "game", volume: 100 },
    ];
    const result = orderChannels(input);
    expect(result[0]).toEqual({ id: "game", volume: 100 });
    expect(result[1]).toEqual({ id: "aux", volume: 80 });
  });

  it("does not mutate the input array", () => {
    const input = [ch("aux"), ch("game"), ch("chat"), ch("media")];
    const copy = [...input];
    orderChannels(input);
    expect(input).toEqual(copy);
  });

  it("custom channel interleaved with standards comes after all standards", () => {
    // Input: game, voip, chat, media, aux — voip is custom, interleaved
    const input = [ch("game"), ch("voip"), ch("chat"), ch("media"), ch("aux")];
    const result = orderChannels(input);
    // Standard section first, then voip
    expect(result.map((c) => c.id)).toEqual(["game", "chat", "media", "aux", "voip"]);
  });
});
