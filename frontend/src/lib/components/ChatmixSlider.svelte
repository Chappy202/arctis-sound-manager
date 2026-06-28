<script lang="ts">
  /**
   * ChatmixSlider.svelte — Horizontal ChatMix balance slider (0–9 integer).
   *
   * Wraps bits-ui Slider (type="single", orientation="horizontal") for the
   * Game↔Chat balance. The backend contract is 0–9 (set_chatmix position),
   * NOT the 0–100 volume model — do not change the scale.
   *
   * - Local $state `value` tracks the thumb position; updates immediately
   *   during drag for zero perceived lag.
   * - onValueChange → arms dragging flag, updates local value.
   * - onValueCommit → flushes IPC commit on pointer-up; clears drag flag.
   * - Reconcile $effect → accepts engine echoes only when NOT dragging
   *   (untrack prevents the effect from re-running when dragging/value change).
   * - Grey-out: when hardwareActive the slider is disabled + dimmed with a
   *   "Controlled by headset dial" hint; slider remains visible.
   */
  import { untrack } from "svelte";
  import { Slider } from "bits-ui";
  import { setChatmix } from "../ipc.js";
  import { engineState } from "../stores.js";
  import { toErrorMsg } from "./channelStripUtils.js";

  let { position, hardwareActive = false, onError = () => {} }:
    { position: number; hardwareActive?: boolean; onError?: (msg: string) => void } = $props();

  // -----------------------------------------------------------------------
  // Local state — thumb position + drag flag
  // -----------------------------------------------------------------------

  // untrack() reads the initial prop value without registering a reactive dep.
  // Subsequent engine updates flow through the reconcile $effect below.
  //
  // Display↔position inversion: engine 0=full-chat, 9=full-game; spec requires
  // Game on the LEFT (slider min). So display = 9 − position:
  //   engine 9 (full game) → display 0 → thumb at LEFT under "Game" label ✓
  //   engine 0 (full chat) → display 9 → thumb at RIGHT under "Chat" label ✓
  let value = $state(untrack(() => 9 - position));
  let dragging = $state(false);

  // -----------------------------------------------------------------------
  // IPC commit — log errors and surface via onError prop (M1)
  // -----------------------------------------------------------------------

  async function commit(pos: number) {
    try {
      engineState.set(await setChatmix(pos));
    } catch (e) {
      const msg = toErrorMsg(e);
      console.error("setChatmix failed:", e);
      onError(msg);
    }
  }

  // -----------------------------------------------------------------------
  // bits-ui Slider callbacks
  // -----------------------------------------------------------------------

  function handleValueChange(v: number) {
    dragging = true;
    value = v;
    // 0–9 is coarse (10 steps) — no throttle needed; commit on every step
  }

  function handleValueCommit(v: number) {
    value = v;
    void commit(9 - v);   // invert display→engine: display 0 (Game/left) → engine 9
    dragging = false;
  }

  // -----------------------------------------------------------------------
  // Reconcile $effect — accept engine echoes only when NOT dragging.
  //
  // `position` is the only reactive dependency; reading `dragging` inside
  // untrack() prevents this effect from re-running when dragging changes.
  // -----------------------------------------------------------------------

  $effect(() => {
    const incoming = position;          // reactive dep
    if (untrack(() => !dragging)) value = 9 - incoming;  // invert engine→display
  });
</script>

