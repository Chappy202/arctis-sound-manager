<script lang="ts">
  /**
   * VolumeSlider.svelte — Responsive vertical volume slider (0–100 %).
   *
   * Uses bits-ui Slider (type="single") for keyboard/pointer accessibility and
   * wraps volumeSliderLogic for throttled IPC commits + reconcile guard.
   *
   * Design:
   *   - Local $state `value` tracks the thumb position; updates every frame
   *     during drag (zero perceived lag).
   *   - onValueChange → arms the trailing throttle (80 ms) for cheap IPC.
   *   - onValueCommit → flushes immediately on pointer-up.
   *   - Reconcile $effect → accepts engine echoes only when NOT dragging
   *     (untrack prevents the effect from depending on dragging/value).
   */
  import { untrack } from "svelte";
  import { Slider } from "bits-ui";
  import { toErrorMsg } from "./channelStripUtils.js";
  import {
    createVolumeCommitter,
    VOLUME_COMMIT_INTERVAL_MS,
    reconcileValue,
  } from "./volumeSliderLogic.js";

  interface Props {
    /** 0–100 percent, incoming engine value (reactive). */
    volume: number;
    /** Called to persist the volume; may return a Promise (rejections go to onError). */
    oncommit: (v: number) => void | Promise<unknown>;
    /** Called with a user-readable message when oncommit rejects. */
    onError?: (msg: string) => void;
    /** ARIA label for the slider (e.g. "Volume for GAME"). */
    label?: string;
    /** CSS color override for the accent fill; defaults to var(--ss-accent). */
    accent?: string;
    disabled?: boolean;
  }

  let {
    volume,
    oncommit,
    onError,
    label = "Volume",
    accent,
    disabled = false,
  }: Props = $props();

  // -----------------------------------------------------------------------
  // Local state — thumb position + drag flag
  // -----------------------------------------------------------------------

  // untrack() reads the current prop value without registering a reactive
  // dependency. This seeds the local thumb position from the initial engine
  // value; subsequent engine updates flow through the reconcile $effect below.
  let value = $state(untrack(() => volume));
  let dragging = $state(false);

  // -----------------------------------------------------------------------
  // Committer — trailing throttle with immediate flush on pointer-up
  // -----------------------------------------------------------------------

  const committer = createVolumeCommitter(VOLUME_COMMIT_INTERVAL_MS, (v) => {
    // Route async rejections to onError; never let them throw uncaught.
    Promise.resolve(oncommit(v)).catch((e) => onError?.(toErrorMsg(e)));
  });

  // -----------------------------------------------------------------------
  // bits-ui Slider callbacks
  // -----------------------------------------------------------------------

  function handleValueChange(v: number) {
    dragging = true;
    value = v;
    committer.schedule(v);
  }

  function handleValueCommit(v: number) {
    value = v;
    committer.flush();
    dragging = false;
  }

  // -----------------------------------------------------------------------
  // Reconcile $effect — accept engine echoes only when NOT dragging.
  //
  // `volume` is the only reactive dependency; reading `dragging` and `value`
  // inside untrack() prevents this effect from re-running when they change.
  // -----------------------------------------------------------------------

  $effect(() => {
    const incoming = volume; // reactive dep
    value = untrack(() => reconcileValue(dragging, incoming, value));
  });

  // Dispose the committer on component teardown.
  $effect(() => () => committer.dispose());
</script>

<div
  class="volume-slider"
  style="--accent: {accent ?? 'var(--ss-accent)'}"
>
  <!--
    bits-ui Slider.Root with type="single":
      value  → scalar number (controlled)
      onValueChange  → called each frame during drag
      onValueCommit  → called once on pointer-up / key-commit
    aria-label is forwarded to the internal role="slider" thumb by bits-ui.
  -->
  <Slider.Root
    type="single"
    orientation="vertical"
    min={0}
    max={100}
    step={1}
    value={value}
    onValueChange={handleValueChange}
    onValueCommit={handleValueCommit}
    {disabled}
    aria-label={label}
    class="slider-root"
  >
    <!-- Visual track — overflow:hidden clips the Range fill cleanly -->
    <span class="slider-track">
      <Slider.Range class="slider-range" />
    </span>
    <!-- Thumb — positioned absolutely by bits-ui within the Root -->
    <Slider.Thumb index={0} class="slider-thumb" />
  </Slider.Root>

  <!-- Live % readout — uses local `value` so it tracks the drag instantly -->
  <div class="slider-readout" aria-hidden="true">{value}%</div>
</div>

<style>
  /* ===== Container ===== */
  .volume-slider {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--ss-space-2);
    height: 100%;
  }

  /* ===== Slider root (bits-ui generated element — must use :global) ===== */
  :global(.slider-root) {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    /* Fill available height; parent can set explicit height */
    flex: 1;
    min-height: 80px;
    width: 36px; /* comfortable hit area */
    touch-action: none;
    user-select: none;
    cursor: pointer;
    flex-shrink: 0;
  }

  :global(.slider-root[data-disabled]) {
    opacity: 0.5;
    cursor: not-allowed;
    pointer-events: none;
  }

  /* ===== Track — scoped (it's a direct <span> in our template) ===== */
  .slider-track {
    position: relative;
    width: 4px;
    /* Fill the full Root height */
    height: 100%;
    background: var(--ss-surface-input-alt);
    border-radius: var(--ss-radius-pill);
    overflow: hidden;
    flex-grow: 1;
    pointer-events: none;
  }

  /* ===== Range fill (bits-ui generated — :global) ===== */
  :global(.slider-range) {
    position: absolute;
    bottom: 0;
    left: 0;
    width: 100%;
    /* height is set inline by bits-ui based on the value percentage */
    background: var(--accent);
    border-radius: var(--ss-radius-pill);
  }

  /* ===== Thumb (bits-ui generated — :global) ===== */
  :global(.slider-thumb) {
    display: block;
    position: absolute;
    /* left/bottom set inline by bits-ui for positioning */
    width: 16px;
    height: 16px;
    background: white;
    border: 2px solid var(--accent);
    border-radius: var(--ss-radius-pill);
    box-shadow: var(--ss-e1);
    cursor: grab;
    /* Center horizontally — bits-ui may not translate, so we help */
    left: 50%;
    transform: translateX(-50%);
    transition:
      box-shadow var(--ss-dur-fast) var(--ss-ease-standard),
      transform var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  :global(.slider-thumb:hover) {
    box-shadow: var(--ss-glow-accent);
  }

  :global(.slider-thumb[data-active]) {
    cursor: grabbing;
    /* Slight scale for tactile feedback — doesn't shift the tracked center */
    transform: translateX(-50%) scale(1.1);
    box-shadow: var(--ss-glow-accent);
  }

  :global(.slider-thumb:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: 3px;
  }

  :global(.slider-thumb[data-disabled]) {
    background: var(--ss-surface-input-alt);
    border-color: var(--ss-border-strong);
    cursor: not-allowed;
    box-shadow: none;
  }

  /* ===== % Readout ===== */
  .slider-readout {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-readout-size);
    font-variant-numeric: tabular-nums;
    color: var(--ss-text-secondary);
    line-height: 1;
    flex-shrink: 0;
  }
</style>
