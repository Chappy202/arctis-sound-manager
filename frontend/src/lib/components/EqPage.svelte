<script lang="ts">
  /**
   * EqPage.svelte — Parametric EQ editing page for a selected channel.
   *
   * KNOWN LIMITATION: The daemon's get-state response includes eq_bands as an
   * array of EqBandSnapshot, but the engine does not yet persist and return the
   * per-band parameters (freqHz / q / gainDb) set by the user. Therefore this
   * page initialises with default flat bands at standard log-spaced center
   * frequencies (0 dB, Q=1). Dragging the dots calls set-eq-band live and the
   * changes take effect in the audio immediately, but will reset to defaults
   * on page reload until the engine enhancement lands.
   * See task-6-brief.md "Open Questions" for the resolution path.
   */

  import { onMount } from "svelte";
  import { engineState } from "../stores.js";
  import { currentPage } from "../stores/page.js";
  import EqCanvas from "./EqCanvas.svelte";
  import BandList from "./BandList.svelte";
  import { defaultBands, type Band } from "../eq.js";

  // ---------------------------------------------------------------------------
  // Channel selection
  // ---------------------------------------------------------------------------

  // Read the channel id set by ChannelStrip.openEq() or default to the first channel.
  let channelId = $state<string>("");

  // Track bands per-channel so switching channels doesn't lose edits
  const bandsByChannel = $state<Record<string, Band[]>>({});

  let selectedBandIndex = $state(0);

  // Current channel's bands (derived from per-channel cache or defaults)
  let bands = $state<Band[]>([]);

  function getOrInitBands(id: string): Band[] {
    if (!bandsByChannel[id]) {
      // Number of bands = number of EqBandSnapshot entries the engine reported
      // (or 10 as the default).
      const channel = $engineState?.channels.find((c) => c.id === id);
      const count = channel?.eq_bands?.length ?? 10;
      bandsByChannel[id] = defaultBands(Math.max(1, Math.min(count, 10)));
    }
    return bandsByChannel[id];
  }

  function selectChannel(id: string) {
    channelId = id;
    bands = getOrInitBands(id);
    selectedBandIndex = 0;
  }

  onMount(() => {
    // Read deep-link from ChannelStrip's openEq()
    const stored = sessionStorage.getItem("eq:channel");
    if (stored) sessionStorage.removeItem("eq:channel");

    const channels = $engineState?.channels ?? [];
    if (channels.length === 0) {
      // No state yet — use a placeholder; state-changed will trigger re-render
      channelId = stored ?? "game";
      bands = defaultBands(10);
      return;
    }

    const target = stored
      ? channels.find((c) => c.id === stored)
      : channels[0];

    selectChannel(target?.id ?? channels[0].id);
  });

  // When engine state loads/updates, initialise channelId if not yet set
  $effect(() => {
    if (!channelId && $engineState?.channels.length) {
      selectChannel($engineState.channels[0].id);
    }
  });

  // ---------------------------------------------------------------------------
  // Band change handler (from canvas or list)
  // ---------------------------------------------------------------------------

  function handleBandChange(index: number, band: Band) {
    if (!channelId) return;
    if (!bandsByChannel[channelId]) return;
    bandsByChannel[channelId][index] = band;
    // Trigger reactivity
    bands = [...bandsByChannel[channelId]];
  }

  function handleSelectBand(index: number) {
    selectedBandIndex = index;
  }

  // ---------------------------------------------------------------------------
  // Derived state
  // ---------------------------------------------------------------------------

  const channels = $derived($engineState?.channels ?? []);

  const currentChannelLabel = $derived(
    channels.find((c) => c.id === channelId)?.node_name ||
      channelId.toUpperCase() ||
      "—",
  );

  const CHANNEL_ICONS: Record<string, string> = {
    game: "⊞",
    chat: "💬",
    media: "♪",
    aux: "⊕",
    mic: "⏺",
    master: "◉",
  };

  function getIcon(id: string): string {
    return CHANNEL_ICONS[id.toLowerCase()] ?? "◈";
  }

  function goBack() {
    currentPage.set("mixer");
  }
</script>

