<script lang="ts">
  import { setMasterVolume, setMasterMute, setDefaultSinkChannel,
    type EngineState, type AppStream } from "../ipc.js";
  import { engineState } from "../stores.js";
  import AppPill from "./AppPill.svelte";

  let { state, unrouted, onClearStream }:
    { state: EngineState; unrouted: AppStream[]; onClearStream: (id: string) => void } = $props();

  let dragOver = $state(false);

  function handleDragOver(e: DragEvent) {
    if (e.dataTransfer?.types.includes("text/asm-stream-id")) {
      e.preventDefault();
      dragOver = true;
    }
  }
  function handleDragLeave() { dragOver = false; }
  function handleDrop(e: DragEvent) {
    e.preventDefault();
    dragOver = false;
    const id = e.dataTransfer?.getData("text/asm-stream-id");
    if (id) onClearStream(id);
  }

  async function handleVolumeChange(e: Event) {
    const db = Number((e.target as HTMLInputElement).value);
    engineState.set(await setMasterVolume(db));
  }
  async function handleMuteToggle() {
    engineState.set(await setMasterMute(!state.master_mute));
  }
  async function handleDefaultToggle(e: Event) {
    const checked = (e.target as HTMLInputElement).checked;
    engineState.set(await setDefaultSinkChannel(checked ? "game" : null));
  }
</script>

<div class="strip master" style="--accent: var(--ss-accent-master)">
  <h3 class="strip-name">MASTER</h3>
  <input class="vol" type="range" min="-60" max="6" step="1"
    value={state.master_volume_db} oninput={handleVolumeChange} aria-label="Master volume" />
  <span class="vol-label">{state.master_volume_db.toFixed(0)} dB</span>
  <button class="mute" class:on={state.master_mute} onclick={handleMuteToggle}
    aria-pressed={state.master_mute}>{state.master_mute ? "Muted" : "Mute"}</button>

  <label class="default-toggle">
    <input type="checkbox" checked={state.default_sink_channel != null}
      onchange={handleDefaultToggle} />
    Auto-route new apps
  </label>

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
    display: flex; flex-direction: column; gap: var(--ss-space-2);
    width: var(--ss-channel-strip-w, 120px); min-width: var(--ss-channel-strip-w-min, 100px);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--accent);
    border-radius: var(--ss-radius-md); padding: var(--ss-space-3); flex-shrink: 0;
  }
  .strip-name { font-family: var(--ss-font-display); text-transform: uppercase;
    color: var(--accent); margin: 0; font-size: var(--ss-type-h2-size); }
  .vol { width: 100%; accent-color: var(--accent); }
  .vol-label { font-family: var(--ss-font-mono); font-size: var(--ss-type-caption-size);
    color: var(--ss-text-secondary); }
  .mute { cursor: pointer; }
  .mute.on { color: var(--ss-danger); }
  .default-toggle { display: flex; gap: var(--ss-space-1); align-items: center;
    font-size: var(--ss-type-caption-size); color: var(--ss-text-tertiary); }
  .tray-label { font-size: var(--ss-type-micro-size); text-transform: uppercase;
    color: var(--ss-text-tertiary); margin: var(--ss-space-2) 0 0; }
  .tray { display: flex; flex-direction: column; gap: var(--ss-space-1); min-height: 60px;
    padding: var(--ss-space-2); border: var(--ss-border-width) dashed var(--ss-border);
    border-radius: var(--ss-radius-sm); }
  .tray.drag-over { background: var(--ss-drag-highlight); border-color: var(--accent); }
  .tray-empty { font-size: var(--ss-type-caption-size); color: var(--ss-text-disabled);
    font-style: italic; }
</style>
