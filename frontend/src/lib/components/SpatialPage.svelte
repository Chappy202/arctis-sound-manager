<script lang="ts">
  /**
   * SpatialPage.svelte — Virtual surround / HRIR page (F1.5).
   *
   * Thin view: all pure logic is in surround.ts (hrirDisplayName, channelChecked,
   * toggleChannel, buildSinkOptions, sinkSelectValue, sinkValueToHwSink).
   * IPC wrappers are in ipc.ts. Mirrors MicPage.svelte idioms.
   *
   * Controls:
   *   - Enable toggle  → surroundEnable (master gate; dims rest when off)
   *   - HRIR picker    → surroundSetHrir (dropdown; honest banner when no profiles)
   *   - Channels       → surroundSetChannels (checkbox per channel in state)
   *   - HW sink select → surroundSetHwSink (dropdown; Auto-detect = null)
   *   - Import HRIRs   → surroundImportHrirs (pick dir via dialog)
   *   - Download HRIRs → surroundFetchHrirs  (placeholder; surfaces informational error)
   *
   * Each call applies the returned EngineState to the store (.then(applyState)).
   */

  import { onMount } from "svelte";
  import { engineState } from "../stores.js";
  import {
    surroundEnable,
    surroundSetHrir,
    surroundSetChannels,
    surroundSetHwSink,
    surroundImportHrirs,
    surroundFetchHrirs,
    listOutputs,
    type OutputDeviceSnapshot,
  } from "../ipc.js";
  import {
    hrirDisplayName,
    channelChecked,
    toggleChannel,
    buildSinkOptions,
    sinkSelectValue,
    sinkValueToHwSink,
  } from "../surround.js";
  import Switch from "../ui/Switch.svelte";
  import Select from "../ui/Select.svelte";
  import type { SelectOption } from "../ui/selectUtils.js";

  // ---------------------------------------------------------------------------
  // Derived surround state
  // ---------------------------------------------------------------------------

  const surround = $derived($engineState?.surround ?? null);

  const masterEnabled     = $derived(surround?.enabled ?? false);
  const currentHrir       = $derived(surround?.hrir ?? null);
  const availableHrirs    = $derived(surround?.available_hrirs ?? []);
  const availableHrirEntries = $derived(surround?.available_hrir_entries ?? []);
  const activeChannels    = $derived(surround?.channels ?? []);
  // All channel ids known to the engine (from the channels list)
  const allChannelIds = $derived(($engineState?.channels ?? []).map((c) => c.id));

  // No HRIR profiles → honest unavailable state
  const noHrirs = $derived(availableHrirs.length === 0);

  // HRIR picker options. Prefer rich available_hrir_entries (display + group) when
  // available; fall back to stem-based hrirDisplayName for older engine builds.
  const hrirOptions = $derived<SelectOption[]>([
    ...(currentHrir === null && availableHrirs.length > 0
      ? [{ value: "", label: "(none selected)" }]
      : []),
    ...(availableHrirEntries.length > 0
      ? availableHrirEntries.map((e) => ({
          value: e.stem,
          label: e.group ? `${e.group} — ${e.display}` : e.display,
        }))
      : availableHrirs.map((stem) => ({ value: stem, label: hrirDisplayName(stem) }))),
  ]);

  // ---------------------------------------------------------------------------
  // Output device list (for the hardware-sink Select)
  // ---------------------------------------------------------------------------

  let outputs = $state<OutputDeviceSnapshot[]>([]);

  onMount(() => {
    listOutputs().then((o) => (outputs = o)).catch(() => {});
  });

  const sinkOptions = $derived(buildSinkOptions(outputs, null));

  // ---------------------------------------------------------------------------
  // Import / fetch HRIR feedback state
  // ---------------------------------------------------------------------------

  let importBusy = $state(false);
  let importMsg = $state<string | null>(null);

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
    surroundEnable(on).then(applyState).catch((err) => {
      console.warn("[SpatialPage] surroundEnable failed:", err);
    });
  }

  function onHrirChange(name: string) {
    if (!name) return; // ignore the placeholder "(none selected)" entry
    surroundSetHrir(name).then(applyState).catch((err) => {
      console.warn("[SpatialPage] surroundSetHrir failed:", err);
    });
  }

  function onChannelToggle(id: string) {
    const next = toggleChannel(id, activeChannels);
    surroundSetChannels(next).then(applyState).catch((err) => {
      console.warn(`[SpatialPage] surroundSetChannels(${id}) failed:`, err);
    });
  }

  async function onImportHrirs() {
    importBusy = true;
    importMsg = null;
    try {
      await surroundImportHrirs(null).then(applyState);
      importMsg = "Imported HRIR profiles.";
    } catch (err) {
      importMsg = String(err);
    } finally {
      importBusy = false;
    }
  }

  async function onFetchHrirs() {
    importBusy = true;
    importMsg = null;
    try {
      await surroundFetchHrirs().then(applyState);
    } catch (err) {
      // The daemon returns an informational error: automatic download is not yet
      // available. Surface it as a note rather than a crash.
      importMsg = String(err);
    } finally {
      importBusy = false;
    }
  }
