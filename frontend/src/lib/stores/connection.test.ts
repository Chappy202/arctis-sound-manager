/**
 * stores/connection.test.ts — Unit tests for daemon connection store + monitor.
 *
 * All tests are pure: ipc.js and stores.js are mocked; Tauri runtime is never
 * invoked. Fake timers drive setInterval so we don't wait on real time.
 *
 * The `started` module guard requires a fresh module per test — achieved with
 * vi.resetModules() + dynamic imports in each test.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { get } from "svelte/store";
import type { EngineState } from "../ipc.js";

// ---------------------------------------------------------------------------
// Hoisted mocks — factory is re-invoked after each vi.resetModules() call,
// so each test gets fresh vi.fn() instances.
// ---------------------------------------------------------------------------
vi.mock("../ipc.js", () => ({
  getState: vi.fn(),
}));

vi.mock("../stores.js", () => ({
  engineState: { set: vi.fn() },
  loadError: { set: vi.fn() },
  init: vi.fn().mockResolvedValue(undefined),
  destroy: vi.fn(),
}));

// ---------------------------------------------------------------------------
// Minimal EngineState fixture for poll-success cases.
// ---------------------------------------------------------------------------
const fakeState: Partial<EngineState> = {
  active_profile: "Default",
  channels: [],
  device_present: false,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Fresh import of the connection store module (respects vi.resetModules()). */
async function loadConn() {
  return import("./connection.js");
}

async function loadIpc() {
  return import("../ipc.js");
}

async function loadStores() {
  return import("../stores.js");
}

// ---------------------------------------------------------------------------
// startConnectionMonitor
// ---------------------------------------------------------------------------
describe("startConnectionMonitor", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("flips connectionStatus to 'connected' after a successful poll", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState).mockResolvedValue(fakeState as EngineState);

    const conn = await loadConn();

    const stop = conn.startConnectionMonitor(1000);
    expect(get(conn.connectionStatus)).toBe("connecting"); // initial value

    await vi.advanceTimersByTimeAsync(1000);

    expect(get(conn.connectionStatus)).toBe("connected");
    stop();
  });

  it("flips connectionStatus to 'disconnected' when poll rejects", async () => {
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

    // First poll fires → connected.
    await vi.advanceTimersByTimeAsync(1000);
    expect(get(conn.connectionStatus)).toBe("connected");
    const callsBeforeStop = vi.mocked(ipc.getState).mock.calls.length;

    stop();

    // Subsequent ticks should NOT fire.
    vi.mocked(ipc.getState).mockRejectedValue(new Error("daemon down"));
    await vi.advanceTimersByTimeAsync(3000);

    expect(get(conn.connectionStatus)).toBe("connected"); // unchanged
    expect(vi.mocked(ipc.getState).mock.calls.length).toBe(callsBeforeStop);
  });

  it("second startConnectionMonitor call is a no-op (module-level started guard)", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState).mockResolvedValue(fakeState as EngineState);

    const conn = await loadConn();

    const stop1 = conn.startConnectionMonitor(1000);
    const stop2 = conn.startConnectionMonitor(1000); // should be no-op

    await vi.advanceTimersByTimeAsync(1000);

    // Only ONE poll should fire (not two).
    expect(vi.mocked(ipc.getState).mock.calls.length).toBe(1);

    stop1();
    stop2();
  });

  it("sets engineState and loadError(null) on success", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState).mockResolvedValue(fakeState as EngineState);

    const stores = await loadStores();
    const conn = await loadConn();

    const stop = conn.startConnectionMonitor(1000);
    await vi.advanceTimersByTimeAsync(1000);

    expect(vi.mocked(stores.engineState.set)).toHaveBeenCalledWith(fakeState);
    expect(vi.mocked(stores.loadError.set)).toHaveBeenCalledWith(null);
    stop();
  });

  it("sets loadError with error message on rejection", async () => {
    const ipc = await loadIpc();
    vi.mocked(ipc.getState).mockRejectedValue(new Error("timeout"));

    const stores = await loadStores();
    const conn = await loadConn();

    const stop = conn.startConnectionMonitor(1000);
    await vi.advanceTimersByTimeAsync(1000);

    expect(vi.mocked(stores.loadError.set)).toHaveBeenCalledWith("timeout");
    stop();
  });

  it("skips overlapping polls (in-flight guard)", async () => {
    const ipc = await loadIpc();
    // Simulate a slow getState that takes longer than the interval.
    vi.mocked(ipc.getState).mockImplementation(
      () => new Promise((resolve) => setTimeout(() => resolve(fakeState as EngineState), 3000)),
    );

    const conn = await loadConn();
    const stop = conn.startConnectionMonitor(1000);

    // Advance 2 intervals — timer fires twice but only ONE in-flight.
    await vi.advanceTimersByTimeAsync(2000);

    expect(vi.mocked(ipc.getState).mock.calls.length).toBe(1);
    stop();
  });
});

// ---------------------------------------------------------------------------
// markConnected / markDisconnected
// ---------------------------------------------------------------------------
describe("markConnected / markDisconnected", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("markConnected sets connectionStatus to 'connected'", async () => {
    const conn = await loadConn();
    conn.markConnected();
    expect(get(conn.connectionStatus)).toBe("connected");
  });

  it("markDisconnected sets connectionStatus to 'disconnected'", async () => {
    const conn = await loadConn();
    conn.markDisconnected();
    expect(get(conn.connectionStatus)).toBe("disconnected");
  });

  it("initial value is 'connecting'", async () => {
    const conn = await loadConn();
    expect(get(conn.connectionStatus)).toBe("connecting");
  });
});

// ---------------------------------------------------------------------------
// reconnect
// ---------------------------------------------------------------------------
describe("reconnect", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("sets connectionStatus to 'connecting' synchronously before awaiting init", async () => {
    // Start connected so we can observe the transition.
    const conn = await loadConn();
    conn.markConnected();
    expect(get(conn.connectionStatus)).toBe("connected");

    const promise = conn.reconnect();
    // Synchronous check — must already be 'connecting'.
    expect(get(conn.connectionStatus)).toBe("connecting");

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
