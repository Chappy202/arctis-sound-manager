/**
 * stores/connection.test.ts — Unit tests for the derived connection status +
 * health monitor.
 *
 * `connectionStatus` is DERIVED from loadError + engineState, so the mock for
 * stores.js provides REAL writable stores (a derived store needs subscribable
 * dependencies). ipc.js getState is mocked; fake timers drive the monitor.
 *
 * The `started` module guard requires a fresh module per test — achieved with
 * vi.resetModules() + dynamic imports in each test.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { get } from "svelte/store";
import type { EngineState } from "../ipc.js";

// ---------------------------------------------------------------------------
// Hoisted mocks. stores.js exposes REAL writables so the derived
// connectionStatus can subscribe to them.
// ---------------------------------------------------------------------------
vi.mock("../ipc.js", () => ({
  getState: vi.fn(),
}));

vi.mock("../stores.js", async () => {
  const { writable } = await import("svelte/store");
  return {
    engineState: writable<EngineState | null>(null),
    loadError: writable<string | null>(null),
    init: vi.fn().mockResolvedValue(undefined),
    destroy: vi.fn(),
  };
});

const fakeState: Partial<EngineState> = {
  active_profile: "Default",
  channels: [],
  device_present: false,
};

async function loadConn() {
  return import("./connection.js");
}
async function loadIpc() {
  return import("../ipc.js");
}
async function loadStores() {
  return import("../stores.js");
}

/** The vi.mock factory's writables are cached across resetModules, so reset
 * their VALUES between tests to prevent state leaking. */
async function resetStoreState() {
  const s = await loadStores();
  s.engineState.set(null);
  s.loadError.set(null);
}

// ---------------------------------------------------------------------------
// connectionStatus derivation (single source of truth)
// ---------------------------------------------------------------------------
describe("connectionStatus (derived from loadError + engineState)", () => {
  beforeEach(async () => { vi.resetModules(); await resetStoreState(); });

  it("is 'connecting' when loadError null and engineState null", async () => {
    const conn = await loadConn();
    expect(get(conn.connectionStatus)).toBe("connecting");
  });

  it("is 'disconnected' when loadError is set", async () => {
    const stores = await loadStores();
    const conn = await loadConn();
    stores.loadError.set("daemon down");
    expect(get(conn.connectionStatus)).toBe("disconnected");
  });

  it("is 'connected' when engineState set and loadError null", async () => {
    const stores = await loadStores();
    const conn = await loadConn();
    stores.engineState.set(fakeState as EngineState);
    expect(get(conn.connectionStatus)).toBe("connected");
  });
});

