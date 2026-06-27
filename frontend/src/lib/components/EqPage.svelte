<script lang="ts">
  /**
   * EqPage.svelte — Parametric EQ editing page for a selected channel.
   *
   * Single source of truth: `bands` is the only band array. The engine snapshot
   * is reconciled via reconcileBands(), which preserves the local array while any
   * edit is in progress (pointer drag, scroll, keyboard, numeric field focus),
   * preventing the "disappearing dots" bug caused by the old 4-way bookkeeping.
   */

  import { untrack } from "svelte";
  import { get } from "svelte/store";
  import { engineState } from "../stores.js";
  import { currentPage } from "../stores/page.js";
  import { eqEditing, pulseEditing } from "../stores/eqEditing.js";
  import { reconcileBands, type Band } from "../eq.js";
  import { setEqBand, eqPresetSave, eqPresetApply, eqPresetDelete } from "../ipc.js";
  import EqGraph from "./EqGraph.svelte";
  import EqBandPanel from "./EqBandPanel.svelte";
  import BandList from "./BandList.svelte";

  // ---------------------------------------------------------------------------
  // Channel selection
  // ---------------------------------------------------------------------------

  let channelId = $state<string>("");
  let bands = $state<Band[]>([]);            // SINGLE source of truth for the active channel
  let selectedBandIndex = $state(0);

  function snapshotToBands(id: string): Band[] {
    const ch = $engineState?.channels.find((c) => c.id === id);
    return (ch?.eq_bands ?? []).map((b) => ({
      kind: b.kind as Band["kind"], freqHz: b.freq_hz, q: b.q, gainDb: b.gain_db,
    }));
  }

  function selectChannel(id: string) {
    channelId = id;
    bands = snapshotToBands(id);            // engine is dense-10; no fabrication
    selectedBandIndex = 0;
  }

  // Init once state is available.
  $effect(() => {
    if (!channelId && $engineState?.channels.length) {
      selectChannel($engineState.channels[0].id);
    }
  });

  // Reconcile from engine ONLY when idle (covers all edit modalities via eqEditing).
  $effect(() => {
    const st = $engineState;            // dependency
    if (!channelId || !st) return;
    const incoming = snapshotToBands(channelId);
    if (incoming.length === 0) return;
    bands = reconcileBands(untrack(() => bands), incoming, get(eqEditing));
  });

  // Single writer: all child edits land here.
  function handleBandChange(index: number, band: Band) {
    bands = bands.map((b, i) => (i === index ? band : b));
  }
  function handleSelect(index: number) { selectedBandIndex = index; }
  // Flush helper passed to the band panel (graph flushes internally via setEqBand).
  function flushBand(index: number, band: Band) {
    setEqBand(channelId, index, band.kind, band.freqHz, band.q, band.gainDb)
      .catch((e) => console.warn("[EqPage] setEqBand failed:", e));
  }
  async function flattenAll() {
    pulseEditing();
    const flat = bands.map((b) => ({ ...b, gainDb: 0 }));
    bands = flat;
    for (let i = 0; i < flat.length; i++) {
      try { await setEqBand(channelId, i, flat[i].kind, flat[i].freqHz, flat[i].q, 0); }
      catch (e) { console.warn("[EqPage] flatten band failed:", e); }
    }
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

  // ---------------------------------------------------------------------------
  // EQ preset management
  // ---------------------------------------------------------------------------

  /** Names of saved presets from engine state. */
  const eqPresets = $derived($engineState?.eq_presets ?? []);

  /** Whether we are in "save preset" mode (inline name entry). */
  let savingPreset = $state(false);
  let presetSaveName = $state("");
  let presetSaveInput: HTMLInputElement | undefined = $state();

  /** Transient feedback message. */
  let presetFeedback = $state<string | null>(null);
  let presetFeedbackTimer: ReturnType<typeof setTimeout> | null = null;

  function showPresetFeedback(msg: string) {
    presetFeedback = msg;
    if (presetFeedbackTimer) clearTimeout(presetFeedbackTimer);
    presetFeedbackTimer = setTimeout(() => { presetFeedback = null; }, 3000);
  }

  $effect(() => {
    if (savingPreset && presetSaveInput) {
      requestAnimationFrame(() => presetSaveInput?.focus());
    }
  });

  async function applyPreset(presetName: string) {
    if (!channelId) return;
    try {
      const next = await eqPresetApply(presetName, channelId);
      engineState.set(next);
      showPresetFeedback(`Applied "${presetName}"`);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      showPresetFeedback(`Apply failed: ${msg}`);
      console.error("[EqPage] eqPresetApply failed:", e);
    }
  }

  async function commitSavePreset() {
    const name = presetSaveName.trim();
    if (!name || !channelId) return;
    savingPreset = false;
    presetSaveName = "";
    try {
      const next = await eqPresetSave(name, channelId);
      engineState.set(next);
      showPresetFeedback(`Saved preset "${name}"`);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      showPresetFeedback(`Save failed: ${msg}`);
      console.error("[EqPage] eqPresetSave failed:", e);
    }
  }

  async function deletePreset(presetName: string) {
    const confirmed = window.confirm(`Delete preset "${presetName}"?`);
    if (!confirmed) return;
    try {
      const next = await eqPresetDelete(presetName);
      engineState.set(next);
      showPresetFeedback(`Deleted "${presetName}"`);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      showPresetFeedback(`Delete failed: ${msg}`);
      console.error("[EqPage] eqPresetDelete failed:", e);
    }
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

  <!-- ===== EQ Graph (hero) ===== -->
  <div class="eq-canvas-card">
    <div class="canvas-area">
      <EqGraph {channelId} {bands} selectedIndex={selectedBandIndex}
        onBandChange={handleBandChange} onSelect={handleSelect} />
    </div>
    <div class="gesture-hint" aria-hidden="true">
      <span>Drag = freq / gain</span><span class="hint-sep">·</span>
      <span>Scroll = Q</span><span class="hint-sep">·</span>
      <span>Dbl-click = flatten band</span><span class="hint-sep">·</span>
      <span>Arrows = nudge · Alt+↑↓ = Q</span>
    </div>
  </div>

  <!-- ===== Band detail row: list + panel ===== -->
  <div class="eq-detail-row">
    <div class="band-list-card">
      <div class="card-header">
        <h2 class="card-title">BANDS</h2>
        <span class="band-count">{bands.length}</span>
        <button class="flatten-btn" onclick={flattenAll}>Flatten</button>
      </div>
      <BandList {bands} selectedIndex={selectedBandIndex} onSelectBand={handleSelect} />
    </div>
    <div class="band-list-card">
      <EqBandPanel band={bands[selectedBandIndex] ?? null} index={selectedBandIndex}
        onBandChange={handleBandChange} onFlush={flushBand} />
    </div>
  </div>

  <!-- ===== EQ Presets ===== -->
  <div class="preset-card">
    <div class="card-header">
      <h2 class="card-title">PRESETS</h2>
      {#if eqPresets.length > 0}
        <span class="band-count">{eqPresets.length}</span>
      {/if}
      {#if presetFeedback}
        <span class="preset-feedback" role="status" aria-live="polite">{presetFeedback}</span>
      {/if}
    </div>

    {#if eqPresets.length > 0}
      <div class="preset-list" role="list">
        {#each eqPresets as preset}
          <div class="preset-row" role="listitem">
            <span class="preset-name" title="{preset.name} ({preset.band_count} bands)">
              {preset.name}
            </span>
            <span class="preset-meta">{preset.band_count}b</span>
            <div class="preset-actions">
              <button
                class="preset-btn"
                onclick={() => applyPreset(preset.name)}
                title="Apply to {currentChannelLabel}"
                aria-label="Apply preset {preset.name} to {currentChannelLabel}"
              >
                Apply
              </button>
              <button
                class="preset-btn danger"
                onclick={() => deletePreset(preset.name)}
                title="Delete preset {preset.name}"
                aria-label="Delete preset {preset.name}"
              >
                ✕
              </button>
            </div>
          </div>
        {/each}
      </div>
    {:else}
      <p class="preset-empty">No saved presets. Save the current channel EQ as a named preset.</p>
    {/if}

    <!-- Save current EQ as preset -->
    <div class="preset-save-area">
      {#if savingPreset}
        <div class="preset-save-row">
          <input
            bind:this={presetSaveInput}
            bind:value={presetSaveName}
            class="preset-save-input"
            type="text"
            placeholder="Preset name…"
            maxlength={48}
            aria-label="New preset name"
            onkeydown={(e) => {
              if (e.key === "Enter") { e.preventDefault(); commitSavePreset(); }
              else if (e.key === "Escape") { savingPreset = false; presetSaveName = ""; }
            }}
          />
          <button
            class="preset-confirm-btn"
            onclick={commitSavePreset}
            disabled={!presetSaveName.trim()}
            aria-label="Save preset"
          >
            Save
          </button>
          <button
            class="preset-cancel-btn"
            onclick={() => { savingPreset = false; presetSaveName = ""; }}
            aria-label="Cancel"
          >
            Cancel
          </button>
        </div>
      {:else}
        <button
          class="preset-save-trigger"
          onclick={() => { savingPreset = true; }}
          aria-label="Save current EQ as preset"
        >
          <span class="action-icon" aria-hidden="true">+</span>
          Save current EQ as preset…
        </button>
      {/if}
    </div>
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

  /* ===== Band detail row ===== */
  .eq-detail-row {
    display: flex;
    gap: var(--ss-space-4);
    flex-wrap: wrap;
  }

  /* ===== Band list card ===== */
  .band-list-card {
    background: var(--ss-surface-1);
    border: 1px solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    padding: var(--ss-space-4) var(--ss-space-4) var(--ss-space-3);
    box-shadow: var(--ss-e1);
    flex: 1;
    min-width: 220px;
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

  /* ===== Flatten button (mirrors .preset-btn) ===== */
  .flatten-btn {
    margin-left: auto;
    height: 22px;
    padding: 0 var(--ss-space-2);
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-xs);
    background: transparent;
    color: var(--ss-text-tertiary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    cursor: pointer;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .flatten-btn:hover {
    background: var(--ss-accent-soft);
    color: var(--ss-accent);
    border-color: var(--ss-accent-border);
  }

  .flatten-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 1px;
  }

  /* ===== Preset card ===== */
  .preset-card {
    background: var(--ss-surface-1);
    border: 1px solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    padding: var(--ss-space-4) var(--ss-space-4) var(--ss-space-3);
    box-shadow: var(--ss-e1);
    flex-shrink: 0;
  }

  .preset-feedback {
    margin-left: auto;
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-accent);
  }

  .preset-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-bottom: var(--ss-space-3);
  }

  .preset-row {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    padding: var(--ss-space-1) var(--ss-space-2);
    border-radius: var(--ss-radius-xs);
    transition: background var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .preset-row:hover {
    background: var(--ss-surface-2);
  }

  .preset-name {
    flex: 1;
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .preset-meta {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    flex-shrink: 0;
  }

  .preset-actions {
    display: flex;
    gap: var(--ss-space-1);
    flex-shrink: 0;
  }

  .preset-btn {
    height: 22px;
    padding: 0 var(--ss-space-2);
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-xs);
    background: transparent;
    color: var(--ss-text-tertiary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    cursor: pointer;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .preset-btn:hover {
    background: var(--ss-accent-soft);
    color: var(--ss-accent);
    border-color: var(--ss-accent-border);
  }

  .preset-btn.danger:hover {
    background: rgba(229, 72, 77, 0.15);
    color: var(--ss-danger, #e5484d);
    border-color: rgba(229, 72, 77, 0.4);
  }

  .preset-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 1px;
  }

  .preset-empty {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    margin: 0 0 var(--ss-space-3);
    padding: var(--ss-space-2);
    text-align: center;
  }

  /* ===== Save preset form ===== */
  .preset-save-area {
    border-top: 1px solid var(--ss-border);
    padding-top: var(--ss-space-3);
    margin-top: var(--ss-space-1);
  }

  .preset-save-trigger {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    background: transparent;
    border: none;
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-tertiary);
    cursor: pointer;
    padding: var(--ss-space-1) 0;
    transition: color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .preset-save-trigger:hover {
    color: var(--ss-text-secondary);
  }

  .preset-save-trigger:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
    border-radius: 2px;
  }

  .action-icon {
    color: var(--ss-accent);
    font-size: 14px;
    font-weight: 700;
    line-height: 1;
    width: 14px;
    text-align: center;
  }

  .preset-save-row {
    display: flex;
    gap: var(--ss-space-2);
    align-items: center;
  }

  .preset-save-input {
    flex: 1;
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-2);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    min-width: 0;
  }

  .preset-save-input:focus {
    outline: none;
    border-color: var(--ss-accent-border);
  }

  .preset-confirm-btn {
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-3);
    background: var(--ss-gradient-primary);
    border: none;
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-bright);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-button-size);
    font-weight: var(--ss-type-button-weight);
    cursor: pointer;
    white-space: nowrap;
    transition: opacity var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .preset-confirm-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .preset-confirm-btn:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .preset-confirm-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .preset-cancel-btn {
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-2);
    background: transparent;
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-tertiary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    cursor: pointer;
    white-space: nowrap;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .preset-cancel-btn:hover {
    background: var(--ss-surface-2);
    color: var(--ss-text-secondary);
  }
</style>
