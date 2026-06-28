<script lang="ts">
  import { Popover } from "bits-ui";
  import type { ChannelSnapshot, AppStream, OutputDeviceSnapshot } from "../ipc.js";
  import { setChannelOutput, setChannelVolume, setChannelMute } from "../ipc.js";
  import {
    buildDeviceOptions,
    toSelectOptions,
    outputToSelectValue,
    selectValueToOutput,
    toErrorMsg,
  } from "./channelStripUtils.js";
  import { engineState } from "../stores.js";
  import { currentPage } from "../stores/page.js";
  import LevelMeter from "./LevelMeter.svelte";
  import AppPill from "./AppPill.svelte";
  import VolumeSlider from "./VolumeSlider.svelte";
  import Select from "../ui/Select.svelte";

  interface Props {
    channel: ChannelSnapshot;
    /** App streams currently assigned to this channel. */
    streams?: AppStream[];
    /** Real output device list fetched from the daemon via `list_outputs`. */
    outputDevices?: OutputDeviceSnapshot[];
    /** Called when a stream is dropped onto this channel strip. */
    onDropStream?: (streamId: string, channelId: string) => void;
    /** Called after output device change so parent can refresh state if needed. */
    onOutputChanged?: () => void;
    /**
     * When provided the strip shows a remove button. Should be undefined for
     * fixed channels (e.g. last remaining channel) to prevent removal.
     */
    onRemove?: () => void;
    /**
     * Called whenever a channel write (volume / mute / output) fails.
     * Receives the error message so the parent can surface it (e.g. in an
     * existing error banner). The optimistic revert still runs regardless.
     */
    onError?: (msg: string) => void;
  }

  let { channel, streams = [], outputDevices = [], onDropStream = () => {}, onOutputChanged, onRemove, onError = () => {} }: Props = $props();

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

  function accentFor(id: string): string {
    const map: Record<string, string> = {
      game:  "var(--ss-accent-game)",
      chat:  "var(--ss-accent-chat)",
      media: "var(--ss-accent-media)",
      aux:   "var(--ss-accent-aux)",
    };
    return map[id] ?? "var(--ss-accent)";
  }

  // -----------------------------------------------------------------------
  // Output device popover state
  // -----------------------------------------------------------------------
  let deviceOptions = $derived(buildDeviceOptions(channel, outputDevices));
  let selectOptions = $derived(toSelectOptions(deviceOptions));
  let changing = $state(false);
  let popoverOpen = $state(false);

  async function handleDeviceChange(value: string | null) {
    if (changing) return;
    changing = true;
    try {
      const newState = await setChannelOutput(channel.id, value);
      if (newState) {
        engineState.set(newState);
      }
      onOutputChanged?.();
    } catch (err) {
      const msg = toErrorMsg(err);
      console.error("[ChannelStrip] setChannelOutput failed:", err);
      onError(msg);
    } finally {
      changing = false;
    }
  }

  // -----------------------------------------------------------------------
  // Volume — commit via VolumeSlider (rejections propagate to its onError)
  // -----------------------------------------------------------------------
  async function handleVolumeCommit(pct: number) {
    const s = await setChannelVolume(channel.id, pct);
    if (s) engineState.set(s);
  }

  // -----------------------------------------------------------------------
  // Mute handler
  // -----------------------------------------------------------------------
  let muteBusy = $state(false);

  async function handleMuteToggle() {
    if (muteBusy) return;
    muteBusy = true;
    try {
      const newState = await setChannelMute(channel.id, !channel.muted);
      if (newState) {
        engineState.set(newState);
      }
    } catch (err) {
      const msg = toErrorMsg(err);
      console.error("[ChannelStrip] setChannelMute failed:", err);
      onError(msg);
    } finally {
      muteBusy = false;
    }
  }

  // -----------------------------------------------------------------------
  // EQ navigation
  // -----------------------------------------------------------------------
  function openEq() {
    sessionStorage.setItem("eq:channel", channel.id);
    currentPage.set("eq");
  }

  // -----------------------------------------------------------------------
  // Derived labels / IDs
  // -----------------------------------------------------------------------
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
  function handleDragLeave(e: DragEvent) {
    const related = e.relatedTarget as Node | null;
    if (related && e.currentTarget instanceof Node && e.currentTarget.contains(related)) {
      return;
    }
    dragOver = false;
  }
  function handleDrop(e: DragEvent) {
    e.preventDefault();
    dragOver = false;
    const id = e.dataTransfer?.getData("text/asm-stream-id");
    if (id) onDropStream(id, channel.id);
  }
</script>

<article
  class="channel-strip"
  aria-labelledby={labelId}
  data-channel-id={channel.id}
  style="--accent: {accentFor(channel.id)}"
