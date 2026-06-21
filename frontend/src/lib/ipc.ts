/**
 * ipc.ts — Typed wrappers around Tauri invoke/listen for Arctis Sound Manager.
 *
 * NOTE: Tauri v2 converts Rust snake_case *command parameter names* to camelCase
 * when routing from JS → Rust. Struct fields serialised in the *response* keep
 * their serde names (snake_case). See the command definitions in
 * src-tauri/src/commands.rs for the canonical parameter names.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// ---------------------------------------------------------------------------
// Types — mirror crates/engine/src/state.rs exactly (serde names = snake_case)
// ---------------------------------------------------------------------------

export interface EqBandSnapshot {
  kind: string;
  freq_hz: number;
  q: number;
  gain_db: number;
}

export interface ChannelSnapshot {
  id: string;
  node_name: string;
  output_device: string | null;
  /** Full per-band EQ state; empty = flat / no overrides. */
  eq_bands: EqBandSnapshot[];
}

export interface MicStageSnapshot {
  /** serde snake_case StageName: "gain"|"highpass"|"suppression"|"compressor"|"gate"|"mic_eq" */
  kind: string;
  enabled: boolean;
  available: boolean;
  params: Record<string, number>; // BTreeMap<String,f32>; keys per engine state() builder
}

export interface MicSnapshot {
  enabled: boolean;
  stages: MicStageSnapshot[];
  eq_bands: EqBandSnapshot[]; // reuse existing EqBandSnapshot
  /** Active suppression backend: "deep_filter" | "rnnoise" */
  suppression_backend: string;
  /** Backends whose plugin is present on this machine */
  available_suppression_backends: string[];
}

export interface SurroundSnapshot {
  enabled: boolean;
  hrir: string | null;
  available_hrirs: string[];
  channels: string[];
  hw_sink: string | null;
}

export interface EngineState {
  active_profile: string;
  profiles: string[];
  channels: ChannelSnapshot[];
  /** [app_binary, target_sink] pairs */
  routes: [string, string][];
  device_present: boolean;
  device_fields: Record<string, string>;
  mic: MicSnapshot;
  surround: SurroundSnapshot;
}

// ---------------------------------------------------------------------------
// Pure helpers — build invoke arg objects (testable without Tauri runtime)
// ---------------------------------------------------------------------------

/** Builds the camelCase arg object for the set_eq_band command. */
export function buildSetEqBandArgs(
  channel: string,
  band: number,
  kind: string,
  freq_hz: number,
  q: number,
  gain_db: number,
): { channel: string; band: number; kind: string; freqHz: number; q: number; gainDb: number } {
  return { channel, band, kind, freqHz: freq_hz, q, gainDb: gain_db };
}

/** Builds the camelCase arg object for the set_route command. */
export function buildSetRouteArgs(
  app_binary: string,
  target_sink: string,
): { appBinary: string; targetSink: string } {
  return { appBinary: app_binary, targetSink: target_sink };
}

/** Builds the camelCase arg object for set_channel_output. */
export function buildSetChannelOutputArgs(
  channel: string,
  device: string | null,
): { channel: string; device: string | null } {
  return { channel, device };
}

/** Builds the arg object for device_set (no rename needed — both args are lowercase). */
export function buildDeviceSetArgs(
  control: string,
  value: number,
): { control: string; value: number } {
  return { control, value };
}

/** Builds the camelCase arg object for the mic_eq_band command.
 * Tauri v2 converts these camelCase keys back to the Rust command's snake_case params
 * at the invoke boundary — the same round-trip as buildSetEqBandArgs; this is intentional. */
export function buildMicEqBandArgs(
  band: number,
  kind: string,
  freq_hz: number,
  q: number,
  gain_db: number,
): { band: number; kind: string; freqHz: number; q: number; gainDb: number } {
  return { band, kind, freqHz: freq_hz, q, gainDb: gain_db };
}

// ---------------------------------------------------------------------------
// IPC commands
// ---------------------------------------------------------------------------

/** Fetch the current full EngineState from the daemon. */
export const getState = (): Promise<EngineState> => invoke<EngineState>("get_state");

/** Switch the active profile and return the updated EngineState. */
export const switchProfile = (name: string): Promise<EngineState> =>
  invoke<EngineState>("switch_profile", { name });

