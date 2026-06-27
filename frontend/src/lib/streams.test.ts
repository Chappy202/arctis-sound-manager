import { describe, it, expect } from "vitest";
import { groupStreamsByChannel } from "./streams.js";
import type { AppStream } from "./ipc.js";

const mk = (binary: string, current_channel: string | null): AppStream => ({
  id: 1, binary, app_name: binary, pid: null, icon_name: null,
  media_name: null, current_channel, routed: false,
});

describe("groupStreamsByChannel", () => {
  it("buckets streams by channel id and unrouted", () => {
    const streams = [mk("firefox", "game"), mk("spotify", null), mk("discord", "chat")];
    const g = groupStreamsByChannel(streams, ["game", "chat", "media", "aux"]);
    expect(g.byChannel.game.map((s) => s.binary)).toEqual(["firefox"]);
    expect(g.byChannel.chat.map((s) => s.binary)).toEqual(["discord"]);
    expect(g.byChannel.media).toEqual([]);
    expect(g.unrouted.map((s) => s.binary)).toEqual(["spotify"]);
  });

  it("treats streams on unknown channels as unrouted", () => {
    const g = groupStreamsByChannel([mk("x", "ghost")], ["game"]);
    expect(g.unrouted.map((s) => s.binary)).toEqual(["x"]);
  });
});