<div class="chatmix" class:disabled={hardwareActive}>
  <span class="end game">🎮 Game</span>

  <!--
    bits-ui Slider.Root with type="single" orientation="horizontal":
      value  → scalar 0–9 (controlled)
      onValueChange  → called each drag step
      onValueCommit  → called once on pointer-up / key-commit
  -->
  <Slider.Root
    type="single"
    orientation="horizontal"
    min={0}
    max={9}
    step={1}
    bind:value={value}
    disabled={hardwareActive}
    onValueChange={handleValueChange}
    onValueCommit={handleValueCommit}
    aria-label="ChatMix balance"
    class="chatmix-slider-root"
  >
    <!-- Visual track — overflow:hidden clips the Range fill cleanly -->
    <span class="chatmix-slider-track">
      <Slider.Range class="chatmix-slider-range" />
    </span>
    <!-- Thumb — positioned absolutely by bits-ui within the Root -->
    <Slider.Thumb index={0} class="chatmix-slider-thumb" />
  </Slider.Root>

  <span class="end chat">💬 Chat</span>

  {#if hardwareActive}
    <span class="hw-note">Controlled by headset dial</span>
  {/if}
</div>

<style>
  /* ===== Container ===== */
  .chatmix {
    display: flex;
    align-items: center;
    gap: var(--ss-space-3);
    padding: var(--ss-space-3);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    flex-wrap: wrap;
  }

  .chatmix.disabled { opacity: 0.6; }

  /* ===== End labels ===== */
  .end {
    font-family: var(--ss-font-display);
    text-transform: uppercase;
    font-size: var(--ss-type-caption-size);
    white-space: nowrap;
  }
  .end.game { color: var(--ss-accent-game); }
  .end.chat { color: var(--ss-accent-chat); }

  /* ===== Hardware-dial hint ===== */
  .hw-note {
    width: 100%;
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    text-align: center;
  }

  /* ===== Slider root (bits-ui generated element — must use :global) ===== */
  :global(.chatmix-slider-root) {
    position: relative;
    display: flex;
    flex-direction: row;
    align-items: center;
    flex: 1;
    min-width: 80px;
    height: 36px;
    touch-action: none;
    user-select: none;
    cursor: pointer;
    flex-shrink: 0;
  }

  :global(.chatmix-slider-root[data-disabled]) {
    cursor: not-allowed;
    pointer-events: none;
  }

  /* ===== Track — scoped (direct <span> in our template) ===== */
  .chatmix-slider-track {
    position: relative;
    height: 4px;
    width: 100%;
    background: var(--ss-surface-input-alt);
    border-radius: var(--ss-radius-pill);
    overflow: hidden;
    flex-grow: 1;
    pointer-events: none;
  }

  /* ===== Range fill (bits-ui generated — :global) ===== */
  :global(.chatmix-slider-range) {
    position: absolute;
    top: 0;
    left: 0;
    height: 100%;
    /* width is set inline by bits-ui based on the value percentage */
    background: var(--ss-accent);
    border-radius: var(--ss-radius-pill);
  }

  /* ===== Thumb (bits-ui generated — :global) ===== */
  :global(.chatmix-slider-thumb) {
    display: block;
    position: absolute;
    /* left set inline by bits-ui for horizontal positioning */
    width: 16px;
    height: 16px;
    background: white;
    border: 2px solid var(--ss-accent);
    border-radius: var(--ss-radius-pill);
    box-shadow: var(--ss-e1);
    cursor: grab;
    /* Center vertically — bits-ui may not translate, so we help */
    top: 50%;
    transform: translateY(-50%);
    transition:
      box-shadow var(--ss-dur-fast) var(--ss-ease-standard),
      transform var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  :global(.chatmix-slider-thumb:hover) {
    box-shadow: var(--ss-glow-accent);
  }

  :global(.chatmix-slider-thumb[data-active]) {
    cursor: grabbing;
    /* Slight scale for tactile feedback — keeps tracked center */
    transform: translateY(-50%) scale(1.1);
    box-shadow: var(--ss-glow-accent);
  }

  :global(.chatmix-slider-thumb:focus-visible) {
    outline: 2px solid var(--ss-accent);
    outline-offset: 3px;
  }

  :global(.chatmix-slider-thumb[data-disabled]) {
    background: var(--ss-surface-input-alt);
    border-color: var(--ss-border-strong);
    cursor: not-allowed;
    box-shadow: none;
  }
</style>
