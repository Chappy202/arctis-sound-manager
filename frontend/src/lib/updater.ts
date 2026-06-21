/**
 * Auto-updater integration (tauri-plugin-updater).
 *
 * Performs a background update check on startup.  If an update is available
 * the caller receives a populated `UpdateInfo` object; no download or install
 * is triggered automatically — the UI must confirm with the user first.
 *
 * In development (non-packaged) builds the updater endpoint is unreachable
 * and `check()` returns `null` — the function handles this silently.
 *
 * OWNER-FILL: Set `plugins.updater.endpoints` in tauri.conf.json to your
 * real update server URL before shipping.  The current value is a placeholder.
 */

import { check } from "@tauri-apps/plugin-updater";

export interface UpdateInfo {
  version: string;
  date: string | null;
  body: string | null;
  /** Call this to download and install the update, then relaunch. */
  downloadAndInstall: () => Promise<void>;
}

/**
 * Check for an available update.
 *
 * Returns `null` when:
 * - No update is available (current version is up to date).
 * - The endpoint is unreachable (dev build, no internet, placeholder URL).
 *
 * Errors are caught and logged to the console rather than thrown, so a
 * failing update check never breaks app startup.
 */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  try {
    const update = await check();
    if (!update?.available) {
      return null;
    }
    return {
      version: update.version,
      date: update.date ?? null,
      body: update.body ?? null,
      downloadAndInstall: async () => {
        await update.downloadAndInstall();
      },
    };
  } catch (err) {
    // Endpoint unreachable (dev / placeholder URL) or network error —
    // log at debug level only, never surface as an error to the user.
    console.debug("[updater] update check failed (non-fatal):", err);
    return null;
  }
}
