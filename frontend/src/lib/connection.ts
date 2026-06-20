/**
 * connection.ts — Pure helpers for daemon-reachability status.
 *
 * These are extracted from AppShell so they can be unit-tested without a DOM
 * or Svelte runtime.  AppShell imports and calls them instead of repeating
 * the inline ternary chain.
 */

export type ConnectionStatus = "connected" | "connecting" | "disconnected";

/**
 * Derive the connection status from daemon-reachability signals.
 *
 * - loadError set (non-null)  → daemon unreachable → "disconnected"
 * - engineState === null      → waiting for first reply → "connecting"
 * - engineState !== null      → daemon replied successfully → "connected"
 *
 * Note: device_present is intentionally NOT consulted.  The dot reflects
 * whether the daemon is reachable, not whether a headset HID device is
 * attached.
 */
export function deriveConnectionStatus(
  loadError: string | null,
  engineState: unknown | null,
): ConnectionStatus {
  if (loadError !== null) return "disconnected";
  if (engineState === null) return "connecting";
  return "connected";
}

/** Human-readable label for the connection status. */
export function connectionLabel(status: ConnectionStatus): string {
  switch (status) {
    case "connected":    return "Daemon connected";
    case "connecting":   return "Connecting…";
    case "disconnected": return "Daemon offline";
  }
}
