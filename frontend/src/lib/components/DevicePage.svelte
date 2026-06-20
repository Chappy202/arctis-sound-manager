<script lang="ts">
  import { engineState, loadError } from "../stores.js";

  // ---------------------------------------------------------------------------
  // View-model: map raw EngineState → typed display rows
  // ---------------------------------------------------------------------------

  /** A single display row for a device field. */
  interface DeviceFieldRow {
    key: string;
    label: string;
    value: string;
    /** Optional semantic decoration on the value. */
    kind: "default" | "battery" | "status" | "percent";
  }

  /**
   * Maps a raw device_fields BTreeMap<String, String> into typed display rows.
   * Pure function — all logic here is unit-testable (see device.test.ts).
   *
   * Exported so tests can import it without Svelte.
   */
  export function mapDeviceFields(fields: Record<string, string>): DeviceFieldRow[] {
    return Object.entries(fields).map(([key, value]) => ({
      key,
      label: labelForKey(key),
      value,
      kind: kindForKey(key),
    }));
  }

  /** Human-readable label from a snake_case key. */
  export function labelForKey(key: string): string {
    // Known keys with better labels
    const wellKnown: Record<string, string> = {
      battery: "Battery",
      battery_charging: "Charging",
      battery_level: "Battery Level",
      anc_mode: "ANC Mode",
      anc_enabled: "Active Noise Cancelling",
      sidetone: "Sidetone",
      mic_muted: "Mic Muted",
      mic_gain: "Mic Gain",
      firmware: "Firmware",
      model: "Model",
      serial: "Serial",
      connection: "Connection",
      connected: "Connected",
    };
    return wellKnown[key] ?? key.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
  }

  /** Classify a key for value rendering hints. */
  export function kindForKey(key: string): DeviceFieldRow["kind"] {
    if (key === "battery" || key === "battery_level") return "battery";
    if (key === "battery_charging" || key === "connection" || key === "connected") return "status";
    if (key.endsWith("_percent") || key.endsWith("_pct")) return "percent";
    return "default";
  }

  /**
   * Returns the CSS color variable name for a battery level string.
   * E.g. "85" → "--ss-success", "20" → "--ss-warning", "10" → "--ss-danger".
   * Returns null when the string isn't a parseable number.
   */
  export function batteryColor(value: string): string | null {
    const n = parseFloat(value);
    if (isNaN(n)) return null;
    if (n > 40) return "--ss-success";
    if (n > 15) return "--ss-warning";
    return "--ss-danger";
  }

  // ---------------------------------------------------------------------------
  // Derived reactive values
  // ---------------------------------------------------------------------------

  const devicePresent = $derived($engineState?.device_present ?? false);
  const deviceFields = $derived($engineState?.device_fields ?? {});
  const fieldRows = $derived(mapDeviceFields(deviceFields));
  const hasDaemon = $derived($engineState !== null && $loadError === null);
</script>

