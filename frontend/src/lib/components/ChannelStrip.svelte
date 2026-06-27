<script lang="ts">
  import type { ChannelSnapshot, AppStream } from "../ipc.js";
  import { setChannelOutput, setChannelVolume, setChannelMute } from "../ipc.js";
  import { engineState } from "../stores.js";
  import { currentPage } from "../stores/page.js";
  import LevelMeter from "./LevelMeter.svelte";
  import AppPill from "./AppPill.svelte";

  // Domain bounds — mirror crates/domain/src/eq_bounds.rs CHANNEL_VOLUME_MIN/MAX_DB
  const VOLUME_MIN_DB = -60;
  const VOLUME_MAX_DB = 6;

  interface Props {
    channel: ChannelSnapshot;
    /** App streams currently assigned to this channel. */
    streams?: AppStream[];
    /** Called when a stream is dropped onto this channel strip. */
    onDropStream?: (streamId: string, channelId: string) => void;
    /** Called after output device change so parent can refresh state if needed. */
    onOutputChanged?: () => void;
    /**
     * When provided the strip shows a remove button. Should be undefined for
     * fixed channels (e.g. last remaining channel) to prevent removal.
     */
    onRemove?: () => void;
  }

  let { channel, streams = [], onDropStream = () => {}, onOutputChanged, onRemove }: Props = $props();

  // -----------------------------------------------------------------------
  // Channel identity / icon mapping
  // -----------------------------------------------------------------------
  const CHANNEL_ICONS: Record<string, string> = {
    game:  "🎮",
    chat:  "💬",
    media: "♪",
    aux:   "⊕",
    mic:   "⏺",
    master:"◉",
  };

  function getIcon(id: string): string {
    return CHANNEL_ICONS[id.toLowerCase()] ?? "◈";
  }

  // -----------------------------------------------------------------------
  // Output device selector state
  // -----------------------------------------------------------------------
  /**
   * Known device options: "default" + any named device in the current output.
   * If the engine returns more devices via device_fields in future, this list
   * can be extended; for now we keep "Default" + the currently-set device.
   */
  function buildDeviceOptions(ch: ChannelSnapshot): { value: string | null; label: string }[] {
    const opts: { value: string | null; label: string }[] = [
      { value: null, label: "Default (follow system)" },
    ];
    if (ch.output_device && ch.output_device !== "default") {
      opts.push({ value: ch.output_device, label: ch.output_device });
    }
    return opts;
  }

  let deviceOptions = $derived(buildDeviceOptions(channel));
  let selectedDevice = $derived(channel.output_device);
  let changing = $state(false);

  async function handleDeviceChange(e: Event) {
    const select = e.target as HTMLSelectElement;
    const value = select.value === "__default__" ? null : select.value;
    if (changing) return;
    changing = true;
    try {
      const newState = await setChannelOutput(channel.id, value);
      // Apply the fresh state immediately for snappy feedback; the
      // state-changed event from the daemon will arrive shortly and
      // corroborate (or correct) this optimistic update.
      if (newState) {
        engineState.set(newState);
      }
      onOutputChanged?.();
    } catch (err) {
      console.error("[ChannelStrip] setChannelOutput failed:", err);
      // Revert the select back to reflect actual state
      select.value = selectedDevice ?? "__default__";
    } finally {
      changing = false;
    }
  }

  // -----------------------------------------------------------------------
  // Volume / mute handlers
  // -----------------------------------------------------------------------
  let volumeBusy = $state(false);
  let muteBusy = $state(false);

  /** Format a dB value for the readout: "0.0 dB", "-60 dB", "+6.0 dB". */
  function formatDb(db: number): string {
    const fixed = db.toFixed(1);
    return db > 0 ? `+${fixed} dB` : `${fixed} dB`;
  }

  /** Translate slider value (number) → slider position % for the fader fill. */
  function sliderPercent(db: number): number {
    const pct = ((db - VOLUME_MIN_DB) / (VOLUME_MAX_DB - VOLUME_MIN_DB)) * 100;
    return Math.max(0, Math.min(100, pct));
  }

  async function handleVolumeChange(e: Event) {
    const input = e.target as HTMLInputElement;
    const db = parseFloat(input.value);
    if (volumeBusy || isNaN(db)) return;
    volumeBusy = true;
    try {
      const newState = await setChannelVolume(channel.id, db);
      if (newState) {
        engineState.set(newState);
      }
    } catch (err) {
      console.error("[ChannelStrip] setChannelVolume failed:", err);
      // Revert input to reflect actual state
      input.value = String(channel.volume_db);
    } finally {
      volumeBusy = false;
    }
  }

  async function handleMuteToggle() {
    if (muteBusy) return;
    muteBusy = true;
    try {
      const newState = await setChannelMute(channel.id, !channel.muted);
      if (newState) {
        engineState.set(newState);
      }
    } catch (err) {
      console.error("[ChannelStrip] setChannelMute failed:", err);
    } finally {
      muteBusy = false;
    }
  }

  function openEq() {
    // Store the target channel id in sessionStorage so the EQ page can read it.
    sessionStorage.setItem("eq:channel", channel.id);
    currentPage.set("eq");
  }

  const eqBandCount = $derived(channel.eq_bands.length);
  const displayName = $derived(channel.node_name || channel.id.toUpperCase());
  const labelId = $derived(`channel-label-${channel.id}`);
  const selectId = $derived(`channel-output-${channel.id}`);

  // -----------------------------------------------------------------------
  // Drag-and-drop — app stream drop target
  // -----------------------------------------------------------------------
  let dragOver = $state(false);

  function handleDragOver(e: DragEvent) {
    if (e.dataTransfer?.types.includes("text/asm-stream-id")) {
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      dragOver = true;
    }
  }
  function handleDragLeave() { dragOver = false; }
  function handleDrop(e: DragEvent) {
    e.preventDefault();
    dragOver = false;
    const id = e.dataTransfer?.getData("text/asm-stream-id");
    if (id) onDropStream(id, channel.id);
  }

  function accentFor(id: string): string {
    const map: Record<string, string> = {
      game:  "var(--ss-accent-game)",
      chat:  "var(--ss-accent-chat)",
      media: "var(--ss-accent-media)",
      aux:   "var(--ss-accent-aux)",
    };
    return map[id] ?? "var(--ss-accent)";
  }
