/**
 * updater.test.ts — Unit tests for the auto-updater logic.
 *
 * Two concerns are covered:
 *  1. `reduceProgress` / `progressLabel` — the pure download-progress state
 *     machine that drives the banner's progress bar and button label.
 *  2. `checkForUpdate` wiring — that a download *timeout* is always passed
 *     (so a stalled stream can never hang forever) and that progress events
 *     are forwarded to the caller's callback.
 *
 * The plugin's `check` is mocked so no Tauri IPC is needed.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";

vi.mock("@tauri-apps/plugin-updater", () => ({ check: vi.fn() }));
import { check } from "@tauri-apps/plugin-updater";

import {
  reduceProgress,
  progressLabel,
  initialProgress,
  checkForUpdate,
  DOWNLOAD_TIMEOUT_MS,
} from "./updater";

describe("reduceProgress", () => {
  it("Started with contentLength sets total and 0 percent", () => {
    const s = reduceProgress(initialProgress, { event: "Started", data: { contentLength: 200 } });
    expect(s.phase).toBe("downloading");
    expect(s.total).toBe(200);
    expect(s.received).toBe(0);
    expect(s.percent).toBe(0);
  });

  it("Started without contentLength is indeterminate", () => {
    const s = reduceProgress(initialProgress, { event: "Started", data: {} });
    expect(s.phase).toBe("downloading");
    expect(s.total).toBeNull();
    expect(s.percent).toBeNull();
  });

  it("Progress accumulates received bytes and computes percent", () => {
    let s = reduceProgress(initialProgress, { event: "Started", data: { contentLength: 200 } });
    s = reduceProgress(s, { event: "Progress", data: { chunkLength: 50 } });
    expect(s.received).toBe(50);
    expect(s.percent).toBe(25);
    s = reduceProgress(s, { event: "Progress", data: { chunkLength: 50 } });
    expect(s.received).toBe(100);
    expect(s.percent).toBe(50);
  });

  it("Progress with unknown total tracks bytes but stays indeterminate", () => {
    let s = reduceProgress(initialProgress, { event: "Started", data: {} });
    s = reduceProgress(s, { event: "Progress", data: { chunkLength: 1000 } });
    expect(s.received).toBe(1000);
    expect(s.percent).toBeNull();
  });

  it("percent never exceeds 100 even if bytes overshoot", () => {
    let s = reduceProgress(initialProgress, { event: "Started", data: { contentLength: 100 } });
    s = reduceProgress(s, { event: "Progress", data: { chunkLength: 150 } });
    expect(s.percent).toBe(100);
  });

  it("Finished moves to installing phase", () => {
    let s = reduceProgress(initialProgress, { event: "Started", data: { contentLength: 100 } });
    s = reduceProgress(s, { event: "Finished" });
    expect(s.phase).toBe("installing");
  });
});

describe("progressLabel", () => {
  it("downloading with known percent", () => {
    expect(progressLabel({ phase: "downloading", received: 25, total: 100, percent: 25 })).toBe(
      "Downloading… 25%",
    );
  });
  it("downloading indeterminate", () => {
    expect(progressLabel({ phase: "downloading", received: 0, total: null, percent: null })).toBe(
      "Downloading…",
    );
  });
  it("installing", () => {
    expect(progressLabel({ phase: "installing", received: 100, total: 100, percent: 100 })).toBe(
      "Installing…",
    );
  });
  it("restarting (post-install re-exec — Linux does not auto-relaunch)", () => {
    expect(progressLabel({ phase: "restarting", received: 100, total: 100, percent: 100 })).toBe(
      "Restarting…",
    );
  });
  it("idle falls back to the install prompt", () => {
    expect(progressLabel(initialProgress)).toBe("Install & Relaunch");
  });
});

describe("checkForUpdate wiring", () => {
  beforeEach(() => {
    vi.mocked(check).mockReset();
  });

  it("returns null when no update is available", async () => {
    vi.mocked(check).mockResolvedValue(null);
    expect(await checkForUpdate()).toBeNull();
  });

  it("returns null (never throws) when the check fails", async () => {
    vi.mocked(check).mockRejectedValue(new Error("endpoint unreachable"));
    expect(await checkForUpdate()).toBeNull();
  });

  it("passes a download timeout and forwards progress events", async () => {
    const recorded: { onEvent?: (e: unknown) => void; options?: { timeout?: number } } = {};
    const fakeUpdate = {
      version: "9.9.9",
      date: undefined,
      body: undefined,
      downloadAndInstall: vi.fn((onEvent: (e: unknown) => void, options: { timeout?: number }) => {
        recorded.onEvent = onEvent;
        recorded.options = options;
        return Promise.resolve();
      }),
    };
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    vi.mocked(check).mockResolvedValue(fakeUpdate as any);

    const info = await checkForUpdate();
    expect(info).not.toBeNull();
    expect(info!.version).toBe("9.9.9");

    const seen: number[] = [];
    await info!.downloadAndInstall((e) => {
      if (e.event === "Progress") seen.push(e.data.chunkLength);
    });

    // A hard timeout ceiling is always supplied — a stalled stream can't hang forever.
    expect(recorded.options?.timeout).toBe(DOWNLOAD_TIMEOUT_MS);
    // The caller's progress callback is wired straight through to the plugin.
    recorded.onEvent?.({ event: "Progress", data: { chunkLength: 42 } });
    expect(seen).toEqual([42]);
  });
});
