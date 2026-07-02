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

export interface AppStream {
  id: number;
  binary: string;
  app_name: string;
  pid: number | null;
  icon_name: string | null;
  media_name: string | null;
  current_channel: string | null;
  routed: boolean;
}

/** Mirror of crates/engine/src/commands.rs OutputDeviceSnapshot. */
export interface OutputDeviceSnapshot {
  node_name: string;
  description: string;
  is_default: boolean;
}

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
  /** Software volume 0–100 percent. 100 = unity gain. */
  volume_pct: number;
  /** Whether the channel is muted. */
  muted: boolean;
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
  /** Mic source volume, percent 0–100. 100 = unity. */
  volume_pct: number;
  stages: MicStageSnapshot[];
  eq_bands: EqBandSnapshot[]; // reuse existing EqBandSnapshot
  /** Active suppression backend: "deep_filter" | "rnnoise" */
  suppression_backend: string;
  /** Backends whose plugin is present on this machine */
  available_suppression_backends: string[];
  /** Pinned hardware mic capture source; null = auto / not pinned. */
  hw_mic?: string | null;
}

/** Mirror of crates/engine/src/state.rs HrirEntrySnapshot. */
export interface HrirEntrySnapshot {
  stem: string;
  display: string;
  group: string;
  tonality: string;
}

export interface SurroundSnapshot {
  enabled: boolean;
  hrir: string | null;
  available_hrirs: string[];
  /** Rich per-entry metadata; empty when the engine is older (serde default = []). */
  available_hrir_entries?: HrirEntrySnapshot[];
  channels: string[];
  hw_sink: string | null;
  /** Configured mode as lowercase string, e.g. "auto" | "hrir71" | "hrir51" | "stereo_bypass". Absent on older engines. */
  mode?: string;
  /** Resolved effective mode after fallback, e.g. "hrir71". Absent on older engines. */
  effective_mode?: string;
  /** Hardware-negotiated channel count from pw-dump probe, or null if not yet probed. */
  negotiated_channels?: number | null;
  /** Whether the negotiated surround input has rear/side channels (true 7.1/5.1) vs stereo. null = no source. */
  negotiated_surround?: boolean | null;
  /** Pinned HRIR stem requested but not installed (fallback in use); null/absent when OK. */
  hrir_missing?: string | null;
  /** Pinned convolver blocksize, or null/absent for PipeWire default. */
  blocksize?: number | null;
}

export interface EqPresetSnapshot {
  name: string;
  band_count: number;
}

export interface MicPresetSnapshot {
  name: string;
  description: string;
}

