<script lang="ts">
  /**
   * MicPage.svelte — Mic DSP chain tuning page.
   *
   * Thin view: all logic is in mic.ts (stageWireName, stagePluginPath,
   * isStageDisabled, stageUnavailableTooltip, micBandToArgs). Reuses EqCanvas
   * via the additive onFlush prop (E4) and BandList as-is.
   *
   * E8 NOTE: The live input meter is a static placeholder. EngineState carries
   * no level/metering data in this plan. A future plan must add telemetry.
   */

  import { engineState } from "../stores.js";
  import {
    micEnable,
    micStage,
    micSet,
    micEqBand,
    type MicStageSnapshot,
  } from "../ipc.js";
  import {
    stageWireName,
    isStageDisabled,
    stageUnavailableTooltip,
    micBandToArgs,
  } from "../mic.js";
  import EqCanvas from "./EqCanvas.svelte";
  import BandList from "./BandList.svelte";
  import { type Band } from "../eq.js";

  // ---------------------------------------------------------------------------
  // Derived mic state
  // ---------------------------------------------------------------------------

  const mic = $derived($engineState?.mic ?? null);

  function findStage(kind: string): MicStageSnapshot | undefined {
    return mic?.stages.find((s) => s.kind === kind);
  }

  const gainStage      = $derived(findStage("gain"));
  const highpassStage  = $derived(findStage("highpass"));
  const rnnoiseStage   = $derived(findStage("rnnoise"));
  const compStage      = $derived(findStage("compressor"));
  const gateStage      = $derived(findStage("gate"));
  const micEqStage     = $derived(findStage("mic_eq"));

  // Master enabled gates all stage cards
  const masterEnabled  = $derived(mic?.enabled ?? false);

  // ---------------------------------------------------------------------------
  // Mic EQ bands (mirroring EqPage pattern)
  // ---------------------------------------------------------------------------

  let micEqBands = $state<Band[]>([]);
  let selectedBandIndex = $state(0);

  $effect(() => {
    const eqBands = mic?.eq_bands ?? [];
    if (eqBands.length > 0) {
      micEqBands = eqBands.map((b) => ({
        kind: b.kind as Band["kind"],
        freqHz: b.freq_hz,
        q: b.q,
        gainDb: b.gain_db,
      }));
    }
  });

  // ---------------------------------------------------------------------------
  // Apply returned state to the store
  // ---------------------------------------------------------------------------

  function applyState(state: import("../ipc.js").EngineState) {
    engineState.set(state);
  }

  // ---------------------------------------------------------------------------
  // Handlers
  // ---------------------------------------------------------------------------

  function onMasterToggle(e: Event) {
    const on = (e.target as HTMLInputElement).checked;
    micEnable(on).then(applyState).catch((err) => {
      console.warn("[MicPage] micEnable failed:", err);
    });
  }

  function onStageToggle(kind: string, e: Event) {
    const on = (e.target as HTMLInputElement).checked;
    const wireName = stageWireName(kind);
    micStage(wireName, on).then(applyState).catch((err) => {
      console.warn(`[MicPage] micStage(${wireName}) failed:`, err);
    });
  }

  function onParamChange(param: string, e: Event) {
    const value = parseFloat((e.target as HTMLInputElement).value);
    if (isNaN(value)) return;
    micSet(param, value).then(applyState).catch((err) => {
      console.warn(`[MicPage] micSet(${param}) failed:`, err);
    });
  }

  function handleMicEqBandChange(index: number, band: Band) {
    micEqBands = micEqBands.map((b, i) => (i === index ? band : b));
  }

  function handleMicEqFlush(index: number, band: Band) {
    const args = micBandToArgs(index, band);
    micEqBand(args.band, args.kind, args.freq_hz, args.q, args.gain_db)
      .then(applyState)
      .catch((err) => console.warn("[MicPage] micEqBand failed:", err));
  }

  function handleSelectBand(index: number) {
    selectedBandIndex = index;
  }

  // ---------------------------------------------------------------------------
  // Helpers for param display from snapshot
  // ---------------------------------------------------------------------------

  function paramVal(stage: MicStageSnapshot | undefined, key: string): number {
    return stage?.params?.[key] ?? 0;
  }
</script>

