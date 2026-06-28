<script lang="ts">
  import type { AppStream } from "../ipc.js";
  import { pillTitle } from "./appPillUtils.js";
  let { stream, accent = "var(--ss-accent)" }: { stream: AppStream; accent?: string } = $props();

  function onDragStart(e: DragEvent) {
    if (!e.dataTransfer) return;
    // Carry the live node id so the drop target can move the exact instance.
    e.dataTransfer.setData("text/asm-stream-id", String(stream.id));
    e.dataTransfer.effectAllowed = "move";
  }
</script>

<div
  class="app-pill"
  draggable="true"
  ondragstart={onDragStart}
  style="--pill-accent: {accent}"
  title={pillTitle(stream)}
  role="listitem"
>
  <span class="pill-dot" aria-hidden="true"></span>
  <span class="pill-name">{stream.app_name}</span>
</div>

<style>
  .app-pill {
    display: inline-flex;
    align-items: center;
    gap: var(--ss-space-1);
    padding: 2px var(--ss-space-2);
    background: var(--ss-surface-2);
    border: var(--ss-border-width) solid var(--pill-accent);
    border-radius: var(--ss-radius-pill);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    cursor: grab;
    user-select: none;
    max-width: 100%;
  }
  .app-pill:active { cursor: grabbing; }
  .pill-dot {
    width: 8px; height: 8px; border-radius: 50%;
    background: var(--pill-accent); flex-shrink: 0;
  }
  .pill-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
