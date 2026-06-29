import { describe, it, expect } from "vitest";
import { statusLabel, dotKind, canStart, canStop, canRestart, autostartDisabledReason } from "./daemonControl";
import type { DaemonStatus } from "./ipc";

const base: DaemonStatus = { running: false, managed_by: "stopped", autostart_enabled: false, systemd_available: true, binary_path: "/usr/bin/asm-cli", unit_installed: false };

describe("daemonControl", () => {
  it("labels", () => {
    expect(statusLabel({ ...base, running: true, managed_by: "systemd" })).toBe("Running (systemd)");
    expect(statusLabel({ ...base, running: true, managed_by: "manual" })).toBe("Running (manual)");
    expect(statusLabel(base)).toBe("Stopped");
  });
  it("button enablement", () => {
    expect(canStart(base)).toBe(true);
    expect(canStop(base)).toBe(false);
    expect(canStop({ ...base, running: true })).toBe(true);
    expect(canStart({ ...base, running: true })).toBe(false);
  });
  it("autostart disabled without systemd", () => {
    expect(autostartDisabledReason(base)).toBeNull();
    expect(autostartDisabledReason({ ...base, systemd_available: false })).toMatch(/systemd/i);
  });
});