<div class="eq-page">
  <!-- ===== Page header ===== -->
  <div class="eq-header">
    <div class="eq-title-row">
      <button class="back-btn" onclick={goBack} aria-label="Back to Mixer" title="Back to Mixer">
        ◁
      </button>
      <span class="channel-icon" aria-hidden="true">{getIcon(channelId)}</span>
      <h1 class="eq-title">{currentChannelLabel}</h1>
      <span class="eq-subtitle">PARAMETRIC EQ</span>
    </div>

    <!-- Channel selector tabs -->
    {#if channels.length > 1}
      <div class="channel-tabs" role="tablist" aria-label="Select channel to edit">
        {#each channels as ch}
          <button
            class="channel-tab"
            class:active={ch.id === channelId}
            role="tab"
            aria-selected={ch.id === channelId}
            aria-label={ch.node_name || ch.id.toUpperCase()}
            onclick={() => selectChannel(ch.id)}
          >
            <span aria-hidden="true">{getIcon(ch.id)}</span>
            {ch.id.toUpperCase()}
          </button>
        {/each}
      </div>
    {/if}
  </div>

  <!-- ===== Defaults notice ===== -->
  <div class="defaults-notice" role="note" aria-label="Band values notice">
    <span class="notice-icon" aria-hidden="true">ℹ</span>
    Showing default values — the engine does not yet report live band parameters.
    Changes take effect immediately in audio but reset on page reload.
  </div>

  <!-- ===== EQ Canvas (hero) ===== -->
  <div class="eq-canvas-card">
    <div class="canvas-area">
      <EqCanvas
        {channelId}
        {bands}
        {selectedBandIndex}
        onBandChange={handleBandChange}
        onSelectBand={handleSelectBand}
      />
    </div>

    <!-- Gesture hint -->
    <div class="gesture-hint" aria-hidden="true">
      <span>Drag dot = freq / gain</span>
      <span class="hint-sep">·</span>
      <span>Scroll on dot = Q</span>
      <span class="hint-sep">·</span>
      <span>↑↓ arrows = gain · ←→ arrows = freq · Shift = coarse</span>
    </div>
  </div>

  <!-- ===== Band list ===== -->
  <div class="band-list-card">
    <div class="card-header">
      <h2 class="card-title">BANDS</h2>
      <span class="band-count">{bands.length}</span>
    </div>
    <BandList
      {channelId}
      {bands}
      {selectedBandIndex}
      onSelectBand={handleSelectBand}
      onBandChange={handleBandChange}
    />
  </div>
</div>

<style>
  .eq-page {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-4);
    height: 100%;
    min-height: 0;
  }

  /* ===== Page header ===== */
  .eq-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: var(--ss-space-4);
  }

  .eq-title-row {
    display: flex;
    align-items: center;
    gap: var(--ss-space-3);
  }

  .back-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: var(--ss-icon-btn);
    height: var(--ss-icon-btn);
    background: var(--ss-surface-1);
    border: 1px solid var(--ss-border);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-secondary);
    font-size: 14px;
    cursor: pointer;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .back-btn:hover {
    background: var(--ss-surface-2);
    color: var(--ss-text-primary);
    border-color: var(--ss-border-strong);
  }

  .back-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .channel-icon {
    font-size: 20px;
    line-height: 1;
  }

  .eq-title {
    font-family: var(--ss-font-display);
    font-size: var(--ss-type-display-size);
    font-weight: var(--ss-type-display-weight);
    line-height: var(--ss-type-display-line-height);
    letter-spacing: var(--ss-type-display-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-bright);
    margin: 0;
  }

  .eq-subtitle {
    font-family: var(--ss-font-display);
    font-size: var(--ss-type-h2-size);
    font-weight: var(--ss-type-h2-weight);
    letter-spacing: var(--ss-type-h2-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-accent);
    line-height: 1;
  }

  /* ===== Channel tabs ===== */
  .channel-tabs {
    display: flex;
    gap: 2px;
    background: var(--ss-surface-1);
    border: 1px solid var(--ss-border);
    border-radius: var(--ss-radius-sm);
    padding: 3px;
  }

  .channel-tab {
    display: flex;
    align-items: center;
    gap: var(--ss-space-1);
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-3);
    border-radius: var(--ss-radius-xs);
    background: transparent;
    border: none;
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-secondary);
    cursor: pointer;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .channel-tab:hover {
    background: var(--ss-surface-2);
    color: var(--ss-text-primary);
  }

  .channel-tab.active {
    background: var(--ss-accent-soft);
    color: var(--ss-accent);
  }

  .channel-tab:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 1px;
  }

  /* ===== Defaults notice ===== */
  .defaults-notice {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    padding: var(--ss-space-2) var(--ss-space-3);
    background: var(--ss-danger-soft, rgba(229, 72, 77, 0.10));
    border: 1px solid rgba(229, 72, 77, 0.25);
    border-radius: var(--ss-radius-xs);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-secondary);
    flex-shrink: 0;
  }

  .notice-icon {
    color: var(--ss-warning);
    font-size: 12px;
    flex-shrink: 0;
  }

  /* ===== EQ Canvas card ===== */
  .eq-canvas-card {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-2);
    min-height: 0;
  }

  .canvas-area {
    flex: 1;
    min-height: 240px;
    border-radius: var(--ss-radius-md);
    overflow: hidden;
    border: 1px solid var(--ss-border);
    box-shadow: var(--ss-e1);
  }

  .gesture-hint {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    flex-wrap: wrap;
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    padding: 0 var(--ss-space-1);
  }

  .hint-sep {
    color: var(--ss-border-strong);
  }

  /* ===== Band list card ===== */
  .band-list-card {
    background: var(--ss-surface-1);
    border: 1px solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    padding: var(--ss-space-4) var(--ss-space-4) var(--ss-space-3);
    box-shadow: var(--ss-e1);
    flex-shrink: 0;
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: var(--ss-space-3);
    margin-bottom: var(--ss-space-3);
    padding-bottom: var(--ss-space-3);
    border-bottom: 1px solid var(--ss-border);
  }

  .card-title {
    font-family: var(--ss-font-display);
    font-size: var(--ss-type-h2-size);
    font-weight: var(--ss-type-h2-weight);
    letter-spacing: var(--ss-type-h2-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-primary);
    margin: 0;
  }

  .band-count {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    min-width: 20px;
    height: 18px;
    padding: 0 var(--ss-space-1);
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-pill);
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-caption-size);
    font-variant-numeric: tabular-nums;
    color: var(--ss-text-tertiary);
  }
</style>