>
  <!-- ===== Channel header ===== -->
  <div class="strip-header">
    <span class="channel-icon" aria-hidden="true">{getIcon(channel.id)}</span>
    <h3 class="channel-label" id={labelId}>{channel.id.toUpperCase()}</h3>
  </div>

  <p class="channel-node-name" title={displayName}>{displayName}</p>

  <!-- ===== Action row: gear (output popover trigger) + EQ + remove ===== -->
  <div class="strip-actions">
    <!-- Gear / output-device popover -->
    <Popover.Root bind:open={popoverOpen}>
      <Popover.Trigger
        class="action-btn gear-btn"
        aria-label="Output device for {channel.id.toUpperCase()}"
        aria-haspopup="true"
        aria-expanded={popoverOpen}
      >⚙</Popover.Trigger>
      <Popover.Portal>
        <Popover.Content class="output-popover-content" sideOffset={8}>
          <div class="popover-inner" class:changing>
            <label class="popover-label" for={selectId}>Playback device</label>
            <Select
              options={selectOptions}
              value={outputToSelectValue(channel.output_device)}
              onValueChange={(v) => handleDeviceChange(selectValueToOutput(v))}
              disabled={changing}
              ariaLabel="Output device for {channel.id.toUpperCase()}"
              id={selectId}
            />
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>

    <!-- EQ button -->
    <button
      class="action-btn eq-btn"
      onclick={openEq}
      aria-label="Open EQ for {channel.id.toUpperCase()}"
      title="{eqBandCount === 0 ? 'flat' : `${eqBandCount} band${eqBandCount !== 1 ? 's' : ''}`}"
    >EQ</button>

    <!-- Remove button — only when caller allows removal -->
    {#if onRemove}
      <button
        class="action-btn remove-btn"
        title="Remove channel {channel.id.toUpperCase()}"
        aria-label="Remove channel {channel.id.toUpperCase()}"
        onclick={onRemove}
      >×</button>
    {/if}
  </div>

  <!-- ===== Volume area: slider + level meter ===== -->
  <div class="volume-area">
    <VolumeSlider
      volume={channel.volume_pct}
      accent={accentFor(channel.id)}
      label="Volume for {channel.id.toUpperCase()}"
      oncommit={handleVolumeCommit}
      onError={onError}
    />
    <LevelMeter
      nodeName={channel.node_name}
      orientation="vertical"
      ariaLabel="Volume level for {channel.id.toUpperCase()}"
    />
  </div>

  <!-- ===== Mute toggle ===== -->
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
  /* ===== Channel strip container ===== */
  .channel-strip {
    display: flex;
    flex-direction: column;
    width: var(--ss-channel-strip-w);
    min-width: var(--ss-channel-strip-w-min);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border);
    border-top: 2px solid var(--accent, var(--ss-accent));
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

  /* ===== Action row ===== */
  .strip-actions {
    display: flex;
    align-items: center;
    gap: var(--ss-space-1);
  }

  /* Shared icon-button base for gear / eq / remove */
  :global(.action-btn) {
    display: flex;
    align-items: center;
    justify-content: center;
    height: var(--ss-control-h-sm);
    min-width: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-2);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-secondary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    font-weight: var(--ss-type-button-weight);
    letter-spacing: var(--ss-type-button-letter-spacing);
    text-transform: uppercase;
    cursor: pointer;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  :global(.action-btn:hover) {
    border-color: var(--ss-border-strong);
    background: var(--ss-surface-2);
    color: var(--ss-text-primary);
  }

  :global(.action-btn:focus-visible) {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  :global(.action-btn:disabled) {
    cursor: not-allowed;
    opacity: 0.5;
  }

  /* Gear button: accent on open state */
  :global(.gear-btn[aria-expanded="true"]) {
    border-color: var(--ss-accent-border);
    color: var(--ss-accent);
    background: var(--ss-accent-soft);
  }

  /* Remove button: danger tint on hover */
  .remove-btn {
    margin-left: auto;
  }

  .remove-btn:hover {
    color: var(--ss-danger) !important;
    background: var(--ss-danger-soft) !important;
    border-color: var(--ss-danger) !important;
  }

  /* ===== Output device popover content (portalled — :global required) ===== */
  :global(.output-popover-content) {
    background: var(--ss-surface-3);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-sm);
    box-shadow: var(--ss-e2);
    z-index: 50;
    min-width: 200px;
    max-width: 280px;
  }

  .popover-inner {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-2);
    padding: var(--ss-space-3);
    transition: opacity var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .popover-inner.changing {
    opacity: 0.6;
    pointer-events: none;
  }

  .popover-label {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-tertiary);
  }

  /* ===== Volume area ===== */
  .volume-area {
    display: flex;
    flex-direction: row;
    align-items: stretch;
    justify-content: center;
    gap: var(--ss-space-2);
    flex: 1;
    min-height: 120px;
  }

  /* ===== Mute button ===== */
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
    align-self: center;
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

  /* ===== App stream drop area ===== */
  .strip-apps {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-1);
    min-height: 48px;
    padding: var(--ss-space-2);
    border: var(--ss-border-width) dashed var(--ss-border);
    border-radius: var(--ss-radius-sm);
    transition: background var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .strip-apps.drag-over {
    background: var(--ss-drag-highlight);
    border-color: var(--ss-accent);
  }
</style>