</script>

<div class="spatial-page">
  <!-- ===== Page header ===== -->
  <div class="page-header">
    <h1 class="page-title">SPATIAL</h1>
    <p class="page-subtitle">Virtual surround · HRIR profiles · Channel routing</p>
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

    <!-- ===== No HRIR profiles banner ===== -->
    {#if noHrirs}
      <div class="banner banner--warn" role="alert">
        <span class="banner-icon" aria-hidden="true">◎</span>
        <div class="banner-body">
          <span class="banner-title">No HRIR profiles found</span>
          <span class="banner-desc">
            Drop HeSuVi <code>.wav</code> files into
            <code>~/.local/share/pipewire/hrir_hesuvi/profiles/</code>
            to enable virtual surround.
          </span>
        </div>
      </div>
    {/if}

    <!-- ===== HRIR import / fetch actions ===== -->
    <div class="device-card device-card--live">
      <div class="card-header">
        <span class="card-icon" aria-hidden="true">⤓</span>
        <h2 class="card-title">ADD HRIR PROFILES</h2>
      </div>
      <div class="card-body">
        <div class="control-row">
          <span class="field-label">IMPORT</span>
          <div class="hrir-actions">
            <button
              class="ss-btn"
              disabled={importBusy}
              onclick={onImportHrirs}
              aria-label="Import HRIR profiles from default directory"
            >
              {importBusy ? "Working…" : "Import my HRIRs"}
            </button>
            <button
              class="ss-btn"
              disabled={importBusy}
              onclick={onFetchHrirs}
              aria-label="Download HeSuVi HRIR set (not yet available)"
            >
              {importBusy ? "Working…" : "Download HeSuVi set"}
            </button>
          </div>
        </div>
        {#if importMsg}
          <div class="field-row">
            <span class="field-label--hint hrir-msg">{importMsg}</span>
          </div>
        {/if}
        <div class="field-row">
          <span class="field-label--hint">
            "Import" scans <code>~/.local/share/pipewire/hrir_hesuvi/profiles/</code> for
            HeSuVi 14-channel <code>.wav</code> files. "Download" is not yet available —
            import files manually for now.
          </span>
        </div>
      </div>
    </div>

    <!-- ===== Master enable ===== -->
    <div class="device-card device-card--live">
      <div class="card-header">
        <span class="card-icon" aria-hidden="true">◎</span>
        <h2 class="card-title">VIRTUAL SURROUND</h2>
        <span class="pill {masterEnabled ? 'pill--live' : 'pill--coming'}">
          {masterEnabled ? "ACTIVE" : "DISABLED"}
        </span>
      </div>
      <div class="card-body">
        <div class="control-row">
          <span class="field-label">ENABLE VIRTUAL SURROUND</span>
          <span
            title={noHrirs ? "No HRIR profiles found — add profiles to enable surround" : "Enable or disable virtual surround"}
          >
            <Switch
              checked={masterEnabled}
              disabled={noHrirs}
              onCheckedChange={onMasterToggle}
              ariaLabel="Enable virtual surround"
            />
          </span>
        </div>
      </div>
    </div>

    <!-- Controls — dimmed when master is off -->
    <div class="controls-layout" class:controls-layout--dimmed={!masterEnabled} inert={!masterEnabled || undefined}>

      <!-- ─── HRIR Picker ───────────────────────────────────────────────── -->
      <div class="device-card device-card--live">
        <div class="card-header">
          <span class="card-icon" aria-hidden="true">◈</span>
          <h2 class="card-title">HRIR PROFILE</h2>
        </div>
        <div class="card-body">
          <div class="control-row">
            <span class="field-label">PROFILE</span>
            <div class="select-group">
              <div class="select-control">
                <Select
                  options={hrirOptions}
                  value={currentHrir ?? ""}
                  disabled={noHrirs || !masterEnabled}
                  ariaLabel="Select HRIR profile"
                  onValueChange={onHrirChange}
                />
              </div>
            </div>
          </div>
          <div class="field-row">
            <span class="field-label--hint">
              Switching HRIR requires a surround sink recreate (~50 ms gap in audio).
              Drop <code>.wav</code> files into <code>~/.local/share/pipewire/hrir_hesuvi/profiles/</code>.
            </span>
          </div>
        </div>
      </div>

      <!-- ─── Channel routing ──────────────────────────────────────────── -->
      <div class="device-card device-card--live">
        <div class="card-header">
          <span class="card-icon" aria-hidden="true">⊞</span>
          <h2 class="card-title">CHANNELS</h2>
        </div>
        <div class="card-body">
          {#if allChannelIds.length === 0}
            <div class="field-row">
              <span class="field-label--hint">No channels configured.</span>
            </div>
          {:else}
            {#each allChannelIds as id (id)}
              <div class="control-row">
                <span class="field-label">{id.toUpperCase()}</span>
                <span title="Route {id} through virtual surround">
                  <Switch
                    size="sm"
                    checked={channelChecked(id, activeChannels)}
                    disabled={!masterEnabled}
                    onCheckedChange={() => onChannelToggle(id)}
                    ariaLabel="Route {id} through surround"
                  />
                </span>
              </div>
            {/each}
            <div class="field-row">
              <span class="field-label--hint">
                Checked channels are routed through the surround sink.
                Chat bypasses surround by default.
              </span>
            </div>
          {/if}
        </div>
      </div>

    </div><!-- /controls-layout -->

    <!-- ─── Advanced: hardware sink (full-width) ─────────────────────── -->
    <div
      class="device-card device-card--live"
      class:controls-layout--dimmed={!masterEnabled}
      inert={!masterEnabled || undefined}
    >
      <div class="card-header">
        <span class="card-icon" aria-hidden="true">▮</span>
        <h2 class="card-title">ADVANCED</h2>
      </div>
      <div class="card-body">
        <div class="control-row">
          <span class="field-label">HARDWARE SINK</span>
          <div class="select-group">
            <div class="select-control">
              <Select
                options={sinkOptions}
                value={sinkSelectValue(surround?.hw_sink)}
                onValueChange={(v) =>
                  surroundSetHwSink(sinkValueToHwSink(v))
                    .then(applyState)
                    .catch((err) => console.warn("[SpatialPage] surroundSetHwSink failed:", err))}
                disabled={!masterEnabled}
                ariaLabel="Hardware sink (Auto-detect or pick a device)"
              />
            </div>
          </div>
        </div>
        <div class="field-row">
          <span class="field-label--hint">
            PipeWire output device for the surround sink target.
            "Auto-detect" lets the engine pick the Arctis hardware sink automatically.
          </span>
        </div>
      </div>
    </div>

  {/if}
</div>

<style>
  .spatial-page {
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

  /* ===== No-HRIR banner ===== */
  .banner {
    display: flex;
    align-items: flex-start;
    gap: var(--ss-space-3);
    padding: var(--ss-space-4);
    border-radius: var(--ss-radius-md);
    border: var(--ss-border-width) solid;
  }

  .banner--warn {
    background: rgba(255, 180, 0, 0.08);
    border-color: var(--ss-warning);
  }

  .banner-icon {
    font-size: 18px;
    color: var(--ss-warning);
    flex-shrink: 0;
    margin-top: 1px;
  }

  .banner-body {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-1);
  }

  .banner-title {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-label-size);
    font-weight: var(--ss-type-label-weight);
    color: var(--ss-warning);
    letter-spacing: var(--ss-type-label-letter-spacing);
    text-transform: uppercase;
  }

  .banner-desc {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-secondary);
  }

  /* ===== Controls layout (2-col card grid) ===== */
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

  /* ===== Device card ===== */
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
    font-family: var(--ss-font-ui);
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

  /* ===== Select (HRIR picker) ===== */
  .select-group {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    flex: 1;
    justify-content: flex-end;
  }

  /* Constrains the bits-ui Select wrapper (which is width:100%). */
  .select-control {
    width: 200px;
    flex-shrink: 0;
  }

  /* ===== HRIR import / fetch actions ===== */
  .hrir-actions {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .ss-btn {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-label-size);
    font-weight: var(--ss-type-label-weight);
    letter-spacing: var(--ss-type-label-letter-spacing);
    color: var(--ss-text-primary);
    background: var(--ss-surface-input);
    border: 1px solid var(--ss-border);
    border-radius: var(--ss-radius-sm);
    padding: var(--ss-space-1) var(--ss-space-3);
    cursor: pointer;
    transition: background 0.1s, border-color 0.1s;
    min-height: var(--ss-control-h);
  }

  .ss-btn:hover:not(:disabled) {
    background: var(--ss-surface-2);
    border-color: var(--ss-accent);
  }

  .ss-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .hrir-msg {
    color: var(--ss-text-secondary);
  }
</style>