<div class="mic-page">
  <!-- ===== Page header ===== -->
  <div class="page-header">
    <h1 class="page-title">MIC</h1>
    <p class="page-subtitle">Noise suppression · EQ · Gain · Chain</p>
  </div>

  {#if !$engineState}
    <div class="state-card" role="status" aria-live="polite">
      <div class="state-icon connecting-icon" aria-hidden="true">◎</div>
      <div class="state-body">
        <span class="state-title">Connecting to Daemon…</span>
        <span class="state-desc">Waiting for state from the Arctis daemon.</span>
      </div>
    </div>
  {:else}
    <!-- ===== Master enable ===== -->
    <div class="device-card device-card--live">
      <div class="card-header">
        <span class="card-icon" aria-hidden="true">⏺</span>
        <h2 class="card-title">MIC CHAIN</h2>
        <span class="pill {masterEnabled ? 'pill--live' : 'pill--coming'}">
          {masterEnabled ? "ACTIVE" : "DISABLED"}
        </span>
      </div>
      <div class="card-body">
        <div class="control-row">
          <span class="field-label">ENABLE MIC DSP CHAIN</span>
          <label class="toggle" title="Enable or disable the entire mic DSP chain">
            <input
              type="checkbox"
              class="toggle-input"
              checked={masterEnabled}
              onchange={onMasterToggle}
              aria-label="Enable mic DSP chain"
            />
            <span class="toggle-track">
              <span class="toggle-thumb"></span>
            </span>
          </label>
        </div>
        <!-- E8: Live input meter placeholder (no level data in EngineState) -->
        <div class="field-row meter-placeholder">
          <span class="field-label">INPUT LEVEL</span>
          <div class="meter-bar-wrap" aria-hidden="true">
            <div class="meter-bar meter-bar--static"></div>
          </div>
          <span class="meter-coming-soon">level metering coming soon</span>
        </div>
      </div>
    </div>

    <!-- Stage cards — dimmed when master is off -->
    <div class="controls-layout" class:controls-layout--dimmed={!masterEnabled} aria-hidden={!masterEnabled}>

      <!-- ─── GAIN card ─────────────────────────────────────────────────── -->
      {#if gainStage}
        <div
          class="device-card device-card--live"
          class:device-card--disabled={isStageDisabled(gainStage)}
          title={stageUnavailableTooltip(gainStage)}
        >
          <div class="card-header">
            <span class="card-icon" aria-hidden="true">◈</span>
            <h2 class="card-title">GAIN</h2>
            <label class="toggle toggle--sm" title={stageUnavailableTooltip(gainStage)}>
              <input
                type="checkbox"
                class="toggle-input"
                checked={gainStage.enabled}
                disabled={isStageDisabled(gainStage) || !masterEnabled}
                onchange={(e) => onStageToggle("gain", e)}
                aria-label="Enable gain stage"
              />
              <span class="toggle-track"><span class="toggle-thumb"></span></span>
            </label>
          </div>
          <div
            class="card-body"
            class:field-row--disabled={isStageDisabled(gainStage)}
          >
            <div class="control-row">
              <span class="field-label">GAIN (dB)</span>
              <div class="slider-group">
                <input
                  type="range"
                  class="ss-slider"
                  min="-20"
                  max="20"
                  step="0.5"
                  value={paramVal(gainStage, "gain_db")}
                  disabled={!gainStage.enabled || isStageDisabled(gainStage) || !masterEnabled}
                  onchange={(e) => onParamChange("gain_db", e)}
                  aria-label="Gain dB"
                />
                <span class="slider-readout">{paramVal(gainStage, "gain_db").toFixed(1)} dB</span>
              </div>
            </div>
          </div>
        </div>
      {/if}

      <!-- ─── HIGH-PASS card ────────────────────────────────────────────── -->
      {#if highpassStage}
        <div
          class="device-card device-card--live"
          class:device-card--disabled={isStageDisabled(highpassStage)}
          title={stageUnavailableTooltip(highpassStage)}
        >
          <div class="card-header">
            <span class="card-icon" aria-hidden="true">〰</span>
            <h2 class="card-title">HIGH-PASS</h2>
            <label class="toggle toggle--sm">
              <input
                type="checkbox"
                class="toggle-input"
                checked={highpassStage.enabled}
                disabled={isStageDisabled(highpassStage) || !masterEnabled}
                onchange={(e) => onStageToggle("highpass", e)}
                aria-label="Enable high-pass filter"
              />
              <span class="toggle-track"><span class="toggle-thumb"></span></span>
            </label>
          </div>
          <div
            class="card-body"
            class:field-row--disabled={isStageDisabled(highpassStage)}
          >
            <div class="control-row">
              <span class="field-label">CUTOFF (Hz)</span>
              <div class="slider-group">
                <input
                  type="range"
                  class="ss-slider"
                  min="20"
                  max="400"
                  step="5"
                  value={paramVal(highpassStage, "freq_hz")}
                  disabled={!highpassStage.enabled || isStageDisabled(highpassStage) || !masterEnabled}
                  onchange={(e) => onParamChange("highpass_freq", e)}
                  aria-label="High-pass cutoff Hz"
                />
                <span class="slider-readout">{Math.round(paramVal(highpassStage, "freq_hz"))} Hz</span>
              </div>
            </div>
          </div>
        </div>
      {/if}

      <!-- ─── RNNOISE card ───────────────────────────────────────────────── -->
      {#if rnnoiseStage}
        <div
          class="device-card device-card--live"
          class:device-card--disabled={isStageDisabled(rnnoiseStage)}
          title={stageUnavailableTooltip(rnnoiseStage)}
        >
          <div class="card-header">
            <span class="card-icon" aria-hidden="true">◉</span>
            <h2 class="card-title">NOISE SUPPRESSION</h2>
            <label class="toggle toggle--sm" title={stageUnavailableTooltip(rnnoiseStage)}>
              <input
                type="checkbox"
                class="toggle-input"
                checked={rnnoiseStage.enabled}
                disabled={isStageDisabled(rnnoiseStage) || !masterEnabled}
                onchange={(e) => onStageToggle("rnnoise", e)}
                aria-label="Enable noise suppression"
              />
              <span class="toggle-track"><span class="toggle-thumb"></span></span>
            </label>
          </div>
          <div
            class="card-body"
            class:controls-layout--dimmed={isStageDisabled(rnnoiseStage)}
          >
            <div class="control-row">
              <span class="field-label">VAD THRESHOLD (%)</span>
              <div class="slider-group">
                <input
                  type="range"
                  class="ss-slider"
                  min="0"
                  max="100"
                  step="1"
                  value={paramVal(rnnoiseStage, "vad_threshold")}
                  disabled={!rnnoiseStage.enabled || isStageDisabled(rnnoiseStage) || !masterEnabled}
                  onchange={(e) => onParamChange("vad_threshold", e)}
                  aria-label="VAD threshold percent"
                />
                <span class="slider-readout">{Math.round(paramVal(rnnoiseStage, "vad_threshold"))}%</span>
              </div>
            </div>
            <div class="control-row">
              <span class="field-label">VAD GRACE (ms)</span>
              <div class="slider-group">
                <input
                  type="range"
                  class="ss-slider"
                  min="0"
                  max="2000"
                  step="50"
                  value={paramVal(rnnoiseStage, "vad_grace_ms")}
                  disabled={!rnnoiseStage.enabled || isStageDisabled(rnnoiseStage) || !masterEnabled}
                  onchange={(e) => onParamChange("vad_grace_ms", e)}
                  aria-label="VAD grace period ms"
                />
                <span class="slider-readout">{Math.round(paramVal(rnnoiseStage, "vad_grace_ms"))} ms</span>
              </div>
            </div>
            <div class="field-row">
              <span class="field-label--hint">
                Lower threshold = less suppression, less tinny
              </span>
            </div>
          </div>
        </div>
      {/if}

      <!-- ─── COMPRESSOR card ──────────────────────────────────────────── -->
      {#if compStage}
        <div
          class="device-card device-card--live"
          class:device-card--disabled={isStageDisabled(compStage)}
          title={stageUnavailableTooltip(compStage)}
        >
          <div class="card-header">
            <span class="card-icon" aria-hidden="true">◎</span>
            <h2 class="card-title">COMPRESSOR</h2>
            <label class="toggle toggle--sm" title={stageUnavailableTooltip(compStage)}>
              <input
                type="checkbox"
                class="toggle-input"
                checked={compStage.enabled}
                disabled={isStageDisabled(compStage) || !masterEnabled}
                onchange={(e) => onStageToggle("compressor", e)}
                aria-label="Enable compressor"
              />
              <span class="toggle-track"><span class="toggle-thumb"></span></span>
            </label>
          </div>
          <div
            class="card-body"
            class:controls-layout--dimmed={isStageDisabled(compStage)}
          >
            <div class="control-row">
              <span class="field-label">THRESHOLD (dB)</span>
              <div class="slider-group">
                <input
                  type="range"
                  class="ss-slider"
                  min="-60"
                  max="0"
                  step="1"
                  value={paramVal(compStage, "threshold_db")}
                  disabled={!compStage.enabled || isStageDisabled(compStage) || !masterEnabled}
                  onchange={(e) => onParamChange("comp_threshold_db", e)}
                  aria-label="Compressor threshold dB"
                />
                <span class="slider-readout">{paramVal(compStage, "threshold_db").toFixed(1)} dB</span>
              </div>
            </div>
            <div class="control-row">
              <span class="field-label">RATIO</span>
              <div class="slider-group">
                <input
                  type="range"
                  class="ss-slider"
                  min="1"
                  max="20"
                  step="0.5"
                  value={paramVal(compStage, "ratio")}
                  disabled={!compStage.enabled || isStageDisabled(compStage) || !masterEnabled}
                  onchange={(e) => onParamChange("comp_ratio", e)}
                  aria-label="Compressor ratio"
                />
                <span class="slider-readout">{paramVal(compStage, "ratio").toFixed(1)}:1</span>
              </div>
            </div>
            <div class="control-row">
              <span class="field-label">MAKEUP (dB)</span>
              <div class="slider-group">
                <input
                  type="range"
                  class="ss-slider"
                  min="0"
                  max="24"
                  step="0.5"
                  value={paramVal(compStage, "makeup_db")}
                  disabled={!compStage.enabled || isStageDisabled(compStage) || !masterEnabled}
                  onchange={(e) => onParamChange("comp_makeup_db", e)}
                  aria-label="Compressor makeup gain dB"
                />
                <span class="slider-readout">{paramVal(compStage, "makeup_db").toFixed(1)} dB</span>
              </div>
            </div>
          </div>
        </div>
      {/if}

      <!-- ─── NOISE GATE card ──────────────────────────────────────────── -->
      {#if gateStage}
        <div
          class="device-card device-card--live"
          class:device-card--disabled={isStageDisabled(gateStage)}
          title={stageUnavailableTooltip(gateStage)}
        >
          <div class="card-header">
            <span class="card-icon" aria-hidden="true">▮</span>
            <h2 class="card-title">NOISE GATE</h2>
            <label class="toggle toggle--sm" title={stageUnavailableTooltip(gateStage)}>
              <input
                type="checkbox"
                class="toggle-input"
                checked={gateStage.enabled}
                disabled={isStageDisabled(gateStage) || !masterEnabled}
                onchange={(e) => onStageToggle("gate", e)}
                aria-label="Enable noise gate"
              />
              <span class="toggle-track"><span class="toggle-thumb"></span></span>
            </label>
          </div>
          <div
            class="card-body"
            class:field-row--disabled={isStageDisabled(gateStage)}
          >
            <div class="control-row">
              <span class="field-label">THRESHOLD</span>
              <div class="slider-group">
                <input
                  type="range"
                  class="ss-slider"
                  min="0"
                  max="1"
                  step="0.01"
                  value={paramVal(gateStage, "threshold")}
                  disabled={!gateStage.enabled || isStageDisabled(gateStage) || !masterEnabled}
                  onchange={(e) => onParamChange("gate_threshold", e)}
                  aria-label="Noise gate threshold"
                />
                <span class="slider-readout">{paramVal(gateStage, "threshold").toFixed(2)}</span>
              </div>
            </div>
          </div>
        </div>
      {/if}

    </div><!-- /controls-layout -->

    <!-- ─── MIC EQ (full-width card) ──────────────────────────────────── -->
    {#if micEqStage}
      <div
        class="device-card device-card--live mic-eq-card"
        class:device-card--disabled={isStageDisabled(micEqStage)}
        class:controls-layout--dimmed={!masterEnabled}
        title={stageUnavailableTooltip(micEqStage)}
      >
        <div class="card-header">
          <span class="card-icon" aria-hidden="true">〰</span>
          <h2 class="card-title">MIC EQ</h2>
          <label class="toggle toggle--sm" title={stageUnavailableTooltip(micEqStage)}>
            <input
              type="checkbox"
              class="toggle-input"
              checked={micEqStage.enabled}
              disabled={isStageDisabled(micEqStage) || !masterEnabled}
              onchange={(e) => onStageToggle("mic_eq", e)}
              aria-label="Enable mic EQ"
            />
            <span class="toggle-track"><span class="toggle-thumb"></span></span>
          </label>
        </div>
        {#if micEqBands.length > 0}
          <div class="card-body mic-eq-body">
            <div class="canvas-area">
              <EqCanvas
                channelId="mic"
                bands={micEqBands}
                {selectedBandIndex}
                onBandChange={handleMicEqBandChange}
                onSelectBand={handleSelectBand}
                onFlush={handleMicEqFlush}
              />
            </div>
            <div class="band-list-wrap">
              <BandList
                channelId="mic"
                bands={micEqBands}
                {selectedBandIndex}
                onSelectBand={handleSelectBand}
                onBandChange={handleMicEqBandChange}
              />
            </div>
          </div>
        {:else}
          <div class="card-body">
            <div class="field-row">
              <span class="field-label field-label--hint">
                No EQ bands configured — enable the mic chain to load defaults.
              </span>
            </div>
          </div>
        {/if}
      </div>
    {/if}
  {/if}
</div>

<style>
  .mic-page {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-4);
  }

  /* ===== Page header ===== */
  .page-header {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-1);
  }

  .page-title {
    font-family: var(--ss-font-display);
    font-size: var(--ss-type-display-size);
    font-weight: var(--ss-type-display-weight);
    line-height: var(--ss-type-display-line-height);
    letter-spacing: var(--ss-type-display-letter-spacing);
    text-transform: var(--ss-type-display-transform);
    color: var(--ss-text-bright);
    margin: 0;
  }

  .page-subtitle {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    margin: 0;
  }

  /* ===== State card (connecting) ===== */
  .state-card {
    display: flex;
    align-items: flex-start;
    gap: var(--ss-space-4);
    padding: var(--ss-space-6);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    box-shadow: var(--ss-e1);
  }

  .state-icon {
    font-size: 28px;
    line-height: 1;
    flex-shrink: 0;
    color: var(--ss-text-tertiary);
    margin-top: 2px;
  }

  .connecting-icon {
    color: var(--ss-warning);
    animation: pulse 1.5s ease-in-out infinite;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50%       { opacity: 0.35; }
  }

  @media (prefers-reduced-motion: reduce) {
    .connecting-icon {
      animation: none;
    }
  }

  .state-body {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-2);
    flex: 1;
  }

  .state-title {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-h3-size);
    font-weight: var(--ss-type-h3-weight);
    color: var(--ss-text-primary);
    letter-spacing: var(--ss-type-h3-letter-spacing);
  }

  .state-desc {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-secondary);
  }

  /* ===== Controls layout (stage cards grid) ===== */
  .controls-layout {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
    gap: var(--ss-space-4);
  }

  .controls-layout--dimmed {
    opacity: 0.4;
    pointer-events: none;
    user-select: none;
  }

  /* ===== Device card (mirroring DevicePage) ===== */
  .device-card {
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    box-shadow: var(--ss-e1);
    overflow: hidden;
  }

  .device-card--live {
    border-color: var(--ss-border-strong);
  }

  .device-card--disabled {
    background: var(--ss-bg-base);
    opacity: 0.6;
    pointer-events: none;
    color: var(--ss-text-disabled);
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    padding: var(--ss-space-3) var(--ss-space-4);
    border-bottom: var(--ss-border-width) solid var(--ss-border);
    background: var(--ss-surface-2);
  }

  .card-icon {
    font-size: 14px;
    color: var(--ss-accent);
    line-height: 1;
    flex-shrink: 0;
  }

  .card-title {
    font-family: var(--ss-font-display);
    font-size: var(--ss-type-h2-size);
    font-weight: var(--ss-type-h2-weight);
    letter-spacing: var(--ss-type-h2-letter-spacing);
    text-transform: var(--ss-type-h2-transform);
    color: var(--ss-text-primary);
    margin: 0;
    flex: 1;
  }

  .card-body {
    display: flex;
    flex-direction: column;
  }

  /* ===== Pills ===== */
  .pill {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: var(--ss-type-micro-transform);
    padding: 2px var(--ss-space-2);
    border-radius: var(--ss-radius-pill);
    flex-shrink: 0;
  }

  .pill--live {
    color: var(--ss-accent);
    background: var(--ss-accent-soft);
  }

  .pill--coming {
    color: var(--ss-text-tertiary);
    background: rgba(255, 255, 255, 0.06);
  }

  /* ===== Field rows ===== */
  .field-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--ss-space-2) var(--ss-space-4);
    border-bottom: var(--ss-border-width) solid var(--ss-border);
    min-height: var(--ss-control-h);
    gap: var(--ss-space-3);
  }

  .field-row:last-child {
    border-bottom: none;
  }

  .field-row--disabled {
    opacity: 0.7;
  }

  .field-label {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-label-size);
    font-weight: var(--ss-type-label-weight);
    letter-spacing: var(--ss-type-label-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-secondary);
    flex-shrink: 0;
  }

  .field-label--hint {
    font-size: var(--ss-type-caption-size);
    font-weight: 400;
    text-transform: none;
    color: var(--ss-text-tertiary);
    letter-spacing: 0;
  }

  /* ===== Control rows ===== */
  .control-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--ss-space-3) var(--ss-space-4);
    border-bottom: var(--ss-border-width) solid var(--ss-border);
    gap: var(--ss-space-3);
    min-height: var(--ss-control-h);
  }

  .control-row:last-child {
    border-bottom: none;
  }

  /* ===== Toggle ===== */
  .toggle {
    display: inline-flex;
    align-items: center;
    cursor: pointer;
    flex-shrink: 0;
    gap: var(--ss-space-2);
  }

  .toggle-input {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
  }

  .toggle-track {
    display: inline-flex;
    align-items: center;
    width: 36px;
    height: 20px;
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-pill);
    border: 1px solid var(--ss-border);
    transition: background var(--ss-dur-fast) var(--ss-ease-standard),
                border-color var(--ss-dur-fast) var(--ss-ease-standard);
    position: relative;
  }

  .toggle-thumb {
    position: absolute;
    left: 2px;
    width: 14px;
    height: 14px;
    border-radius: var(--ss-radius-pill);
    background: var(--ss-text-tertiary);
    transition: transform var(--ss-dur-fast) var(--ss-ease-standard),
                background var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .toggle-input:checked + .toggle-track {
    background: var(--ss-accent);
    border-color: var(--ss-accent);
  }

  .toggle-input:checked + .toggle-track .toggle-thumb {
    transform: translateX(16px);
    background: white;
  }

  .toggle-input:disabled + .toggle-track {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .toggle-input:focus-visible + .toggle-track {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .toggle--sm .toggle-track {
    width: 32px;
    height: 18px;
  }

  .toggle--sm .toggle-thumb {
    width: 12px;
    height: 12px;
  }

  .toggle--sm .toggle-input:checked + .toggle-track .toggle-thumb {
    transform: translateX(14px);
  }

  /* ===== Slider ===== */
  .slider-group {
    display: flex;
    align-items: center;
    gap: var(--ss-space-3);
    flex: 1;
    justify-content: flex-end;
  }

  .ss-slider {
    -webkit-appearance: none;
    appearance: none;
    height: 4px;
    border-radius: var(--ss-radius-pill);
    background: var(--ss-surface-input);
    cursor: pointer;
    width: 120px;
    flex-shrink: 0;
    outline: none;
  }

  .ss-slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 16px;
    height: 16px;
    border-radius: var(--ss-radius-pill);
    background: var(--ss-text-bright);
    box-shadow: var(--ss-e1);
    cursor: pointer;
  }

  .ss-slider::-moz-range-thumb {
    width: 16px;
    height: 16px;
    border-radius: var(--ss-radius-pill);
    background: var(--ss-text-bright);
    box-shadow: var(--ss-e1);
    cursor: pointer;
    border: none;
  }

  .ss-slider:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .ss-slider:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .slider-readout {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-readout-size);
    font-weight: var(--ss-type-readout-weight);
    font-variant-numeric: tabular-nums;
    color: var(--ss-text-primary);
    min-width: 52px;
    text-align: right;
  }

  /* ===== Input meter placeholder (E8) ===== */
  .meter-placeholder {
    flex-direction: column;
    align-items: flex-start;
    gap: var(--ss-space-2);
    padding: var(--ss-space-3) var(--ss-space-4);
  }

  .meter-bar-wrap {
    width: 100%;
    height: 8px;
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-pill);
    overflow: hidden;
    opacity: 0.35;
  }

  .meter-bar--static {
    width: 0%;
    height: 100%;
    background: var(--ss-accent);
    border-radius: var(--ss-radius-pill);
  }

  .meter-coming-soon {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-disabled);
    font-style: italic;
  }

  /* ===== Mic EQ full-width card ===== */
  .mic-eq-card {
    grid-column: 1 / -1;
  }

  .mic-eq-body {
    display: flex;
    flex-direction: column;
    gap: 0;
  }

  .canvas-area {
    height: 240px;
    border-bottom: var(--ss-border-width) solid var(--ss-border);
    overflow: hidden;
  }

  .band-list-wrap {
    padding: var(--ss-space-2) 0;
  }
</style>
