import type { DaemonStatus } from "./ipc.js";

export function statusLabel(s: DaemonStatus): string {
  if (!s.running) return "Stopped";
  return s.managed_by === "systemd" ? "Running (systemd)" : "Running (manual)";
}

export function dotKind(s: DaemonStatus): "ok" | "off" {
  return s.running ? "ok" : "off";
}

export function canStart(s: DaemonStatus): boolean {
  return !s.running;
}

export function canStop(s: DaemonStatus): boolean {
  return s.running;
}

export function canRestart(s: DaemonStatus): boolean {
  return s.running;
}

export function autostartDisabledReason(s: DaemonStatus): string | null {
  return s.systemd_available ? null : "Autostart needs systemd (user manager) — not available here.";
}
