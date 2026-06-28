import type { AppStream } from "../ipc.js";

/** Full hover text for an app pill. Always includes the full app_name (so a
 *  truncated pill always has recourse), and appends the media title when it's
 *  present and distinct. */
export function pillTitle(stream: Pick<AppStream, "app_name" | "media_name">): string {
  const media = stream.media_name?.trim();
  if (media && media !== stream.app_name) {
    return `${stream.app_name} — ${media}`;
  }
  return stream.app_name;
}