// ---------------------------------------------------------------------------
// startConnectionMonitor
// ---------------------------------------------------------------------------
describe("startConnectionMonitor", () => {
  beforeEach(async () => {
    vi.resetModules();
    await resetStoreState();
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("derives connectionStatus 'connected' after a successful poll", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState).mockResolvedValue(fakeState as EngineState);
    const conn = await loadConn();

    const stop = conn.startConnectionMonitor(1000);
    expect(get(conn.connectionStatus)).toBe("connecting"); // initial

    await vi.advanceTimersByTimeAsync(1000);

    expect(get(conn.connectionStatus)).toBe("connected");
    stop();
  });

  it("derives connectionStatus 'disconnected' when poll rejects", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState).mockRejectedValue(new Error("daemon down"));
    const conn = await loadConn();

    const stop = conn.startConnectionMonitor(1000);
    await vi.advanceTimersByTimeAsync(1000);

    expect(get(conn.connectionStatus)).toBe("disconnected");
    stop();
  });

  it("stop() halts further polling and status no longer changes", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState).mockResolvedValue(fakeState as EngineState);
    const conn = await loadConn();

    const stop = conn.startConnectionMonitor(1000);
    await vi.advanceTimersByTimeAsync(1000);
    expect(get(conn.connectionStatus)).toBe("connected");
    const callsBeforeStop = vi.mocked(ipc.getState).mock.calls.length;

    stop();

    vi.mocked(ipc.getState).mockRejectedValue(new Error("daemon down"));
    await vi.advanceTimersByTimeAsync(3000);

    expect(get(conn.connectionStatus)).toBe("connected"); // unchanged
    expect(vi.mocked(ipc.getState).mock.calls.length).toBe(callsBeforeStop);
  });

  it("second startConnectionMonitor call is a no-op (started guard)", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState).mockResolvedValue(fakeState as EngineState);
    const conn = await loadConn();

    const stop1 = conn.startConnectionMonitor(1000);
    const stop2 = conn.startConnectionMonitor(1000); // no-op
    await vi.advanceTimersByTimeAsync(1000);

    expect(vi.mocked(ipc.getState).mock.calls.length).toBe(1);
    stop1();
    stop2();
  });

  it("populates engineState on a successful poll when it was null (recovery)", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState).mockResolvedValue(fakeState as EngineState);
    const stores = await loadStores();
    const conn = await loadConn();

    expect(get(stores.engineState)).toBeNull();
    const stop = conn.startConnectionMonitor(1000);
    await vi.advanceTimersByTimeAsync(1000);

    expect(get(stores.engineState)).toEqual(fakeState);
    stop();
  });

  it("does NOT overwrite an already-populated engineState on success", async () => {
    const ipc = await loadIpc();
    const existing = { active_profile: "Existing", channels: [], device_present: true };
    vi.mocked(ipc.getState).mockResolvedValue(fakeState as EngineState);
    const stores = await loadStores();
    const conn = await loadConn();

    stores.engineState.set(existing as unknown as EngineState);
    const stop = conn.startConnectionMonitor(1000);
    await vi.advanceTimersByTimeAsync(1000);

    expect(get(stores.engineState)).toEqual(existing); // unchanged (event is SoT)
    stop();
  });

  it("clears loadError on recovery", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState)
      .mockRejectedValueOnce(new Error("timeout"))
      .mockResolvedValue(fakeState as EngineState);
    const stores = await loadStores();
    const conn = await loadConn();

    const stop = conn.startConnectionMonitor(1000);
    await vi.advanceTimersByTimeAsync(1000); // poll 1 → error
    expect(get(stores.loadError)).toBe("timeout");
    expect(get(conn.connectionStatus)).toBe("disconnected");

    await vi.advanceTimersByTimeAsync(1000); // poll 2 → recovery
    expect(get(stores.loadError)).toBeNull();
    expect(get(conn.connectionStatus)).toBe("connected");
    stop();
  });

  it("sets loadError with the error message on rejection", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState).mockRejectedValue(new Error("timeout"));
    const stores = await loadStores();
    const conn = await loadConn();

    const stop = conn.startConnectionMonitor(1000);
    await vi.advanceTimersByTimeAsync(1000);

    expect(get(stores.loadError)).toBe("timeout");
    stop();
  });

  it("skips overlapping polls (in-flight guard)", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState).mockImplementation(
      () => new Promise((resolve) => setTimeout(() => resolve(fakeState as EngineState), 3000)),
    );
    const conn = await loadConn();
    const stop = conn.startConnectionMonitor(1000);

    await vi.advanceTimersByTimeAsync(2000); // timer fires twice, one in-flight

    expect(vi.mocked(ipc.getState).mock.calls.length).toBe(1);
    stop();
  });
});

// ---------------------------------------------------------------------------
// reconnect
// ---------------------------------------------------------------------------
describe("reconnect", () => {
  beforeEach(async () => { vi.resetModules(); await resetStoreState(); });
  afterEach(() => vi.clearAllMocks());

  it("derives 'connecting' synchronously (clears engineState + loadError) before awaiting init", async () => {
    const stores = await loadStores();
    const conn = await loadConn();

    stores.engineState.set(fakeState as EngineState);
    expect(get(conn.connectionStatus)).toBe("connected");

    const promise = conn.reconnect();
    expect(get(conn.connectionStatus)).toBe("connecting"); // synchronous

    await promise;
  });

  it("calls destroy() then init() from stores.js", async () => {
    const stores = await loadStores();
    const conn = await loadConn();

    await conn.reconnect();

    expect(vi.mocked(stores.destroy)).toHaveBeenCalledOnce();
    expect(vi.mocked(stores.init)).toHaveBeenCalledOnce();
  });
});
