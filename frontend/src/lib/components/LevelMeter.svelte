<script lang="ts">
  /**
   * LevelMeter.svelte — Real signal-peak level bar driven by the `levels`
   * Tauri event.
   *
   * The `levels` event carries genuine PCM peak values (0.0–1.0) captured
   * by `pw-record` workers in src-tauri/src/meters.rs.  A silent signal
   * shows 0.0 regardless of volume setting; a full-scale signal shows 1.0.
   *
   * Display uses a fast-attack / slow-decay envelope (peakDecay) so brief
   * transients are visible and the bar falls smoothly after loud passages.
   */
  import { onMount, onDestroy } from "svelte";
  import { onLevels, type LevelsPayload } from "../ipc.js";
  import { peakDecay, levelToBarStyle } from "../meter.js";

  interface Props {
    /** PipeWire node.name to show the level for (e.g. "Arctis_Game"). */
    nodeName: string;
    /** Layout: "horizontal" (default) fills left→right; "vertical" fills bottom→top. */
    orientation?: "horizontal" | "vertical";
    /** ARIA label for screen readers. */
    ariaLabel?: string;
  }

  let { nodeName, orientation = "horizontal", ariaLabel }: Props = $props();

  // Current smoothed level (null = no data received yet)
  let smoothed = $state<number | null>(null);
  // True once we have received at least one `levels` tick
  let hasData = $state(false);

  // Derived bar size string
  const barSize = $derived(
    smoothed !== null ? levelToBarStyle(smoothed) : "0%"
  );

  let unlisten: (() => void) | null = null;

  function handleLevels(payload: LevelsPayload) {
    const raw = payload[nodeName];
    if (typeof raw === "number") {
      smoothed = peakDecay(smoothed, raw);
      hasData = true;
    }
  }

  onMount(async () => {
    try {
      unlisten = await onLevels(handleLevels);
    } catch (err) {
      // Tauri unavailable in dev/test — meter stays in "no data" state.
      console.debug("[LevelMeter] could not subscribe to levels event:", err);
    }
  });

  onDestroy(() => {
    unlisten?.();
  });
</script>

<div
  class="level-meter level-meter--{orientation}"
  role="meter"
  aria-label={ariaLabel ?? `Level for ${nodeName}`}
  aria-valuenow={smoothed !== null ? Math.round((smoothed ?? 0) * 100) : 0}
  aria-valuemin={0}
  aria-valuemax={100}
  title={hasData
    ? `Signal peak: ${Math.round((smoothed ?? 0) * 100)}%`
    : "Waiting for signal data…"}
>
  <div
    class="meter-track"
    aria-hidden="true"
  >
    {#if orientation === "vertical"}
      <div
        class="meter-fill meter-fill--vertical"
        class:meter-fill--no-data={!hasData}
        style="height: {barSize}"
      ></div>
    {:else}
      <div
        class="meter-fill meter-fill--horizontal"
        class:meter-fill--no-data={!hasData}
        style="width: {barSize}"
      ></div>
    {/if}
  </div>
</div>

<style>
  .level-meter {
    display: flex;
    align-items: stretch;
    flex-shrink: 0;
  }

  /* ── Horizontal (default) — fills left → right ── */
  .level-meter--horizontal {
    width: 100%;
    height: 6px;
  }

  .level-meter--horizontal .meter-track {
    position: relative;
    width: 100%;
    height: 100%;
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-pill);
    overflow: hidden;
  }

  .meter-fill--horizontal {
    position: absolute;
    top: 0;
    left: 0;
    height: 100%;
    background: var(--ss-accent);
    border-radius: var(--ss-radius-pill);
    /* Short CSS transition — decay is handled in peakDecay() JS logic */
    transition: width 0.04s linear;
  }

  /* ── Vertical — fills bottom → top ── */
  .level-meter--vertical {
    width: 6px;
    height: 80px;
  }

  .level-meter--vertical .meter-track {
    position: relative;
    width: 100%;
    height: 100%;
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-pill);
    overflow: hidden;
    display: flex;
    align-items: flex-end;
  }

  .meter-fill--vertical {
    width: 100%;
    background: var(--ss-accent);
    border-radius: var(--ss-radius-pill);
    /* Short CSS transition — decay is handled in peakDecay() JS logic */
    transition: height 0.04s linear;
  }

  /* No-data state — very dim, static */
  .meter-fill--no-data {
    opacity: 0.2;
  }
</style>