<div class="device-page">
  <!-- Page header -->
  <div class="page-header">
    <h1 class="page-title">DEVICE</h1>
    <p class="page-subtitle">Battery · ANC · Sidetone · Mic · Power · Firmware</p>
  </div>

  <!-- Daemon not connected -->
  {#if $loadError}
    <div class="state-card state-card--error" role="status" aria-live="polite">
      <div class="state-icon" aria-hidden="true">⚠</div>
      <div class="state-body">
        <span class="state-title">Daemon Unavailable</span>
        <span class="state-desc">{$loadError}</span>
      </div>
    </div>

  <!-- Daemon OK but no device -->
  {:else if hasDaemon && !devicePresent}
    <div class="state-card state-card--no-device" role="status">
      <div class="state-icon no-device-icon" aria-hidden="true">◎</div>
      <div class="state-body">
        <span class="state-title">No Arctis Nova Pro Detected</span>
        <span class="state-desc">
          Connect your headset to the base station and ensure the base station
          is powered on.
        </span>
        <span class="state-note">
          Real-time device data (battery, ANC, sidetone) will appear here once
          the hardware polling layer is wired in a future plan.
        </span>
      </div>
    </div>

    <!-- Dimmed placeholder controls — keep layout, show "coming soon" context -->
    <div class="controls-grid controls-grid--dimmed" aria-hidden="true">
      {#each placeholderCards as card}
        <div class="device-card device-card--disabled">
          <div class="card-header">
            <span class="card-icon">{card.icon}</span>
            <h2 class="card-title">{card.label}</h2>
            <span class="pill pill--coming">SOON</span>
          </div>
          <div class="card-body">
            {#each card.rows as row}
              <div class="field-row field-row--disabled">
                <span class="field-label">{row}</span>
                <span class="field-value">—</span>
              </div>
            {/each}
          </div>
        </div>
      {/each}
    </div>

  <!-- Device present — render real fields -->
  {:else if hasDaemon && devicePresent}
    {#if fieldRows.length === 0}
      <div class="state-card" role="status">
        <div class="state-body">
          <span class="state-title">Device Connected</span>
          <span class="state-desc">No device fields reported yet.</span>
        </div>
      </div>
    {:else}
      <div class="device-card device-card--live">
        <div class="card-header">
          <span class="card-icon" aria-hidden="true">◉</span>
          <h2 class="card-title">Device Status</h2>
          <span class="pill pill--connected" role="status">CONNECTED</span>
        </div>
        <div class="card-body">
          {#each fieldRows as row (row.key)}
            <div class="field-row">
              <span class="field-label">{row.label}</span>
              {#if row.kind === "battery"}
                <span
                  class="field-value field-value--readout"
                  style:color={batteryColor(row.value)
                    ? `var(${batteryColor(row.value)})`
                    : "var(--ss-text-primary)"}
                >
                  {row.value}%
                </span>
              {:else}
                <span class="field-value field-value--readout">{row.value}</span>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    {/if}

  <!-- Loading / null state -->
  {:else}
    <div class="state-card" role="status" aria-live="polite">
      <div class="state-icon connecting-icon" aria-hidden="true">◎</div>
      <div class="state-body">
        <span class="state-title">Connecting to Daemon…</span>
        <span class="state-desc">Waiting for state from the Arctis daemon.</span>
      </div>
    </div>
  {/if}
</div>

<script lang="ts" module>
  // Placeholder card definitions (used when no device is present)
  // Kept in module context so they're available before the instance script,
  // and so they're a stable constant (no reactive re-creation).
  const placeholderCards = [
    { icon: "▮", label: "BATTERY",  rows: ["Level", "Charging"] },
    { icon: "◈", label: "ANC",      rows: ["Mode", "Transparency", "Intensity"] },
    { icon: "⏺", label: "MIC",      rows: ["Sidetone", "Mic Gain", "Mute"] },
    { icon: "⏻", label: "POWER",    rows: ["Auto-off", "Standby Timeout"] },
    { icon: "ℹ", label: "FIRMWARE", rows: ["Version", "Model", "Serial"] },
  ] as const;
</script>

<style>
  /* =========================================================================
   * Page layout
   * ========================================================================= */
  .device-page {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-5);
  }

  /* =========================================================================
   * Page header
   * ========================================================================= */
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

  /* =========================================================================
   * State cards (empty / error / connecting)
   * ========================================================================= */
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

  .state-card--error {
    border-color: var(--ss-danger);
    background: var(--ss-surface-1);
  }

  .state-card--no-device {
    border-color: var(--ss-border-strong);
  }

  .state-icon {
    font-size: 28px;
    line-height: 1;
    flex-shrink: 0;
    color: var(--ss-text-tertiary);
    margin-top: 2px;
  }

  .no-device-icon {
    color: var(--ss-text-disabled);
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
    line-height: var(--ss-type-body-line-height);
  }

  .state-note {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    line-height: var(--ss-type-caption-line-height);
    padding: var(--ss-space-2) var(--ss-space-3);
    background: var(--ss-bg-base);
    border-left: 2px solid var(--ss-border-strong);
    border-radius: 0 var(--ss-radius-xs) var(--ss-radius-xs) 0;
    margin-top: var(--ss-space-1);
  }

  /* =========================================================================
   * Controls grid (placeholder + live)
   * ========================================================================= */
  .controls-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: var(--ss-space-4);
  }

  .controls-grid--dimmed {
    opacity: 0.5;
    pointer-events: none;
    user-select: none;
  }

  /* =========================================================================
   * Device card
   * ========================================================================= */
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

  /* Status pills */
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

  .pill--connected {
    color: var(--ss-success);
    background: rgba(65, 169, 48, 0.12);
  }

  .pill--coming {
    color: var(--ss-text-tertiary);
    background: rgba(255, 255, 255, 0.06);
  }

  /* =========================================================================
   * Field rows
   * ========================================================================= */
  .card-body {
    display: flex;
    flex-direction: column;
  }

  .field-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--ss-space-2) var(--ss-space-4);
    border-bottom: var(--ss-border-width) solid var(--ss-border);
    min-height: var(--ss-control-h);
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
  }

  .field-value {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-primary);
  }

  .field-value--readout {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-readout-size);
    font-weight: var(--ss-type-readout-weight);
    font-variant-numeric: tabular-nums;
    line-height: var(--ss-type-readout-line-height);
  }
</style>