/** Mirror of crates/engine/src/factory_profiles.rs FactoryProfileInfo. */
export interface FactoryProfileInfo {
  name: string;
  hrir: string | null;
  mode: string;
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
  eq_presets: EqPresetSnapshot[];
  master_volume_pct: number;
  factory_eq_presets: EqPresetSnapshot[];
  mic_presets: MicPresetSnapshot[];
  master_mute: boolean;
  chatmix_position: number;
  default_sink_channel: string | null;
  /** When true the hardware dial owns ChatMix; GUI slider is read-only. */
  dial_controls_balance: boolean;
  /** When true the base-station volume knob mirrors into master_volume_pct (read-only). */
  knob_controls_master: boolean;
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

/** Builds the camelCase arg object for set_channel_volume. */
export function buildSetChannelVolumeArgs(
  channel: string,
  volume_pct: number,
): { channel: string; volumePct: number } {
  return { channel, volumePct: volume_pct };
}

/** Builds the arg object for set_channel_mute. */
export function buildSetChannelMuteArgs(
  channel: string,
  muted: boolean,
): { channel: string; muted: boolean } {
  return { channel, muted };
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

/** Builds arg object for profile_rename. Old/new are both simple strings — no rename needed. */
export function buildProfileRenameArgs(
  old_name: string,
  new_name: string,
): { old: string; new: string } {
  return { old: old_name, new: new_name };
}

/** Builds arg object for eq_preset_save. */
export function buildEqPresetSaveArgs(
  name: string,
  channel: string,
): { name: string; channel: string } {
  return { name, channel };
}

/** Builds arg object for eq_preset_apply. */
export function buildEqPresetApplyArgs(
  preset: string,
  channel: string,
): { preset: string; channel: string } {
  return { preset, channel };
}

/** Builds arg object for channel_add (F4). */
export function buildChannelAddArgs(id: string): { id: string } {
  return { id };
}

/** Builds arg object for channel_remove (F4). */
export function buildChannelRemoveArgs(id: string): { id: string } {
  return { id };
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
 * Create a factory profile from a named template and make it the active profile.
 * Supported templates: `"DayZ"` (game surround on, footstep EQ, default sink = game).
 * Returns the updated EngineState.
 */
export const profileCreateFromFactory = (template: string): Promise<EngineState> =>
  invoke<EngineState>("profile_create_from_factory", { template });

/**
 * Set (or clear) the output device for a channel.
 * Returns the updated EngineState so the caller can apply it to the store
 * immediately for snappy feedback (no need to wait for the state-changed event).
 */
export const setChannelOutput = (channel: string, device: string | null): Promise<EngineState> =>
  invoke<EngineState>("set_channel_output", buildSetChannelOutputArgs(channel, device));

/**
 * Set the software volume (percent 0–100) for a channel. 100 = unity gain.
 * Returns the updated EngineState for immediate store application.
 */
export const setChannelVolume = (channel: string, volumePct: number): Promise<EngineState> =>
  invoke<EngineState>("set_channel_volume", buildSetChannelVolumeArgs(channel, volumePct));

/**
 * Set the mute state for a channel.
 * Returns the updated EngineState for immediate store application.
 */
export const setChannelMute = (channel: string, muted: boolean): Promise<EngineState> =>
  invoke<EngineState>("set_channel_mute", buildSetChannelMuteArgs(channel, muted));

/**
 * Ack-only twin of {@link setChannelVolume} for intermediate drag ticks: the
 * daemon still applies the volume, but the EngineState echo is discarded in
 * Rust before it can cross the webview bridge as JSON. Use setChannelVolume
 * for the commit/release call so state convergence still happens.
 */
export const setChannelVolumeAck = (channel: string, volumePct: number): Promise<void> =>
  invoke("set_channel_volume_ack", buildSetChannelVolumeArgs(channel, volumePct));

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

/**
 * Ack-only twin of {@link setEqBand} for intermediate drag ticks (~20/s):
 * applies the band but discards the EngineState echo Rust-side. Use setEqBand
 * for the release/commit flush so convergence still happens.
 */
export const setEqBandAck = (
  channel: string,
  band: number,
  kind: string,
  freq_hz: number,
  q: number,
  gain_db: number,
): Promise<void> =>
  invoke("set_eq_band_ack", buildSetEqBandArgs(channel, band, kind, freq_hz, q, gain_db));

/**
 * Set the FULL EQ band set for a channel in ONE batch call; returns updated
 * EngineState. The daemon resolves the sink node once and applies every band,
 * so bulk edits (Flatten / tone curves) settle instantly instead of band-by-band.
 * Single-band live drags keep using {@link setEqBand}.
 */
export const setChannelEq = (channel: string, bands: EqBandSnapshot[]): Promise<EngineState> =>
  invoke<EngineState>("set_channel_eq", { channel, bands });

/** Route an app binary to a target sink; returns updated EngineState. */
export const setRoute = (app_binary: string, target_sink: string): Promise<EngineState> =>
  invoke<EngineState>("set_route", buildSetRouteArgs(app_binary, target_sink));

/** Remove the routing rule for an app binary; returns updated EngineState. */
export const clearRoute = (app_binary: string): Promise<EngineState> =>
  invoke<EngineState>("clear_route", { appBinary: app_binary });

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

/** List the static factory-profile catalog for the data-driven create-profile UI. */
export const listFactoryProfiles = (): Promise<FactoryProfileInfo[]> =>
  invoke<FactoryProfileInfo[]>("list_factory_profiles");

/** Pin (or clear) the convolver blocksize. null = PipeWire default. */
export const surroundSetBlocksize = (blocksize: number | null): Promise<EngineState> =>
  invoke<EngineState>("surround_set_blocksize", { blocksize });

// ── A6: HRIR import / fetch commands ─────────────────────────────────────────

/** Import HeSuVi 14-channel WAVs from `dir` (null = use default path) into the HRIR profiles dir. */
export const surroundImportHrirs = (dir: string | null): Promise<EngineState> =>
  invoke<EngineState>("surround_import_hrirs", { dir });

/** Placeholder: automatic HeSuVi download (not yet available — returns an error from the daemon). */
export const surroundFetchHrirs = (): Promise<EngineState> =>
  invoke<EngineState>("surround_fetch_hrirs", {});

// ── F3b: Profile management commands ─────────────────────────────────────────

/** Rename a profile. Returns updated EngineState. */
export const profileRename = (oldName: string, newName: string): Promise<EngineState> =>
  invoke<EngineState>("profile_rename", buildProfileRenameArgs(oldName, newName));

/** Delete a profile (cannot delete active or last). Returns updated EngineState. */
export const profileDelete = (name: string): Promise<EngineState> =>
  invoke<EngineState>("profile_delete", { name });

/**
 * Export a profile as a TOML string.
 * Returns the raw TOML text, NOT an EngineState — use to trigger a download or clipboard copy.
 */
export const profileExport = (name: string): Promise<string> =>
  invoke<string>("profile_export", { name });

/** Import a profile from a TOML string. Resolves name collisions automatically. */
export const profileImport = (toml: string): Promise<EngineState> =>
  invoke<EngineState>("profile_import", { toml });

// ── F3b: EQ preset commands ───────────────────────────────────────────────────

/** Save the current EQ bands of a channel as a named preset. */
export const eqPresetSave = (name: string, channel: string): Promise<EngineState> =>
  invoke<EngineState>("eq_preset_save", buildEqPresetSaveArgs(name, channel));

/** Apply a named EQ preset to a channel's EQ bands. */
export const eqPresetApply = (preset: string, channel: string): Promise<EngineState> =>
  invoke<EngineState>("eq_preset_apply", buildEqPresetApplyArgs(preset, channel));

/** Apply a named mic preset. */
export const micPresetApply = (name: string): Promise<EngineState> =>
  invoke<EngineState>("mic_preset_apply", { name });

/** Delete a named EQ preset. */
export const eqPresetDelete = (name: string): Promise<EngineState> =>
  invoke<EngineState>("eq_preset_delete", { name });

// ── F4: Channel add / remove commands ────────────────────────────────────────

/** Add a new channel by id. Returns updated EngineState. */
export const channelAdd = (id: string): Promise<EngineState> =>
  invoke<EngineState>("channel_add", buildChannelAddArgs(id));

/** Remove a channel by id. Returns updated EngineState. */
export const channelRemove = (id: string): Promise<EngineState> =>
  invoke<EngineState>("channel_remove", buildChannelRemoveArgs(id));

// ── Task 12: Sonar mixer commands ────────────────────────────────────────────

/** List all active PipeWire app streams. */
export const listStreams = (): Promise<AppStream[]> => invoke<AppStream[]>("list_streams");

/** List all available PipeWire output devices (sinks). */
export const listOutputs = (): Promise<OutputDeviceSnapshot[]> => invoke<OutputDeviceSnapshot[]>("list_outputs");

/** Move an app stream to a channel (or null to unroute). */
export const moveStream = (stream: string, channel: string): Promise<EngineState> =>
  invoke<EngineState>("move_stream", { stream, channel });

/** Set the master volume (percent 0–100). 100 = unity. */
export const setMasterVolume = (volumePct: number): Promise<EngineState> =>
  invoke<EngineState>("set_master_volume", { volumePct });

/** Set the mic source volume (percent 0–100). 100 = unity. */
export const setMicVolume = (volumePct: number): Promise<EngineState> =>
  invoke<EngineState>("set_mic_volume", { volumePct });

/** Set the master mute state. */
export const setMasterMute = (muted: boolean): Promise<EngineState> =>
  invoke<EngineState>("set_master_mute", { muted });

/** Set the ChatMix position (integer 0–9; 0 = full chat, 9 = full game). */
export const setChatmix = (position: number): Promise<EngineState> =>
  invoke<EngineState>("set_chatmix", { position });

/** Set (or clear) the default sink channel for unrouted streams. */
export const setDefaultSinkChannel = (channel: string | null): Promise<EngineState> =>
  invoke<EngineState>("set_default_sink_channel", { channel });

// ── R2: Coexistence teardown types + commands ────────────────────────────────

export interface CoexistReport {
  legacy_loopbacks: string[];
  hrir_switch_present: boolean;
  rpm_daemon_running: boolean;
  /** True when any legacy component was detected. */
  any_detected: boolean;
}

export interface CoexistActionResult {
  description: string;
  ok: boolean;
  error: string | null;
}

export interface CoexistDisableResult {
  dry_run: boolean;
  actions_attempted: number;
  successes: number;
  failures: CoexistActionResult[];
  all_ok: boolean;
  /** Human note: owner must `sudo dnf remove arctis-sound-manager` manually. */
  owner_note: string;
}

/**
 * Detect the legacy arctis-sound-manager RPM stack.
 * Returns a report indicating what legacy components are present.
 */
export const coexistStatus = (): Promise<CoexistReport> =>
  invoke<CoexistReport>("coexist_status");

/**
 * Disable the legacy arctis-sound-manager RPM stack.
 * Stops and disables user services; destroys live loopback nodes.
 * Pass dryRun=true to preview without making changes.
 */
export const coexistDisable = (dryRun: boolean = false): Promise<CoexistDisableResult> =>
  invoke<CoexistDisableResult>("coexist_disable", { dryRun });

// ── Daemon lifecycle types + commands ────────────────────────────────────────

/** Maps to the `ManagedBy` serde-snake_case enum in daemon_control.rs. */
export type ManagedBy = "systemd" | "manual" | "stopped";

/** Mirror of `DaemonStatus` in src-tauri/src/daemon_control.rs. */
export interface DaemonStatus {
  running: boolean;
  managed_by: ManagedBy;
  autostart_enabled: boolean;
  systemd_available: boolean;
  binary_path: string | null;
  unit_installed: boolean;
}

/** Query the daemon's current lifecycle status without side-effects. */
export const daemonStatus = (): Promise<DaemonStatus> =>
  invoke<DaemonStatus>("daemon_status");

/** Start the daemon (via systemd or direct spawn). Returns updated status. */
export const daemonStart = (): Promise<DaemonStatus> =>
  invoke<DaemonStatus>("daemon_start");

/** Stop the daemon (via systemd or IPC shutdown). Returns updated status. */
export const daemonStop = (): Promise<DaemonStatus> =>
  invoke<DaemonStatus>("daemon_stop");

/** Restart the daemon. Returns updated status. */
export const daemonRestart = (): Promise<DaemonStatus> =>
  invoke<DaemonStatus>("daemon_restart");

/**
 * Enable or disable daemon autostart via a systemd user unit.
 * `enabled=true` installs the unit and runs `systemctl --user enable --now`;
 * `enabled=false` runs `systemctl --user disable --now`.
 * Returns updated status.
 */
export const daemonSetAutostart = (enabled: boolean): Promise<DaemonStatus> =>
  invoke<DaemonStatus>("daemon_set_autostart", { enabled });

/** Enable/disable the GUI's own login autostart (launches hidden into the tray).
 *  Distinct from daemonSetAutostart, which manages the engine's systemd unit.
 *  Returns the new enabled state as confirmed by the plugin. */
export const guiSetAutostart = (enabled: boolean): Promise<boolean> =>
  invoke<boolean>("gui_set_autostart", { enabled });

/** Query whether the GUI's own login autostart entry is currently enabled. */
export const guiAutostartEnabled = (): Promise<boolean> =>
  invoke<boolean>("gui_autostart_enabled");

// ---------------------------------------------------------------------------
// Event subscriptions
// ---------------------------------------------------------------------------

/**
 * Subscribe to live EngineState updates pushed by the daemon.
 * Returns an unlisten function to clean up the subscription.
 */
export const onStateChanged = (cb: (s: EngineState) => void): Promise<UnlistenFn> =>
  listen<EngineState>("state-changed", (e) => cb(e.payload));

/**
 * Subscribe to the `daemon-down` event, emitted ONCE by the Rust state poll
 * when a daemon request fails after previously succeeding (edge-triggered).
 * The payload is a human-readable error message. Recovery needs no event:
 * the poll re-emits `state-changed` as soon as the daemon answers again.
 */
export const onDaemonDown = (cb: (msg: string) => void): Promise<UnlistenFn> =>
  listen<string>("daemon-down", (e) => cb(e.payload));

// ---------------------------------------------------------------------------
// R3: Level-meter event (levels)
// ---------------------------------------------------------------------------

/**
 * Payload of the `levels` Tauri event emitted by the src-tauri metering task.
 *
 * Keys are PipeWire `node.name` strings for the Arctis virtual sinks and the
 * clean-mic source (e.g. "Arctis_Game", "Arctis_Chat", "Arctis_Media",
 * "arctis_clean_mic").  Values are real-time PCM signal peaks in [0.0, 1.0].
 *
 * These are true signal peaks captured via a short pw-record capture stream
 * per node, sampled at ~25 Hz.  They reflect actual audio activity, not the
 * configured software volume.
 */
export type LevelsPayload = Record<string, number>;

/**
 * Subscribe to live level updates from the metering task.
 * Emitted every ~2 s; the payload maps node_name → linear volume [0, 1].
 * Returns an unlisten function to clean up the subscription.
 */
export const onLevels = (cb: (levels: LevelsPayload) => void): Promise<UnlistenFn> =>
  listen<LevelsPayload>("levels", (e) => cb(e.payload));

/**
 * Register interest in the `levels` event. The Rust meter task only emits while
 * the subscriber count is > 0, so the UI must call this when a meter mounts and
 * {@link meterUnsubscribe} when it unmounts. Idempotency/counting is handled by
 * the shared subscription in LevelMeter.svelte — call once per "first mount".
 */
export const meterSubscribe = (): Promise<void> => invoke("meter_subscribe");

/** Release interest in the `levels` event (decrements the Rust subscriber count). */
export const meterUnsubscribe = (): Promise<void> => invoke("meter_unsubscribe");

/**
 * Re-exec the app after a successful update. Required on Linux: the updater
 * replaces the AppImage in place but does NOT restart the running process
 * (only Windows restarts automatically). Never resolves on success.
 */
export const relaunchApp = (): Promise<void> => invoke("relaunch_app");

/**
 * Subscribe to live AppStream list updates pushed by the daemon.
 * Emitted whenever PipeWire app streams change (appear/disappear/move).
 * Returns an unlisten function to clean up the subscription.
 */
export const onStreamsChanged = (cb: (s: AppStream[]) => void): Promise<UnlistenFn> =>
  listen<AppStream[]>("streams-changed", (e) => cb(e.payload));
