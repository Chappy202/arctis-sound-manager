<script lang="ts">
  import { micEnable, setMicVolume, type MicSnapshot } from "../ipc.js";
  import { engineState } from "../stores.js";
  import { currentPage } from "../stores/page.js";
  import VolumeSlider from "./VolumeSlider.svelte";
  import Switch from "../ui/Switch.svelte";

  interface Props {
    mic: MicSnapshot;
    onError?: (msg: string) => void;
  }

  let { mic, onError = () => {} }: Props = $props();

  async function handleMicVolumeCommit(pct: number) {
    // Rejections propagate to VolumeSlider → onError.
    engineState.set(await setMicVolume(pct));
  }

  async function handleToggle(on: boolean) {
    try {
      engineState.set(await micEnable(on));
    } catch (err) {
      onError(err instanceof Error ? err.message : String(err));
    }
  }
</script>

<div class="strip mic" style="--accent: var(--ss-accent-mic)">
  <!-- ===== Header ===== -->
  <div class="strip-header">
    <span class="channel-icon" aria-hidden="true">⏺</span>
    <h3 class="channel-label">MIC</h3>
  </div>

  <!-- TODO (C3a/follow-up): mic-source gear once a mic-source list IPC exists -->

  <!-- ===== Volume area ===== -->
  <div class="volume-area">
    <VolumeSlider
      volume={mic.volume_pct}
      accent="var(--ss-accent-mic)"
      label="Mic volume"
      oncommit={handleMicVolumeCommit}
      {onError}
      disabled={!mic.enabled}
    />
  </div>

  <!-- ===== On/Off switch ===== -->
  <div class="switch-row">
    <Switch
      checked={mic.enabled}
      onCheckedChange={handleToggle}
      ariaLabel="Mic enabled"
    />
    <span class="switch-label">{mic.enabled ? "On" : "Off"}</span>
  </div>

  <!-- ===== Edit button ===== -->
  <button class="edit-btn" aria-label="Edit mic settings" onclick={() => currentPage.set("mic")}>
    Edit
  </button>
</div>

<style>
  .strip.mic {
    display: flex; flex-direction: column; gap: var(--ss-space-3);
    width: var(--ss-channel-strip-w, 120px); min-width: var(--ss-channel-strip-w-min, 100px);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border);
    border-top: 2px solid var(--accent);
    border-radius: var(--ss-radius-md); padding: var(--ss-space-3);
    flex-shrink: 0; box-shadow: var(--ss-e1);
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

  /* ===== On/Off switch row ===== */
  .switch-row {
    display: flex; align-items: center; gap: var(--ss-space-2);
  }
  .switch-label {
    font-family: var(--ss-font-ui); font-size: var(--ss-type-caption-size);
    color: var(--ss-text-secondary); min-width: 2ch;
  }

  /* ===== Edit button ===== */
  .edit-btn {
    display: flex; align-items: center; justify-content: center;
    height: var(--ss-control-h-sm); padding: 0 var(--ss-space-3);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-xs); color: var(--ss-text-secondary);
    font-family: var(--ss-font-ui); font-size: var(--ss-type-caption-size);
    font-weight: var(--ss-type-button-weight);
    letter-spacing: var(--ss-type-button-letter-spacing);
    text-transform: uppercase; cursor: pointer;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
  }
  .edit-btn:hover {
    border-color: var(--ss-border-strong);
    background: var(--ss-surface-2);
    color: var(--ss-text-primary);
  }
  .edit-btn:focus-visible { outline: 2px solid var(--ss-accent); outline-offset: 2px; }
</style>
