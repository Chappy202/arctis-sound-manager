<script lang="ts">
  /**
   * Slider.svelte — Styled horizontal slider over bits-ui Slider.
   *
   * Fully controlled (matches Select/Switch): the parent owns `value`;
   * `onValueChange` fires each frame during drag, `onValueCommit` (optional)
   * fires once on pointer-up / keyboard commit. No internal value state, so it
   * never drifts from the parent. Pure presentation wrapper.
   *
   * The caller renders its own label/readout — this wrapper is just the control,
   * so it drops cleanly in place of a native <input type="range">.
   */
  import { Slider } from "bits-ui";

  interface Props {
    /** Current value (controlled). */
    value: number;
    min: number;
    max: number;
    step?: number;
    /** Fires continuously while dragging / on each keystep. */
    onValueChange: (v: number) => void;
    /** Fires once on pointer-up or keyboard commit (defaults to onValueChange). */
    onValueCommit?: (v: number) => void;
    disabled?: boolean;
    ariaLabel?: string;
    id?: string;
    /** CSS color override for the range fill + thumb border. */
    accent?: string;
  }

  let {
    value,
    min,
    max,
    step = 1,
    onValueChange,
    onValueCommit,
    disabled = false,
    ariaLabel,
    id,
    accent,
  }: Props = $props();

  // Fully-controlled binding: bits-ui reads via getter, routes user moves back
  // through onValueChange. The parent is expected to update `value` synchronously
  // so the thumb tracks the drag without lag.
  const getVal = () => value;
  const setVal = (v: number) => onValueChange(v);
</script>

<div class="ui-slider" style="--accent: {accent ?? 'var(--ss-accent)'}">
  <Slider.Root
    type="single"
    orientation="horizontal"
    {min}
    {max}
    {step}
    {disabled}
    {id}
    bind:value={getVal, setVal}
    onValueCommit={(v) => (onValueCommit ?? onValueChange)(v)}
    aria-label={ariaLabel}
    class="ui-slider-root"
  >
    <span class="ui-slider-track">
      <Slider.Range class="ui-slider-range" />
    </span>
    <Slider.Thumb index={0} class="ui-slider-thumb" />
  </Slider.Root>
</div>

<style>
  .ui-slider {
    display: flex;
    width: 100%;
    align-items: center;
  }

  :global(.ui-slider-root) {
    position: relative;
    display: flex;
    align-items: center;
    width: 100%;
    height: 20px;
    touch-action: none;
    user-select: none;
    cursor: pointer;
  }

  :global(.ui-slider-root[data-disabled]) {
    opacity: 0.5;
    cursor: not-allowed;
    pointer-events: none;
  }

  /* ===== Track — scoped (direct <span> in our template) ===== */
  .ui-slider-track {
    position: relative;
    width: 100%;
    height: 4px;
    background: var(--ss-surface-input-alt);
    border-radius: var(--ss-radius-pill);
    overflow: hidden;
    pointer-events: none;
  }

  /* ===== Range fill (bits-ui generated — :global) ===== */
  :global(.ui-slider-range) {
    position: absolute;
    top: 0;
    left: 0;
    height: 100%;
    /* width set inline by bits-ui based on value percentage */
    background: var(--accent);
    border-radius: var(--ss-radius-pill);
  }

  /* ===== Thumb (bits-ui generated — :global) ===== */
  :global(.ui-slider-thumb) {
    display: block;
    position: absolute;
    /* left set inline by bits-ui for positioning */
    width: 16px;
    height: 16px;
    background: white;
    border: 2px solid var(--accent);
    border-radius: var(--ss-radius-pill);
    box-shadow: var(--ss-e1);
    cursor: grab;
    top: 50%;
    transform: translateY(-50%);
    transition:
      box-shadow var(--ss-dur-fast) var(--ss-ease-standard),
      transform var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  :global(.ui-slider-thumb:hover) {
    box-shadow: var(--ss-glow-accent);
  }

  :global(.ui-slider-thumb[data-active]) {
    cursor: grabbing;
    transform: translateY(-50%) scale(1.1);
    box-shadow: var(--ss-glow-accent);
  }

  :global(.ui-slider-thumb:focus-visible) {
    outline: 2px solid var(--accent);
    outline-offset: 3px;
  }

  :global(.ui-slider-thumb[data-disabled]) {
    background: var(--ss-surface-input-alt);
    border-color: var(--ss-border-strong);
    cursor: not-allowed;
    box-shadow: none;
  }
</style>
