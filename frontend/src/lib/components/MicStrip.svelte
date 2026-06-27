<script lang="ts">
  import { micEnable, type MicSnapshot } from "../ipc.js";
  import { engineState } from "../stores.js";
  import { currentPage } from "../stores/page.js";

  let { mic }: { mic: MicSnapshot } = $props();

  async function handleToggle() {
    engineState.set(await micEnable(!mic.enabled));
  }
</script>

<div class="strip mic" style="--accent: var(--ss-accent-mic)">
  <h3 class="strip-name">MIC</h3>
  <button class="mute" class:on={!mic.enabled} onclick={handleToggle}
    aria-pressed={!mic.enabled}>{mic.enabled ? "On" : "Off"}</button>
  <button class="edit" onclick={() => currentPage.set("mic")}>Edit</button>
</div>

<style>
  .strip.mic { display: flex; flex-direction: column; gap: var(--ss-space-2);
    width: var(--ss-channel-strip-w, 120px); min-width: var(--ss-channel-strip-w-min, 100px);
    background: var(--ss-surface-1); border: var(--ss-border-width) solid var(--accent);
    border-radius: var(--ss-radius-md); padding: var(--ss-space-3); flex-shrink: 0; }
  .strip-name { font-family: var(--ss-font-display); text-transform: uppercase;
    color: var(--accent); margin: 0; font-size: var(--ss-type-h2-size); }
  .mute.on { color: var(--ss-danger); }
  .edit, .mute { cursor: pointer; }
</style>
