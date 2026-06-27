import type { AppStream } from "./ipc.js";

export interface GroupedStreams {
  byChannel: Record<string, AppStream[]>;
  unrouted: AppStream[];
}

/**
 * Group live streams under their resolved channel id. Streams whose
 * current_channel is null or not in `channelIds` go to `unrouted` (the
 * Master "Apps to be routed" tray).
 */
export function groupStreamsByChannel(
  streams: AppStream[],
  channelIds: string[],
): GroupedStreams {
  const byChannel: Record<string, AppStream[]> = {};
  for (const id of channelIds) byChannel[id] = [];
  const unrouted: AppStream[] = [];
  for (const s of streams) {
    if (s.current_channel && byChannel[s.current_channel]) {
      byChannel[s.current_channel].push(s);
    } else {
      unrouted.push(s);
    }
  }
  return { byChannel, unrouted };
}