</script>

<article
  class="channel-strip"
  aria-labelledby={labelId}
  data-channel-id={channel.id}
>
  <!-- ===== Channel header ===== -->
  <div class="strip-header">
    <span class="channel-icon" aria-hidden="true">{getIcon(channel.id)}</span>
    <h3 class="channel-label" id={labelId}>{channel.id.toUpperCase()}</h3>
    {#if onRemove}
      <button
        class="remove-btn"
        title="Remove channel {channel.id.toUpperCase()}"
        aria-label="Remove channel {channel.id.toUpperCase()}"
        onclick={onRemove}
      >×</button>
    {/if}
  </div>

  <p class="channel-node-name" title={displayName}>{displayName}</p>

  <!-- ===== Volume slot ===== -->
  <div class="volume-slot" class:volume-busy={volumeBusy}>
    <!-- Custom fader visual (decorative — mirrors the range input value) -->
    <div class="fader-track" aria-hidden="true">
      <div class="fader-fill" style="height: {sliderPercent(channel.volume_db)}%"></div>
      <div class="fader-thumb" style="bottom: calc({sliderPercent(channel.volume_db)}% - 8px)"></div>
    </div>

    <!-- Accessible range input (the real interactive element) -->
    <input
      class="fader-input"
      type="range"
      min={VOLUME_MIN_DB}
      max={VOLUME_MAX_DB}
      step="0.5"
      value={channel.volume_db}
      disabled={volumeBusy}
      aria-label="Volume for {channel.id.toUpperCase()} ({formatDb(channel.volume_db)})"
      aria-valuemin={VOLUME_MIN_DB}
      aria-valuemax={VOLUME_MAX_DB}
      aria-valuenow={channel.volume_db}
      onchange={handleVolumeChange}
    />

    <div class="fader-readout">{formatDb(channel.volume_db)}</div>

    <!-- R3b: Level meter — shows real-time signal peak via levels event.
         Note: reflects real-time signal peak (from pw-record). See meter.ts. -->
    <LevelMeter
      nodeName={channel.node_name}
      orientation="vertical"
      ariaLabel="Volume level for {channel.id.toUpperCase()}"
    />

    <button
      class="mute-btn"
      class:muted={channel.muted}
      disabled={muteBusy}
      title={channel.muted ? "Unmute" : "Mute"}
      aria-label="{channel.muted ? 'Unmute' : 'Mute'} {channel.id.toUpperCase()}"
      aria-pressed={channel.muted}
      onclick={handleMuteToggle}
    >
      <span aria-hidden="true">{channel.muted ? "🔇" : "🔊"}</span>
    </button>
  </div>

  <!-- ===== Output device selector ===== -->
  <div class="output-section">
    <label class="output-label" for={selectId}>OUTPUT</label>
    <div class="select-wrapper" class:changing>
      <select
        id={selectId}
        class="output-select"
        value={selectedDevice ?? "__default__"}
        disabled={changing}
        aria-label="Output device for {channel.id.toUpperCase()}"
        onchange={handleDeviceChange}
      >
        {#each deviceOptions as opt}
          <option value={opt.value ?? "__default__"}>{opt.label}</option>
        {/each}
      </select>
      <span class="select-caret" aria-hidden="true">▾</span>
    </div>
  </div>

  <!-- ===== EQ summary + button ===== -->
  <div class="eq-section">
    <div class="eq-meta">
      <span class="eq-label">EQ</span>
      <span class="eq-count" aria-label="{eqBandCount} EQ bands configured">
        {eqBandCount === 0 ? "flat" : `${eqBandCount} band${eqBandCount !== 1 ? "s" : ""}`}
      </span>
    </div>
    <button
      class="eq-btn"
      onclick={openEq}
      aria-label="Open EQ for {channel.id.toUpperCase()}"
    >
      EQ
    </button>
  </div>

  <!-- ===== App stream drop area ===== -->
  <div
    class="strip-apps"
    class:drag-over={dragOver}
    role="list"
    aria-label="{channel.id} applications"
    ondragover={handleDragOver}
    ondragleave={handleDragLeave}
    ondrop={handleDrop}
  >
    {#each streams as s (s.id)}
      <AppPill stream={s} accent={accentFor(channel.id)} />
    {/each}
  </div>
</article>

<style>
  .channel-strip {
    display: flex;
    flex-direction: column;
    width: var(--ss-channel-strip-w);
    min-width: var(--ss-channel-strip-w-min);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    padding: var(--ss-space-3);
    gap: var(--ss-space-3);
    box-shadow: var(--ss-e1);
    flex-shrink: 0;
  }

  /* ===== Header ===== */
  .strip-header {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
  }

  .remove-btn {
    margin-left: auto;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    background: transparent;
    border: none;
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-tertiary);
    font-size: 14px;
    line-height: 1;
    cursor: pointer;
    padding: 0;
    transition: color var(--ss-dur-fast) var(--ss-ease-standard),
      background var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .remove-btn:hover {
    color: var(--ss-danger);
    background: var(--ss-danger-soft);
  }

  .remove-btn:focus-visible {
    outline: 2px solid var(--ss-danger);
    outline-offset: 2px;
  }

  .channel-icon {
    font-size: 14px;
    line-height: 1;
  }

  .channel-label {
    font-family: var(--ss-font-display);
    font-size: 13px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ss-text-primary);
    margin: 0;
    line-height: 1;
  }

  .channel-node-name {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    margin: 0;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* ===== Volume slot ===== */
  .volume-slot {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--ss-space-2);
    flex: 1;
    min-height: 100px;
    position: relative;
    transition: opacity var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .volume-slot.volume-busy {
    opacity: 0.6;
    pointer-events: none;
  }

  /* Decorative vertical track — overlaid behind the invisible range input */
  .fader-track {
    position: relative;
    width: 4px;
    height: 80px;
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-pill);
    flex-shrink: 0;
    pointer-events: none;
  }

  .fader-fill {
    position: absolute;
    bottom: 0;
    left: 0;
    width: 100%;
    background: var(--ss-accent);
    border-radius: var(--ss-radius-pill);
    transition: height var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .fader-thumb {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    width: 16px;
    height: 16px;
    background: var(--ss-accent);
    border-radius: var(--ss-radius-pill);
    box-shadow: var(--ss-e1);
    transition: bottom var(--ss-dur-fast) var(--ss-ease-standard);
    pointer-events: none;
  }

  /* Transparent range input positioned over the decorative track */
  .fader-input {
    position: absolute;
    /* Rotate so the range runs bottom-to-top along the fader track. */
    writing-mode: vertical-lr;
    direction: rtl;
    width: 80px;
    height: 24px;
    opacity: 0;
    cursor: pointer;
    margin-top: 0;
    z-index: 1;
  }

  .fader-input:disabled {
    cursor: not-allowed;
  }

  .fader-readout {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-readout-size);
    font-variant-numeric: tabular-nums;
    color: var(--ss-text-secondary);
  }

  .mute-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: var(--ss-control-h-sm);
    height: var(--ss-control-h-sm);
    border-radius: var(--ss-radius-xs);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border);
    cursor: pointer;
    font-size: 13px;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .mute-btn:hover {
    border-color: var(--ss-border-strong);
    background: var(--ss-surface-2);
  }

  .mute-btn.muted {
    background: var(--ss-accent);
    border-color: var(--ss-accent-border);
  }

  .mute-btn:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  .mute-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  /* ===== Output device selector ===== */
  .output-section {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-1);
  }

  .output-label {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-tertiary);
  }

  .select-wrapper {
    position: relative;
    display: flex;
    align-items: center;
  }

  .select-wrapper.changing {
    opacity: 0.6;
    pointer-events: none;
  }

  .output-select {
    width: 100%;
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-5) 0 var(--ss-space-2);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    cursor: pointer;
    appearance: none;
    -webkit-appearance: none;
    transition:
      border-color var(--ss-dur-fast) var(--ss-ease-standard),
      background var(--ss-dur-fast) var(--ss-ease-standard);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .output-select:hover {
    border-color: var(--ss-border-strong);
    background: var(--ss-surface-2);
  }

  .output-select:focus {
    outline: none;
    border-color: var(--ss-accent-border);
  }

  .output-select:disabled {
    cursor: not-allowed;
    color: var(--ss-text-disabled);
  }

  /* Style the native option elements for dark theme */
  .output-select option {
    background: var(--ss-surface-3);
    color: var(--ss-text-primary);
  }

  .select-caret {
    position: absolute;
    right: var(--ss-space-2);
    color: var(--ss-text-tertiary);
    font-size: 9px;
    pointer-events: none;
  }

  /* ===== EQ section ===== */
  .eq-section {
    display: flex;
    align-items: center;
    justify-content: space-between;
    border-top: var(--ss-border-width) solid var(--ss-border);
    padding-top: var(--ss-space-2);
  }

  .eq-meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .eq-label {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-tertiary);
  }

  .eq-count {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-secondary);
    font-variant-numeric: tabular-nums;
  }

  .eq-btn {
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-3);
    background: var(--ss-gradient-secondary);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-button-size);
    font-weight: var(--ss-type-button-weight);
    letter-spacing: var(--ss-type-button-letter-spacing);
    text-transform: uppercase;
    cursor: pointer;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .eq-btn:hover {
    background: var(--ss-surface-2);
    border-color: var(--ss-accent-border);
    color: var(--ss-accent);
  }

  .eq-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .eq-btn:active {
    background: var(--ss-surface-input);
  }

  /* ===== App stream drop area ===== */
  .strip-apps {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-1);
    min-height: 48px;
    padding: var(--ss-space-2);
    margin-top: var(--ss-space-2);
    border: var(--ss-border-width) dashed var(--ss-border);
    border-radius: var(--ss-radius-sm);
    transition: background var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .strip-apps.drag-over {
    background: var(--ss-drag-highlight);
    border-color: var(--ss-accent);
  }
</style>
