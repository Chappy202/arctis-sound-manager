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
 * Robustness: every network call carries a timeout.  The download in
 * particular MUST have a ceiling — without one, a stalled stream leaves
 * `downloadAndInstall()` awaiting forever (it neither resolves nor rejects),
 * which pins the UI in an "Installing…" state with no escape.  See
 * `DOWNLOAD_TIMEOUT_MS`.  Progress is surfaced via `reduceProgress` so the UI
 * can show real movement instead of a static spinner.
 */

import { check, type DownloadEvent } from "@tauri-apps/plugin-updater";

/** Timeout for the lightweight `latest.json` check (ms). */
export const CHECK_TIMEOUT_MS = 30_000;

/**
 * Hard ceiling for the whole download+install request (ms).  This is a
 * backstop against a stalled connection: past this bound the plugin aborts
 * the request and `downloadAndInstall()` rejects, so the UI can recover.
 * Generous enough not to false-trip a slow-but-healthy download of the
 * ~86 MB AppImage (3 min ≈ 480 KB/s floor).
 */
export const DOWNLOAD_TIMEOUT_MS = 180_000;

export interface UpdateInfo {
  version: string;
  date: string | null;
  body: string | null;
  /**
   * Download and install the update, then relaunch.  `onEvent` receives the
   * raw plugin download events (feed them to `reduceProgress` for a % state).
   * Rejects on failure (including timeout) so callers can reset their UI.
   */
  downloadAndInstall: (onEvent?: (e: DownloadEvent) => void) => Promise<void>;
}

/** Phase of an in-flight update install. */
export type DownloadPhase = "idle" | "downloading" | "installing";

/** Reactive state driving the progress bar + button label. */
export interface DownloadProgress {
  phase: DownloadPhase;
  /** Bytes received so far. */
  received: number;
  /** Total bytes, or null when the server sent no content-length. */
  total: number | null;
  /** 0–100, or null when total is unknown (indeterminate). */
  percent: number | null;
}

export const initialProgress: DownloadProgress = {
  phase: "idle",
  received: 0,
  total: null,
  percent: null,
};

/**
 * Fold a plugin `DownloadEvent` into the running progress state.  Pure — no
 * side effects — so it is trivially unit-testable and safe to call from a
 * Svelte reactive assignment.
 */
export function reduceProgress(state: DownloadProgress, event: DownloadEvent): DownloadProgress {
  switch (event.event) {
    case "Started": {
      const total = event.data.contentLength ?? null;
      return { phase: "downloading", received: 0, total, percent: total === null ? null : 0 };
    }
    case "Progress": {
      const received = state.received + event.data.chunkLength;
      const percent =
        state.total === null || state.total === 0
          ? null
          : Math.min(100, Math.round((received / state.total) * 100));
      return { ...state, phase: "downloading", received, percent };
    }
    case "Finished":
      // Download complete; the (fast, local) install/replace step runs next.
      return { ...state, phase: "installing", percent: state.total === null ? null : 100 };
    default:
      return state;
  }
}

/** Human-readable label for the install button, derived from progress. */
export function progressLabel(p: DownloadProgress): string {
  switch (p.phase) {
    case "downloading":
      return p.percent === null ? "Downloading…" : `Downloading… ${p.percent}%`;
    case "installing":
      return "Installing…";
    default:
      return "Install & Relaunch";
  }
}

/**
 * Check for an available update.
 *
 * Returns `null` when:
 * - No update is available (current version is up to date).
 * - The endpoint is unreachable (dev build, no internet, placeholder URL).
 *
 * Errors are caught and logged rather than thrown, so a failing update check
 * never breaks app startup.
 */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  try {
    const update = await check({ timeout: CHECK_TIMEOUT_MS });
    // `available` is deprecated (always true) — a null return means "up to date".
    if (!update) {
      return null;
    }
    return {
      version: update.version,
      date: update.date ?? null,
      body: update.body ?? null,
      downloadAndInstall: (onEvent) =>
        update.downloadAndInstall(onEvent, { timeout: DOWNLOAD_TIMEOUT_MS }),
    };
  } catch (err) {
    // Endpoint unreachable (dev / placeholder URL) or network error —
    // log at debug level only, never surface as an error to the user.
    console.debug("[updater] update check failed (non-fatal):", err);
    return null;
  }
}
