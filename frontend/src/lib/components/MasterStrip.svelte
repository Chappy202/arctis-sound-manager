<script lang="ts">
  import { setMasterVolume, setMasterMute, setDefaultSinkChannel,
    type EngineState, type AppStream } from "../ipc.js";
  import { engineState } from "../stores.js";
  import AppPill from "./AppPill.svelte";
  import VolumeSlider from "./VolumeSlider.svelte";
  import Checkbox from "../ui/Checkbox.svelte";

  interface Props {
    mixerState: EngineState;
    unrouted: AppStream[];
    onClearStream: (id: string) => void;
    onError?: (msg: string) => void;
  }

  let { mixerState, unrouted, onClearStream, onError = () => {} }: Props = $props();

  // When the headset's physical volume knob is present + configured to drive the master,
  // it owns the master volume (read-only via the [0x07,0x25] HID frame — no write opcode).
  // The slider then mirrors the knob and is non-interactive ("synced"), mirroring ChatMix.
  const knobControlsMaster = $derived(
    mixerState.device_present && mixerState.knob_controls_master,
  );

  let dragOver = $state(false);

  function handleDragOver(e: DragEvent) {
    if (e.dataTransfer?.types.includes("text/asm-stream-id")) {
      e.preventDefault();
      dragOver = true;
    }
  }
  function handleDragLeave(e: DragEvent) {
    // Ignore dragleave fired when moving onto a child element of the drop area.
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
    if (id) onClearStream(id);
  }

  async function handleMasterVolumeCommit(pct: number) {
    // Rejections propagate to VolumeSlider → onError.
    engineState.set(await setMasterVolume(pct));
  }

  let muteBusy = $state(false);

  async function handleMuteToggle() {
    if (muteBusy) return;
    muteBusy = true;
    try {
      engineState.set(await setMasterMute(!mixerState.master_mute));
    } catch (err) {
      onError(err instanceof Error ? err.message : String(err));
    } finally {
      muteBusy = false;
    }
  }

  async function handleDefaultToggle(checked: boolean) {
    try {
      engineState.set(await setDefaultSinkChannel(checked ? "game" : null));
    } catch (err) {
      onError(err instanceof Error ? err.message : String(err));
    }
  }
</script>

<div class="strip master" role="listitem" style="--accent: var(--ss-accent-master)">
  <!-- ===== Header ===== -->
  <div class="strip-header">
    <span class="channel-icon" aria-hidden="true">◉</span>
    <h3 class="channel-label">MASTER</h3>
  </div>

  <!-- TODO (C3a/follow-up): output-device gear once a master-output / mic-source list IPC exists -->

  <!-- ===== Volume area ===== -->
  <div class="volume-area">
    <VolumeSlider
      volume={mixerState.master_volume_pct}
      accent="var(--ss-accent-master)"
      label="Master volume"
      oncommit={handleMasterVolumeCommit}
      disabled={knobControlsMaster}
      {onError}
    />
  </div>
  {#if knobControlsMaster}
    <span class="knob-hint">Synced to headset knob</span>
  {/if}

  <!-- ===== Mute button ===== -->
  <button
    class="mute-btn"
    class:muted={mixerState.master_mute}
    disabled={muteBusy}
    title={mixerState.master_mute ? "Unmute master" : "Mute master"}
    aria-label={mixerState.master_mute ? "Unmute master" : "Mute master"}
    aria-pressed={mixerState.master_mute}
    onclick={handleMuteToggle}
  >
    <span aria-hidden="true">{mixerState.master_mute ? "🔇" : "🔊"}</span>
  </button>

  <!-- ===== Auto-route toggle ===== -->
  <div class="default-toggle">
    <Checkbox
      checked={mixerState.default_sink_channel != null}
      onCheckedChange={handleDefaultToggle}
      ariaLabel="Auto-route new apps"
    />
    <span>Auto-route new apps</span>
  </div>

  <!-- ===== Apps tray ===== -->
  <p class="tray-label">Apps to be routed</p>
  <div class="tray" class:drag-over={dragOver}
    role="list" aria-label="Unrouted applications"
    ondragover={handleDragOver} ondragleave={handleDragLeave} ondrop={handleDrop}>
    {#each unrouted as s (s.id)}
      <AppPill stream={s} accent="var(--ss-accent-master)" />
    {/each}
    {#if unrouted.length === 0}
      <span class="tray-empty">All apps routed</span>
    {/if}
  </div>
</div>

<style>
  .strip.master {
    display: flex; flex-direction: column; gap: var(--ss-space-3);
    flex: 1 1 0; min-width: 140px;
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border);
    border-top: 2px solid var(--accent);
    border-radius: var(--ss-radius-md); padding: var(--ss-space-3);
    box-shadow: var(--ss-e1);
  }

  /* ===== Header ===== */
  .strip-header {
    display: flex; align-items: center; gap: var(--ss-space-2);
  }
  .channel-icon {
    font-size: 14px; line-height: 1;
  }
  .channel-label {
    font-family: var(--ss-font-display); font-size: 13px; font-weight: 700;
    letter-spacing: 0.06em; text-transform: uppercase;
    color: var(--ss-text-primary); margin: 0; line-height: 1;
  }

  /* ===== Volume area ===== */
  .volume-area {
    display: flex; flex-direction: row; align-items: stretch; justify-content: center;
    gap: var(--ss-space-2); flex: 1; min-height: 120px;
  }

  .knob-hint {
    display: block; text-align: center;
    font-family: var(--ss-font-ui); font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary); margin-top: var(--ss-space-1);
  }

  /* ===== Mute button ===== */
  .mute-btn {
    display: flex; align-items: center; justify-content: center;
    width: var(--ss-control-h-sm); height: var(--ss-control-h-sm);
    border-radius: var(--ss-radius-xs); background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border);
    cursor: pointer; font-size: 13px; align-self: center;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }
  .mute-btn:hover { border-color: var(--ss-border-strong); background: var(--ss-surface-2); }
  .mute-btn.muted { background: var(--accent); border-color: var(--accent); }
  .mute-btn:disabled { cursor: not-allowed; opacity: 0.5; }
  .mute-btn:focus-visible { outline: 2px solid var(--ss-accent); outline-offset: 2px; }

  /* ===== Auto-route toggle ===== */
  .default-toggle {
    display: flex; gap: var(--ss-space-2); align-items: center;
    font-family: var(--ss-font-ui); font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
  }

  /* ===== Apps tray ===== */
  .tray-label {
    font-family: var(--ss-font-ui); font-size: var(--ss-type-micro-size);
    text-transform: uppercase; color: var(--ss-text-tertiary);
    margin: var(--ss-space-1) 0 0;
  }
  .tray {
    display: flex; flex-direction: column; gap: var(--ss-space-1); min-height: 60px;
    padding: var(--ss-space-2); border: var(--ss-border-width) dashed var(--ss-border);
    border-radius: var(--ss-radius-sm);
    transition: background var(--ss-dur-fast) var(--ss-ease-standard);
  }
  .tray.drag-over { background: var(--ss-drag-highlight); border-color: var(--accent); }
  .tray-empty {
    font-family: var(--ss-font-ui); font-size: var(--ss-type-caption-size);
    color: var(--ss-text-disabled); font-style: italic;
  }
</style>