/** Create a new profile with the given name; returns updated EngineState. */
export const profileNew = (name: string): Promise<EngineState> =>
  invoke<EngineState>("profile_new", { name });

/**
 * Set (or clear) the output device for a channel.
 * Returns the updated EngineState so the caller can apply it to the store
 * immediately for snappy feedback (no need to wait for the state-changed event).
 */
export const setChannelOutput = (channel: string, device: string | null): Promise<EngineState> =>
  invoke<EngineState>("set_channel_output", buildSetChannelOutputArgs(channel, device));

/** Update a parametric EQ band; returns updated EngineState. */
export const setEqBand = (
  channel: string,
  band: number,
  kind: string,
  freq_hz: number,
  q: number,
  gain_db: number,
): Promise<EngineState> =>
  invoke<EngineState>("set_eq_band", buildSetEqBandArgs(channel, band, kind, freq_hz, q, gain_db));

/** Route an app binary to a target sink; returns updated EngineState. */
export const setRoute = (app_binary: string, target_sink: string): Promise<EngineState> =>
  invoke<EngineState>("set_route", buildSetRouteArgs(app_binary, target_sink));

/**
 * Set a single device hardware control by name.
 * Writes are gated by the daemon's enabled_writes allowlist (empty until Task 7 owner-validation).
 * Returns the updated EngineState on success, or throws CommandError on gate refusal.
 */
export const deviceSet = (control: string, value: number): Promise<EngineState> =>
  invoke<EngineState>("device_set", buildDeviceSetArgs(control, value));

/** Enable or disable the whole mic chain (master switch). */
export const micEnable = (enabled: boolean): Promise<EngineState> =>
  invoke<EngineState>("mic_enable", { enabled });

/** Enable or disable a named mic DSP stage (gain|highpass|rnnoise|compressor|gate|eq). */
export const micStage = (stage: string, enabled: boolean): Promise<EngineState> =>
  invoke<EngineState>("mic_stage", { stage, enabled });

/** Set a named mic DSP parameter (gain_db|highpass_freq|vad_threshold|…). */
export const micSet = (param: string, value: number): Promise<EngineState> =>
  invoke<EngineState>("mic_set", { param, value });

/** Set one band of the mic EQ (live, no restart). */
export const micEqBand = (
  band: number,
  kind: string,
  freq_hz: number,
  q: number,
  gain_db: number,
): Promise<EngineState> =>
  invoke<EngineState>("mic_eq_band", buildMicEqBandArgs(band, kind, freq_hz, q, gain_db));

/** Set (or clear) the hardware mic capture source. */
export const micHwMic = (device: string | null): Promise<EngineState> =>
  invoke<EngineState>("mic_hw_mic", { device });

/** Switch the noise-suppression backend ("deep_filter" | "rnnoise"). */
export const micSuppressionBackend = (backend: string): Promise<EngineState> =>
  invoke<EngineState>("mic_suppression_backend", { backend });

// ── F1.5: Surround / HRIR commands ──────────────────────────────────────────

/** Enable or disable virtual surround (master switch). */
export const surroundEnable = (enabled: boolean): Promise<EngineState> =>
  invoke<EngineState>("surround_enable", { enabled });

/** Set the active HRIR profile by stem name (e.g. "02-dh-dolby-headphone"). */
export const surroundSetHrir = (name: string): Promise<EngineState> =>
  invoke<EngineState>("surround_set_hrir", { name });

/** Set which channels are routed through surround (e.g. ["game", "media"]). */
export const surroundSetChannels = (channels: string[]): Promise<EngineState> =>
  invoke<EngineState>("surround_set_channels", { channels });

/** Pin (or clear) the surround output to a specific hardware sink. */
export const surroundSetHwSink = (hwSink: string | null): Promise<EngineState> =>
  invoke<EngineState>("surround_set_hw_sink", { hwSink });

// ---------------------------------------------------------------------------
// Event subscriptions
// ---------------------------------------------------------------------------

/**
 * Subscribe to live EngineState updates pushed by the daemon.
 * Returns an unlisten function to clean up the subscription.
 */
export const onStateChanged = (cb: (s: EngineState) => void): Promise<UnlistenFn> =>
  listen<EngineState>("state-changed", (e) => cb(e.payload));
