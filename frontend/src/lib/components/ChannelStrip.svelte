<script lang="ts">
  import type { ChannelSnapshot } from "../ipc.js";
  import { setChannelOutput } from "../ipc.js";
  import { engineState } from "../stores.js";
  import { currentPage } from "../stores/page.js";

  interface Props {
    channel: ChannelSnapshot;
    /** Called after output device change so parent can refresh state if needed. */
    onOutputChanged?: () => void;
  }

  let { channel, onOutputChanged }: Props = $props();

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

  function openEq() {
    // Store the target channel id in sessionStorage so the EQ page can read it.
    sessionStorage.setItem("eq:channel", channel.id);
    currentPage.set("eq");
  }

  const eqBandCount = $derived(channel.eq_bands.length);
  const displayName = $derived(channel.node_name || channel.id.toUpperCase());
  const labelId = $derived(`channel-label-${channel.id}`);
  const selectId = $derived(`channel-output-${channel.id}`);
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
  </div>

  <p class="channel-node-name" title={displayName}>{displayName}</p>

  <!-- ===== Vertical placeholder for volume slot (disabled) ===== -->
  <div
    class="volume-slot"
    title="Volume and mute are not yet supported by the engine"
    aria-label="Volume control — not supported"
  >
    <div class="fader-track" aria-hidden="true">
      <div class="fader-fill" style="height: 70%"></div>
      <div class="fader-thumb" style="bottom: calc(70% - 8px)"></div>
    </div>
    <div class="fader-readout" aria-hidden="true">— dB</div>

    <button
      class="mute-btn"
      disabled
      title="Mute not supported by the engine"
      aria-label="Mute ({channel.id.toUpperCase()}) — not supported"
    >
      <span aria-hidden="true">🔇</span>
    </button>
    <span class="disabled-hint">Volume/mute not yet supported by the engine</span>
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

  /* ===== Volume slot (disabled / placeholder) ===== */
  .volume-slot {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--ss-space-2);
    flex: 1;
    min-height: 100px;
    position: relative;
  }

  .fader-track {
    position: relative;
    width: 4px;
    height: 80px;
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-pill);
    flex-shrink: 0;
    opacity: 0.35;
  }

  .fader-fill {
    position: absolute;
    bottom: 0;
    left: 0;
    width: 100%;
    background: var(--ss-text-disabled);
    border-radius: var(--ss-radius-pill);
  }

  .fader-thumb {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    width: 16px;
    height: 16px;
    background: var(--ss-text-disabled);
    border-radius: var(--ss-radius-pill);
    box-shadow: var(--ss-e1);
  }

  .fader-readout {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-readout-size);
    font-variant-numeric: tabular-nums;
    color: var(--ss-text-disabled);
    opacity: 0.5;
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
    cursor: not-allowed;
    opacity: 0.35;
    font-size: 13px;
  }

  .disabled-hint {
    font-family: var(--ss-font-ui);
    font-size: 9px;
    font-weight: 400;
    line-height: 1.3;
    color: var(--ss-text-disabled);
    text-align: center;
    letter-spacing: 0.01em;
    display: none; /* shown on :hover of the slot */
  }

  .volume-slot:hover .disabled-hint {
    display: block;
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
</style>
