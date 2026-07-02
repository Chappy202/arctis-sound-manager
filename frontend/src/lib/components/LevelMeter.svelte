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
  import {
    onLevels,
    meterSubscribe,
    meterUnsubscribe,
    type LevelsPayload,
  } from "../ipc.js";
  import { peakDecay, clampLevel } from "../meter.js";

  // ── Shared module-level subscription ──────────────────────────────────────
  // ONE Tauri `levels` listener for ALL LevelMeter instances; it fans out to
  // each instance's handler. It also drives the Rust subscriber gate: the first
  // mounted meter subscribes, the last to unmount unsubscribes — and the Rust
  // side ties the pw-record capture workers to that count, so with no meter on
  // screen ZERO capture processes run. Hiding to tray counts as "no meter":
  // visibilitychange releases the Rust subscription while hidden and reclaims
  // it on show (capture teardown/restart is handled Rust-side).
  type LevelsHandler = (payload: LevelsPayload) => void;
  const handlers = new Set<LevelsHandler>();
  let sharedUnlisten: (() => void) | null = null;
  let refCount = 0;
  let rustSubscribed = false;

  function subscribeRust() {
    if (rustSubscribed) return;
    rustSubscribed = true;
    void meterSubscribe();
  }

  function unsubscribeRust() {
    if (!rustSubscribed) return;
    rustSubscribed = false;
    void meterUnsubscribe();
  }

  function handleVisibilityChange() {
    if (document.hidden) {
      unsubscribeRust(); // hidden to tray → stop pw-record capture entirely
    } else if (refCount > 0) {
      subscribeRust();
    }
  }

  async function acquireShared(handler: LevelsHandler) {
    handlers.add(handler);
    refCount += 1;
    if (refCount > 1) return; // listener + Rust subscription already live
    try {
      // Tell Rust at least one meter is mounted — unless the window is hidden,
      // in which case the visibilitychange handler subscribes on show.
      if (typeof document === "undefined" || !document.hidden) subscribeRust();
      if (typeof document !== "undefined") {
        document.addEventListener("visibilitychange", handleVisibilityChange);
      }
      const unlisten = await onLevels((payload) => {
        // Pause while the window/tab is hidden — no point smoothing meters
        // nobody can see, and it keeps the event off a backgrounded compositor.
        if (typeof document !== "undefined" && document.hidden) return;
        for (const h of handlers) h(payload);
      });
      // The only meter may have unmounted while we awaited — don't leak a live
      // listener; tear it down immediately if no one is interested anymore.
      if (refCount === 0) {
        unlisten();
        return;
      }
      sharedUnlisten = unlisten;
    } catch (err) {
      // Tauri unavailable in dev/test — meters stay in "no data" state.
      console.debug("[LevelMeter] could not subscribe to levels event:", err);
    }
  }

  function releaseShared(handler: LevelsHandler) {
    if (handlers.delete(handler)) {
      refCount = Math.max(0, refCount - 1);
    }
    if (refCount === 0) {
      sharedUnlisten?.();
      sharedUnlisten = null;
      if (typeof document !== "undefined") {
        document.removeEventListener("visibilitychange", handleVisibilityChange);
      }
      unsubscribeRust();
    }
  }

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

  // Fill scale factor in [0, 1] for the compositor-only transform.
  const fillScale = $derived(smoothed !== null ? clampLevel(smoothed) : 0);

  function handleLevels(payload: LevelsPayload) {
    const raw = payload[nodeName];
    if (typeof raw === "number") {
      const next = peakDecay(smoothed, raw);
      // Skip the reactive write when our slice didn't actually change
      // (e.g. sustained silence: 0 → 0). Avoids needless $derived re-runs.
      if (next !== smoothed) smoothed = next;
      if (!hasData) hasData = true;
    }
  }

  onMount(() => {
    void acquireShared(handleLevels);
  });

  onDestroy(() => {
    releaseShared(handleLevels);
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
        style="transform: scaleY({fillScale})"
      ></div>
    {:else}
      <div
        class="meter-fill meter-fill--horizontal"
        class:meter-fill--no-data={!hasData}
        style="transform: scaleX({fillScale})"
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
    width: 100%;
    height: 100%;
    background: var(--ss-accent);
    border-radius: var(--ss-radius-pill);
    /* Compositor-only fill: animate transform (scaleX), not layout width.
       transform-origin at the left edge so it grows left → right. */
    transform-origin: left center;
    will-change: transform;
    /* Short CSS transition — decay is handled in peakDecay() JS logic */
    transition: transform 0.066s linear;
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
    height: 100%;
    background: var(--ss-accent);
    border-radius: var(--ss-radius-pill);
    /* Compositor-only fill: animate transform (scaleY), not layout height.
       transform-origin at the bottom so it grows bottom → top. */
    transform-origin: center bottom;
    will-change: transform;
    /* Short CSS transition — decay is handled in peakDecay() JS logic */
    transition: transform 0.066s linear;
  }

  /* No-data state — very dim, static */
  .meter-fill--no-data {
    opacity: 0.2;
  }
</style>
