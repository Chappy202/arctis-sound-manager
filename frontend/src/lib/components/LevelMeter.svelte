<script lang="ts">
  /**
   * LevelMeter.svelte — A small horizontal or vertical level bar driven by
   * the `levels` Tauri event.
   *
   * NOTE: The meter shows the *configured software volume* (0.0–1.0 linear
   * scalar from `pw-dump` Props.channelVolumes), NOT a real-time audio signal
   * peak or RMS.  It updates when the engine's volume for this node changes.
   * True peak metering requires a native pipewire-rs capture stream
   * (documented follow-up).
   */
  import { onMount, onDestroy } from "svelte";
  import { onLevels, type LevelsPayload } from "../ipc.js";
  import { smoothLevel, levelToBarStyle } from "../meter.js";

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
      smoothed = smoothLevel(smoothed, raw);
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
    ? `Volume: ${Math.round((smoothed ?? 0) * 100)}%`
    : "Waiting for level data…"}
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
    transition: width 0.35s ease-out;
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
    transition: height 0.35s ease-out;
  }

  /* No-data state — very dim, static */
  .meter-fill--no-data {
    opacity: 0.2;
  }
</style>
