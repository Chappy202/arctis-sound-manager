<script lang="ts">
  import { setChatmix } from "../ipc.js";
  import { engineState } from "../stores.js";

  let { position, hardwareActive = false }:
    { position: number; hardwareActive?: boolean } = $props();

  async function handleInput(e: Event) {
    const pos = Number((e.target as HTMLInputElement).value);
    engineState.set(await setChatmix(pos));
  }
</script>

<div class="chatmix" class:disabled={hardwareActive}>
  <span class="end game">Game</span>
  <input type="range" min="0" max="9" step="1" value={position}
    disabled={hardwareActive} oninput={handleInput} aria-label="ChatMix balance" />
  <span class="end chat">Chat</span>
  {#if hardwareActive}
    <span class="hw-note">Hardware dial active</span>
  {/if}
</div>

<style>
  .chatmix { display: flex; align-items: center; flex-wrap: wrap; gap: var(--ss-space-3);
    padding: var(--ss-space-3); background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border); border-radius: var(--ss-radius-md); }
  .chatmix input { flex: 1; accent-color: var(--ss-accent); }
  .chatmix.disabled { opacity: 0.6; }
  .end { font-family: var(--ss-font-display); text-transform: uppercase;
    font-size: var(--ss-type-caption-size); }
  .end.game { color: var(--ss-accent-game); }
  .end.chat { color: var(--ss-accent-chat); }
  .hw-note { font-size: var(--ss-type-caption-size); color: var(--ss-text-tertiary); }
</style>
