<script lang="ts">
  /**
   * MicPage.svelte — Mic DSP chain tuning page.
   *
   * Thin view: all logic is in mic.ts (stageWireName, stagePluginPath,
   * isStageDisabled, stageUnavailableTooltip, micBandToArgs). Reuses EqGraph
   * via the additive onFlush prop and BandList as-is.
   *
   * E8 NOTE: The live input meter is a static placeholder. EngineState carries
   * no level/metering data in this plan. A future plan must add telemetry.
   */

  import { get } from "svelte/store";
  import { untrack } from "svelte";
  import { engineState } from "../stores.js";
  import { eqEditing } from "../stores/eqEditing.js";
  import {
    micEnable,
    micStage,
    micSet,
    micEqBand,
    micHwMic,
    micSuppressionBackend,
    micPresetApply,
    type MicStageSnapshot,
  } from "../ipc.js";
  import {
    stageWireName,
    isStageDisabled,
    stageUnavailableTooltip,
    micBandToArgs,
    backendLabel,
    backendAvailable,
  } from "../mic.js";
  import { findMicPresetDescription } from "./micPresetUtils.js";
  import EqGraph from "./EqGraph.svelte";
  import BandList from "./BandList.svelte";
  import LevelMeter from "./LevelMeter.svelte";
  import { type Band } from "../eq.js";
  import Switch from "../ui/Switch.svelte";
  import Slider from "../ui/Slider.svelte";
  import Select from "../ui/Select.svelte";
  import type { SelectOption } from "../ui/selectUtils.js";

  // ---------------------------------------------------------------------------
  // Derived mic state
  // ---------------------------------------------------------------------------

  const mic = $derived($engineState?.mic ?? null);

  function findStage(kind: string): MicStageSnapshot | undefined {
    return mic?.stages.find((s) => s.kind === kind);
  }

  const gainStage         = $derived(findStage("gain"));
  const highpassStage     = $derived(findStage("highpass"));
  const suppressionStage  = $derived(findStage("suppression"));
  const compStage         = $derived(findStage("compressor"));
  const gateStage         = $derived(findStage("gate"));
  const micEqStage        = $derived(findStage("mic_eq"));

  // Master enabled gates all stage cards
  const masterEnabled  = $derived(mic?.enabled ?? false);

  // Suppression backend state from the snapshot
  const suppressionBackend    = $derived(mic?.suppression_backend ?? "deep_filter");
  const availableBackends     = $derived(mic?.available_suppression_backends ?? []);

  // Pinned hardware mic source (null = auto)
  const hwMic = $derived(mic?.hw_mic ?? null);

  // Mic presets
  const micPresets = $derived($engineState?.mic_presets ?? []);

  // hw_mic picker local state
  let hwMicInput = $state("");
  let settingHwMic = $state(false);
  let hwMicError = $state<string | null>(null);

  // Preset picker local state
  let selectedPreset = $state("");
  let applyingPreset = $state(false);
  let presetError = $state<string | null>(null);

  const selectedPresetDescription = $derived(
    findMicPresetDescription(selectedPreset, micPresets),
  );

  // Suppression-backend options. Unavailable backends keep an "(not installed)"
  // suffix (onBackendChange refuses to switch to them — see guard above).
  const backendOptions = $derived<SelectOption[]>(
    ["deep_filter", "rnnoise"].map((b) => ({
      value: b,
      label: backendLabel(b) + (backendAvailable(b, availableBackends) ? "" : " (not installed)"),
    })),
  );

  // Preset picker options, with a leading placeholder for the empty selection.
  const presetOptions = $derived<SelectOption[]>([
    { value: "", label: "— choose a preset —" },
    ...micPresets.map((p) => ({ value: p.name, label: p.name })),
  ]);

  $effect(() => {
    // Keep local input in sync when state arrives (only if not currently editing)
    if (!settingHwMic) {
      hwMicInput = hwMic ?? "";
    }
  });

  // ---------------------------------------------------------------------------
  // Mic EQ bands (mirroring EqPage pattern)
  // ---------------------------------------------------------------------------

  let micEqBands = $state<Band[]>([]);
  let selectedBandIndex = $state(0);

  $effect(() => {
    if (get(eqEditing)) return;
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

  function onMasterToggle(on: boolean) {
    micEnable(on).then(applyState).catch((err) => {
      console.warn("[MicPage] micEnable failed:", err);
    });
  }

  function onStageToggle(kind: string, on: boolean) {
    const wireName = stageWireName(kind);
    micStage(wireName, on).then(applyState).catch((err) => {
      console.warn(`[MicPage] micStage(${wireName}) failed:`, err);
    });
  }

  function onBackendChange(backend: string) {
    if (!backendAvailable(backend, availableBackends)) return; // never switch to an uninstalled backend
    micSuppressionBackend(backend).then(applyState).catch((err) => {
      console.warn(`[MicPage] micSuppressionBackend(${backend}) failed:`, err);
    });
  }

  // ---------------------------------------------------------------------------
  // Slider param drafts — local mirror so each slider thumb + readout track
  // during a drag (engine values only update after a successful commit). The
  // draft is keyed by the backend param name. We resync a key from the snapshot
  // ONLY when its engine value actually changes (i.e. a real state update lands),
  // so polling never clobbers a value the user is mid-drag on, and there's no
  // flicker between release and the commit round-trip.
  // ---------------------------------------------------------------------------

  let draft = $state<Record<string, number>>({
    gain_db: 0,
    highpass_freq: 0,
    attenuation_limit_db: 0,
    vad_threshold: 0,
    vad_grace_ms: 0,
    vad_retro_grace_ms: 0,
    comp_threshold_db: 0,
    comp_ratio: 0,
    comp_makeup_db: 0,
    gate_threshold: 0,
  });

  // Last engine value we synced per key (plain, non-reactive mirror).
  const lastSynced: Record<string, number> = {};

  $effect(() => {
    const next: Record<string, number> = {
      gain_db: paramVal(gainStage, "gain_db"),
      highpass_freq: paramVal(highpassStage, "freq_hz"),
      attenuation_limit_db: paramVal(suppressionStage, "attenuation_limit_db"),
      vad_threshold: paramVal(suppressionStage, "vad_threshold"),
      vad_grace_ms: paramVal(suppressionStage, "vad_grace_ms"),
      vad_retro_grace_ms: paramVal(suppressionStage, "vad_retro_grace_ms"),
      comp_threshold_db: paramVal(compStage, "threshold_db"),
      comp_ratio: paramVal(compStage, "ratio"),
      comp_makeup_db: paramVal(compStage, "makeup_db"),
      gate_threshold: paramVal(gateStage, "threshold"),
    };
    untrack(() => {
      const merged = { ...draft };
      let changed = false;
      for (const key of Object.keys(next)) {
        if (next[key] !== lastSynced[key]) {
          lastSynced[key] = next[key];
          merged[key] = next[key];
          changed = true;
        }
      }
      if (changed) draft = merged;
    });
  });

  /** Update the draft for a param during a drag (drives thumb + readout). */
  function setParamDraft(param: string, value: number) {
    draft = { ...draft, [param]: value };
  }

  /** Commit a param to the backend on pointer-up / keyboard commit. */
  function commitParam(param: string, value: number) {
    draft = { ...draft, [param]: value };
    if (isNaN(value)) return;
    micSet(param, value).then(applyState).catch((err) => {
      console.warn(`[MicPage] micSet(${param}) failed:`, err);
    });
  }

  async function onHwMicSet() {
    const device = hwMicInput.trim() || null;
    if (settingHwMic) return;
    settingHwMic = true;
    hwMicError = null;
    try {
      const next = await micHwMic(device);
      applyState(next);
    } catch (e) {
      hwMicError = e instanceof Error ? e.message : "Failed to set hw mic";
    } finally {
      settingHwMic = false;
    }
  }

  async function onHwMicClear() {
    if (settingHwMic) return;
    settingHwMic = true;
    hwMicError = null;
    hwMicInput = "";
    try {
      const next = await micHwMic(null);
      applyState(next);
    } catch (e) {
      hwMicError = e instanceof Error ? e.message : "Failed to clear hw mic";
    } finally {
      settingHwMic = false;
    }
  }

  function onHwMicKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      onHwMicSet();
    }
  }

  async function onPresetApply() {
    if (!selectedPreset || applyingPreset) return;
    applyingPreset = true;
    presetError = null;
    try {
      const next = await micPresetApply(selectedPreset);
      applyState(next);
    } catch (e) {
      presetError = e instanceof Error ? e.message : "Failed to apply mic preset";
    } finally {
      applyingPreset = false;
    }
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
          <span title="Enable or disable the entire mic DSP chain">
            <Switch
              checked={masterEnabled}
              onCheckedChange={onMasterToggle}
              ariaLabel="Enable mic DSP chain"
            />
          </span>
        </div>
        <!-- R3b: Live signal-peak meter via pw-record PCM capture.
             Shows real PCM peak (0 = silence, 1 = full scale).
             When daemon / arctis_clean_mic node is absent → 0, dim. -->
        <div class="field-row meter-row">
          <span class="field-label">INPUT LEVEL</span>
          <div class="meter-wrap">
            <LevelMeter
              nodeName="arctis_clean_mic"
              orientation="horizontal"
              ariaLabel="Mic input signal level"
            />
            <span class="meter-note">signal peak</span>
          </div>
        </div>
      </div>
    </div>

    <!-- ===== Config row: hardware source + presets (side-by-side on wide) ===== -->
    <div class="mic-config-row">
    <!-- ===== Hardware mic source picker ===== -->
    <div class="device-card device-card--live">
      <div class="card-header">
        <span class="card-icon" aria-hidden="true">◈</span>
        <h2 class="card-title">HARDWARE MIC SOURCE</h2>
        {#if hwMic}
          <span class="pill pill--live">PINNED</span>
        {:else}
          <span class="pill pill--coming">AUTO</span>
        {/if}
      </div>
      <div class="card-body">
        <div class="control-row">
          <span class="field-label">NODE NAME</span>
          <div class="hw-mic-group">
            <input
              type="text"
              class="route-input"
              placeholder="auto (leave blank)"
              value={hwMicInput}
              oninput={(e) => { hwMicInput = (e.target as HTMLInputElement).value; }}
              disabled={settingHwMic}
              onkeydown={onHwMicKeydown}
              aria-label="Hardware mic capture source node name"
              autocomplete="off"
              spellcheck={false}
            />
            <button
              class="hw-mic-set-btn"
              disabled={settingHwMic}
              onclick={onHwMicSet}
              aria-label="Pin hardware mic source"
            >
              {settingHwMic ? "…" : "Set"}
            </button>
            {#if hwMic}
              <button
                class="hw-mic-clear-btn"
                disabled={settingHwMic}
                onclick={onHwMicClear}
                aria-label="Clear pinned hardware mic source (use auto)"
              >
                Clear
              </button>
            {/if}
          </div>
        </div>
        {#if hwMicError}
          <div class="hw-mic-error" role="alert">{hwMicError}</div>
        {/if}
        <div class="field-row">
          <span class="field-label--hint">
            Pin to a specific PipeWire capture source node.name (e.g. <code>alsa_input.usb-SteelSeries_Arctis_Nova_Pro_Wireless_Game-00.mono-fallback</code>). Leave blank for auto.
          </span>
        </div>
      </div>
    </div>

    <!-- ===== Mic preset picker ===== -->
    {#if micPresets.length > 0}
      <div class="device-card device-card--live">
        <div class="card-header">
          <span class="card-icon" aria-hidden="true">◈</span>
          <h2 class="card-title">PRESETS</h2>
          {#if selectedPreset}
            <span class="pill pill--live">PRESET SELECTED</span>
          {:else}
            <span class="pill pill--coming">NONE</span>
          {/if}
        </div>
        <div class="card-body">
          <div class="control-row">
            <span class="field-label">PRESET</span>
            <div class="select-group">
              <div class="preset-select">
                <Select
                  options={presetOptions}
                  value={selectedPreset}
                  onValueChange={(v) => { selectedPreset = v; }}
                  disabled={applyingPreset}
                  ariaLabel="Select mic preset"
                />
              </div>
              <button
                class="preset-apply-btn"
                disabled={!selectedPreset || applyingPreset}
                onclick={onPresetApply}
                aria-label="Apply selected mic preset"
              >
                {applyingPreset ? "…" : "Apply"}
              </button>
            </div>
          </div>
          {#if selectedPresetDescription}
            <div class="field-row preset-desc-row">
              <span class="field-label--hint">{selectedPresetDescription}</span>
            </div>
          {/if}
          {#if presetError}
            <div class="preset-error" role="alert">{presetError}</div>
          {/if}
        </div>
      </div>
    {/if}
    </div><!-- /mic-config-row -->

    <!-- ===== Processing chain ===== -->
    <h2 class="section-label">Processing chain</h2>

    <!-- Stage cards — dimmed when master is off -->
    <div class="controls-layout" class:controls-layout--dimmed={!masterEnabled} inert={!masterEnabled || undefined}>

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
            <span title={stageUnavailableTooltip(gainStage)}>
              <Switch
                size="sm"
                checked={gainStage.enabled}
                disabled={isStageDisabled(gainStage) || !masterEnabled}
                onCheckedChange={(on) => onStageToggle("gain", on)}
                ariaLabel="Enable gain stage"
              />
            </span>
          </div>
          <div
            class="card-body"
            class:field-row--disabled={isStageDisabled(gainStage)}
          >
            <div class="control-row">
              <span class="field-label">GAIN (dB)</span>
              <div class="slider-group">
                <div class="slider-control">
                  <Slider
                    min={-20}
                    max={30}
                    step={0.5}
                    value={draft.gain_db}
                    disabled={!gainStage.enabled || isStageDisabled(gainStage) || !masterEnabled}
                    onValueChange={(v) => setParamDraft("gain_db", v)}
                    onValueCommit={(v) => commitParam("gain_db", v)}
                    ariaLabel="Gain dB"
                  />
                </div>
                <span class="slider-readout">{draft.gain_db.toFixed(1)} dB</span>
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
            <span>
              <Switch
                size="sm"
                checked={highpassStage.enabled}
                disabled={isStageDisabled(highpassStage) || !masterEnabled}
                onCheckedChange={(on) => onStageToggle("highpass", on)}
                ariaLabel="Enable high-pass filter"
              />
            </span>
          </div>
          <div
            class="card-body"
            class:field-row--disabled={isStageDisabled(highpassStage)}
          >
            <div class="control-row">
              <span class="field-label">CUTOFF (Hz)</span>
              <div class="slider-group">
                <div class="slider-control">
                  <Slider
                    min={20}
                    max={300}
                    step={5}
                    value={draft.highpass_freq}
                    disabled={!highpassStage.enabled || isStageDisabled(highpassStage) || !masterEnabled}
                    onValueChange={(v) => setParamDraft("highpass_freq", v)}
                    onValueCommit={(v) => commitParam("highpass_freq", v)}
                    ariaLabel="High-pass cutoff Hz"
                  />
                </div>
                <span class="slider-readout">{Math.round(draft.highpass_freq)} Hz</span>
              </div>
            </div>
          </div>
        </div>
      {/if}

      <!-- ─── NOISE SUPPRESSION card ───────────────────────────────────── -->
      {#if suppressionStage}
        <div
          class="device-card device-card--live"
          class:device-card--disabled={isStageDisabled(suppressionStage)}
          title={stageUnavailableTooltip(suppressionStage)}
        >
          <div class="card-header">
            <span class="card-icon" aria-hidden="true">◉</span>
            <h2 class="card-title">NOISE SUPPRESSION</h2>
            <span title={stageUnavailableTooltip(suppressionStage)}>
              <Switch
                size="sm"
                checked={suppressionStage.enabled}
                disabled={isStageDisabled(suppressionStage) || !masterEnabled}
                onCheckedChange={(on) => onStageToggle("suppression", on)}
                ariaLabel="Enable noise suppression"
              />
            </span>
          </div>
          <div
            class="card-body"
            class:controls-layout--dimmed={isStageDisabled(suppressionStage)}
          >
            <!-- Backend selector -->
            <div class="control-row">
              <span class="field-label">BACKEND</span>
              <div class="select-group">
                <div class="select-control">
                  <Select
                    options={backendOptions}
                    value={suppressionBackend}
                    disabled={!suppressionStage.enabled || isStageDisabled(suppressionStage) || !masterEnabled}
                    onValueChange={onBackendChange}
                    ariaLabel="Noise suppression backend"
                  />
                </div>
              </div>
            </div>

            <!-- DeepFilterNet controls -->
            {#if suppressionBackend === "deep_filter"}
              <div class="control-row">
                <span class="field-label">ATTENUATION LIMIT (dB)</span>
                <div class="slider-group">
                  <div class="slider-control">
                    <Slider
                      min={0}
                      max={100}
                      step={1}
                      value={draft.attenuation_limit_db}
                      disabled={!suppressionStage.enabled || isStageDisabled(suppressionStage) || !masterEnabled}
                      onValueChange={(v) => setParamDraft("attenuation_limit_db", v)}
                      onValueCommit={(v) => commitParam("attenuation_limit_db", v)}
                      ariaLabel="DeepFilterNet attenuation limit dB"
                    />
                  </div>
                  <span class="slider-readout">{Math.round(draft.attenuation_limit_db)} dB</span>
                </div>
              </div>
              <div class="field-row">
                <span class="field-label--hint">
                  Lower = less suppression / fewer artifacts (the anti-tinny control). Default 100 = maximum suppression.
                </span>
              </div>
            {/if}

            <!-- RNNoise controls -->
            {#if suppressionBackend === "rnnoise"}
              <div class="control-row">
                <span class="field-label">VAD THRESHOLD (%)</span>
                <div class="slider-group">
                  <div class="slider-control">
                    <Slider
                      min={0}
                      max={99}
                      step={1}
                      value={draft.vad_threshold}
                      disabled={!suppressionStage.enabled || isStageDisabled(suppressionStage) || !masterEnabled}
                      onValueChange={(v) => setParamDraft("vad_threshold", v)}
                      onValueCommit={(v) => commitParam("vad_threshold", v)}
                      ariaLabel="VAD threshold percent"
                    />
                  </div>
                  <span class="slider-readout">{Math.round(draft.vad_threshold)}%</span>
                </div>
              </div>
              <div class="control-row">
                <span class="field-label">VAD GRACE (ms)</span>
                <div class="slider-group">
                  <div class="slider-control">
                    <Slider
                      min={0}
                      max={1000}
                      step={50}
                      value={draft.vad_grace_ms}
                      disabled={!suppressionStage.enabled || isStageDisabled(suppressionStage) || !masterEnabled}
                      onValueChange={(v) => setParamDraft("vad_grace_ms", v)}
                      onValueCommit={(v) => commitParam("vad_grace_ms", v)}
                      ariaLabel="VAD grace period ms"
                    />
                  </div>
                  <span class="slider-readout">{Math.round(draft.vad_grace_ms)} ms</span>
                </div>
              </div>
              <div class="control-row">
                <span class="field-label">VAD RETRO GRACE (ms)</span>
                <div class="slider-group">
                  <div class="slider-control">
                    <Slider
                      min={0}
                      max={200}
                      step={10}
                      value={draft.vad_retro_grace_ms}
                      disabled={!suppressionStage.enabled || isStageDisabled(suppressionStage) || !masterEnabled}
                      onValueChange={(v) => setParamDraft("vad_retro_grace_ms", v)}
                      onValueCommit={(v) => commitParam("vad_retro_grace_ms", v)}
                      ariaLabel="VAD retro grace period ms"
                    />
                  </div>
                  <span class="slider-readout">{Math.round(draft.vad_retro_grace_ms)} ms</span>
                </div>
              </div>
              <div class="field-row">
                <span class="field-label--hint">
                  RNNoise can sound tinnier (no attenuation cap). Prefer DeepFilterNet for clean voice.
                  VAD affects word-clipping, not timbral tinniness. Retro grace pads audio before speech detection onset.
                </span>
              </div>
            {/if}
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
            <span title={stageUnavailableTooltip(compStage)}>
              <Switch
                size="sm"
                checked={compStage.enabled}
                disabled={isStageDisabled(compStage) || !masterEnabled}
                onCheckedChange={(on) => onStageToggle("compressor", on)}
                ariaLabel="Enable compressor"
              />
            </span>
          </div>
          <div
            class="card-body"
            class:controls-layout--dimmed={isStageDisabled(compStage)}
          >
            <div class="control-row">
              <span class="field-label">THRESHOLD (dB)</span>
              <div class="slider-group">
                <div class="slider-control">
                  <Slider
                    min={-30}
                    max={0}
                    step={1}
                    value={draft.comp_threshold_db}
                    disabled={!compStage.enabled || isStageDisabled(compStage) || !masterEnabled}
                    onValueChange={(v) => setParamDraft("comp_threshold_db", v)}
                    onValueCommit={(v) => commitParam("comp_threshold_db", v)}
                    ariaLabel="Compressor threshold dB"
                  />
                </div>
                <span class="slider-readout">{draft.comp_threshold_db.toFixed(1)} dB</span>
              </div>
            </div>
            <div class="control-row">
              <span class="field-label">RATIO</span>
              <div class="slider-group">
                <div class="slider-control">
                  <Slider
                    min={1}
                    max={20}
                    step={0.5}
                    value={draft.comp_ratio}
                    disabled={!compStage.enabled || isStageDisabled(compStage) || !masterEnabled}
                    onValueChange={(v) => setParamDraft("comp_ratio", v)}
                    onValueCommit={(v) => commitParam("comp_ratio", v)}
                    ariaLabel="Compressor ratio"
                  />
                </div>
                <span class="slider-readout">{draft.comp_ratio.toFixed(1)}:1</span>
              </div>
            </div>
            <div class="control-row">
              <span class="field-label">MAKEUP (dB)</span>
              <div class="slider-group">
                <div class="slider-control">
                  <Slider
                    min={0}
                    max={24}
                    step={0.5}
                    value={draft.comp_makeup_db}
                    disabled={!compStage.enabled || isStageDisabled(compStage) || !masterEnabled}
                    onValueChange={(v) => setParamDraft("comp_makeup_db", v)}
                    onValueCommit={(v) => commitParam("comp_makeup_db", v)}
                    ariaLabel="Compressor makeup gain dB"
                  />
                </div>
                <span class="slider-readout">{draft.comp_makeup_db.toFixed(1)} dB</span>
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
            <span title={stageUnavailableTooltip(gateStage)}>
              <Switch
                size="sm"
                checked={gateStage.enabled}
                disabled={isStageDisabled(gateStage) || !masterEnabled}
                onCheckedChange={(on) => onStageToggle("gate", on)}
                ariaLabel="Enable noise gate"
              />
            </span>
          </div>
          <div
            class="card-body"
            class:field-row--disabled={isStageDisabled(gateStage)}
          >
            <div class="control-row">
              <span class="field-label">THRESHOLD</span>
              <div class="slider-group">
                <div class="slider-control">
                  <Slider
                    min={0}
                    max={0.5}
                    step={0.01}
                    value={draft.gate_threshold}
                    disabled={!gateStage.enabled || isStageDisabled(gateStage) || !masterEnabled}
                    onValueChange={(v) => setParamDraft("gate_threshold", v)}
                    onValueCommit={(v) => commitParam("gate_threshold", v)}
                    ariaLabel="Noise gate threshold"
                  />
                </div>
                <span class="slider-readout">{draft.gate_threshold.toFixed(2)}</span>
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
        inert={!masterEnabled || undefined}
        title={stageUnavailableTooltip(micEqStage)}
      >
        <div class="card-header">
          <span class="card-icon" aria-hidden="true">〰</span>
          <h2 class="card-title">MIC EQ</h2>
          <span title={stageUnavailableTooltip(micEqStage)}>
            <Switch
              size="sm"
              checked={micEqStage.enabled}
              disabled={isStageDisabled(micEqStage) || !masterEnabled}
              onCheckedChange={(on) => onStageToggle("mic_eq", on)}
              ariaLabel="Enable mic EQ"
            />
          </span>
        </div>
        {#if micEqBands.length > 0}
          <div class="card-body mic-eq-body">
            <div class="mic-eq-graph">
              <EqGraph
                bands={micEqBands}
                selectedIndex={selectedBandIndex}
                onBandChange={handleMicEqBandChange}
                onSelect={handleSelectBand}
                onFlush={handleMicEqFlush}
              />
            </div>
            <div class="mic-eq-bands">
              <BandList
                bands={micEqBands}
                selectedIndex={selectedBandIndex}
                onSelectBand={handleSelectBand}
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
    gap: var(--ss-space-5);
  }

  /* ===== Section label ===== */
  .section-label {
    font-family: var(--ss-font-display);
    font-size: var(--ss-type-h3-size);
    font-weight: var(--ss-type-h3-weight);
    letter-spacing: var(--ss-type-h3-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-tertiary);
    margin: var(--ss-space-1) 0 calc(-1 * var(--ss-space-2));
  }

  /* ===== Config row: hardware source + presets ===== */
  .mic-config-row {
    display: flex;
    gap: var(--ss-space-5);
    flex-wrap: wrap;
    align-items: flex-start;
  }

  .mic-config-row > .device-card {
    flex: 1 1 340px;
    min-width: 0;
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
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: var(--ss-space-5);
    /* Each card keeps its natural height instead of stretching to the tallest
       in its row (which left short cards like GAIN with big empty gaps). */
    align-items: start;
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

  /* ===== Slider ===== */
  .slider-group {
    display: flex;
    align-items: center;
    gap: var(--ss-space-3);
    flex: 1;
    justify-content: flex-end;
  }

  /* Constrains the bits-ui Slider wrapper (which is width:100%). */
  .slider-control {
    width: 120px;
    flex-shrink: 0;
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

  /* ===== Select (backend picker) ===== */
  .select-group {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    flex: 1;
    justify-content: flex-end;
  }

  /* Constrains the bits-ui Select wrapper (which is width:100%). */
  .select-control {
    width: 160px;
    flex-shrink: 0;
  }

  /* ===== Input level meter (R3) ===== */
  .meter-row {
    flex-direction: column;
    align-items: flex-start;
    gap: var(--ss-space-2);
    padding: var(--ss-space-3) var(--ss-space-4);
  }

  .meter-wrap {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-1);
    width: 100%;
  }

  .meter-note {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-disabled);
    font-style: italic;
  }

  /* ===== HW mic picker ===== */
  .hw-mic-group {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    flex: 1;
    justify-content: flex-end;
  }

  .route-input {
    height: var(--ss-field-h);
    padding: 0 var(--ss-space-3);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-sm);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-body-size);
    flex: 1;
    min-width: 0;
    transition:
      border-color var(--ss-dur-fast) var(--ss-ease-standard),
      background var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .route-input:focus {
    outline: none;
    border-color: var(--ss-accent-border);
    background: var(--ss-surface-2);
  }

  .route-input:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .route-input::placeholder {
    color: var(--ss-text-disabled);
  }

  .hw-mic-set-btn {
    height: var(--ss-field-h);
    padding: 0 var(--ss-space-4);
    background: var(--ss-gradient-primary);
    border: none;
    border-radius: var(--ss-radius-sm);
    color: var(--ss-text-bright);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-button-size);
    font-weight: var(--ss-type-button-weight);
    letter-spacing: var(--ss-type-button-letter-spacing);
    text-transform: uppercase;
    cursor: pointer;
    white-space: nowrap;
    flex-shrink: 0;
    transition: opacity var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .hw-mic-set-btn:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .hw-mic-set-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .hw-mic-set-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .hw-mic-clear-btn {
    height: var(--ss-field-h);
    padding: 0 var(--ss-space-3);
    background: transparent;
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-sm);
    color: var(--ss-text-secondary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-button-size);
    font-weight: var(--ss-type-button-weight);
    letter-spacing: var(--ss-type-button-letter-spacing);
    text-transform: uppercase;
    cursor: pointer;
    white-space: nowrap;
    flex-shrink: 0;
    transition:
      color var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .hw-mic-clear-btn:hover:not(:disabled) {
    color: var(--ss-danger);
    border-color: var(--ss-danger);
  }

  .hw-mic-clear-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .hw-mic-clear-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .hw-mic-error {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-danger);
    margin: 0;
    padding: var(--ss-space-2) var(--ss-space-4);
    background: var(--ss-danger-soft);
    border-top: var(--ss-border-width) solid rgba(229, 72, 77, 0.3);
  }

  /* ===== Mic EQ full-width card ===== */
  .mic-eq-card {
    grid-column: 1 / -1;
  }

  .mic-eq-body {
    display: flex;
    flex-wrap: wrap;
    gap: var(--ss-space-5);
    padding: var(--ss-space-4);
    align-items: flex-start;
  }

  /* Graph: capped so it keeps a sane aspect ratio (the SVG stretches to fill,
     so an uncapped full-width box flattens the curve). Shares the row with the
     band list on wide screens; stacks on narrow. */
  .mic-eq-graph {
    flex: 1 1 440px;
    min-width: 0;
    max-width: 820px;
    height: 300px;
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-sm);
    overflow: hidden;
  }

  .mic-eq-bands {
    flex: 1 1 240px;
    min-width: 0;
  }

  /* ===== Preset picker ===== */
  .preset-select {
    flex: 1;
    min-width: 160px;
  }

  .preset-apply-btn {
    height: var(--ss-field-h);
    padding: 0 var(--ss-space-4);
    background: var(--ss-gradient-primary);
    border: none;
    border-radius: var(--ss-radius-sm);
    color: var(--ss-text-bright);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-button-size);
    font-weight: var(--ss-type-button-weight);
    letter-spacing: var(--ss-type-button-letter-spacing);
    text-transform: uppercase;
    cursor: pointer;
    white-space: nowrap;
    flex-shrink: 0;
    transition: opacity var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .preset-apply-btn:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .preset-apply-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .preset-apply-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .preset-desc-row {
    border-top: var(--ss-border-width) solid var(--ss-border);
  }

  .preset-error {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-danger);
    margin: 0;
    padding: var(--ss-space-2) var(--ss-space-4);
    background: var(--ss-danger-soft);
    border-top: var(--ss-border-width) solid rgba(229, 72, 77, 0.3);
  }
</style>
