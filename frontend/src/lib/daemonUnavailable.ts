import type { ConnectionStatus } from "./stores/connection.js";

/** Which sub-view DaemonUnavailable renders for a given connection status. */
export function viewFor(status: ConnectionStatus): "connecting" | "disconnected" | "hidden" {
  if (status === "connected") return "hidden";
  return status; // "connecting" | "disconnected"
}
