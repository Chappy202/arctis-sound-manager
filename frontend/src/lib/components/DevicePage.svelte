<script lang="ts">
  import { engineState, loadError } from "../stores.js";
  import { deviceSet } from "../ipc.js";
  import {
    parseAncMode,
    ancModeToValue,
    ANC_MODES,
    ANC_MODE_LABELS,
    parseSidetoneLevel,
    sidetoneLevelLabel,
    SIDETONE_OPTIONS,
    parseAutoOffLevel,
    autoOffLabel,
    AUTO_OFF_OPTIONS,
    parse1to10,
    enabledControls,
    isGateError,
    type AncMode,
  } from "../deviceControls.js";
  import Select from "../ui/Select.svelte";
  import Slider from "../ui/Slider.svelte";
  import ToggleGroup from "../ui/ToggleGroup.svelte";
  import type { SelectOption } from "../ui/selectUtils.js";
  import DaemonSection from "./DaemonSection.svelte";
  import DaemonUnavailable from "./DaemonUnavailable.svelte";
  import { connectionStatus } from "../stores/connection.js";

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

  // Pre-filtered row groups — hoisted out of the template so each {#each}/{#if}
  // doesn't re-run Array.filter on every reactive tick (the state poll runs at
  // 250 ms; these only change when device_fields changes).
  const FIRMWARE_KEYS = ["firmware", "model", "serial"];
  const STATUS_EXCLUDE_KEYS = [
    "battery", "battery_level", "battery_charging",
    "firmware", "model", "serial",
    "anc_mode", "anc_enabled",
    "sidetone",
    "mic_muted", "mic_gain",
    "mic_led", "mic_volume",
    "transparency_level", "inactive_time",
  ];
  const firmwareRows = $derived(fieldRows.filter((r) => FIRMWARE_KEYS.includes(r.key)));
  const statusRows = $derived(fieldRows.filter((r) => !STATUS_EXCLUDE_KEYS.includes(r.key)));
  const nonBatteryRows = $derived(fieldRows.filter((r) => r.kind !== "battery"));
  const micReadoutRows = $derived(
    fieldRows.filter((r) => ["mic_muted", "mic_gain"].includes(r.key)),
  );

  // Enabled controls derived from available device fields
  const controls = $derived(enabledControls(deviceFields));

  // Current values from device state
  const currentAncMode = $derived(
    "anc_mode" in deviceFields ? parseAncMode(deviceFields.anc_mode) : ("off" as AncMode)
  );
  const currentSidetone = $derived(
    "sidetone" in deviceFields ? parseSidetoneLevel(deviceFields.sidetone) : 0
  );
  const currentAutoOff = $derived(
    "inactive_time" in deviceFields ? parseAutoOffLevel(deviceFields.inactive_time) : 0
  );
  const currentMicLed = $derived(
    "mic_led" in deviceFields ? parse1to10(deviceFields.mic_led) : 5
  );
  const currentTransparencyLevel = $derived(
    "transparency_level" in deviceFields ? parse1to10(deviceFields.transparency_level) : 5
  );
  const currentMicVolume = $derived(
    "mic_volume" in deviceFields ? parse1to10(deviceFields.mic_volume) : 5
  );

  // Battery display
  const batteryValue = $derived(deviceFields.battery ?? deviceFields.battery_level ?? null);
  const isCharging = $derived(
    deviceFields.battery_charging === "true" || deviceFields.battery_charging === "1"
  );

  // ---------------------------------------------------------------------------
  // Control state: pending errors per control + global gate banner state
  // ---------------------------------------------------------------------------

  /** Per-control error messages (null = no error). */
  let controlErrors = $state<Record<string, string | null>>({
    anc: null,
    sidetone: null,
    inactive_time: null,
    mic_led: null,
    transparency_level: null,
    mic_volume: null,
  });

  /** True when ANY write attempt hit the gate (for the banner). */
  let gateHit = $state(false);

  /** True when a write is in-flight for a given control. */
  let pending = $state<Record<string, boolean>>({
    anc: false,
    sidetone: false,
    inactive_time: false,
    mic_led: false,
    transparency_level: false,
    mic_volume: false,
  });

  /** Clear the gate banner. */
  function dismissGate() {
    gateHit = false;
  }

  /** Generic device-write handler — surfaces the gate error honestly, never fakes success. */
  async function sendControl(control: string, value: number): Promise<void> {
    pending = { ...pending, [control]: true };
    controlErrors = { ...controlErrors, [control]: null };
    try {
      await deviceSet(control, value);
      // On success, EngineState update will arrive via the state-changed event;
      // the control will snap to the new value automatically.
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      controlErrors = { ...controlErrors, [control]: msg };
      if (isGateError(msg)) {
        gateHit = true;
      }
    } finally {
      pending = { ...pending, [control]: false };
    }
  }

  // ---------------------------------------------------------------------------
  // Select / segmented options (string-typed for the bits-ui wrappers)
  // ---------------------------------------------------------------------------

  const ancOptions: SelectOption[] = ANC_MODES.map((m) => ({ value: m, label: ANC_MODE_LABELS[m] }));
  const sidetoneOptions: SelectOption[] = SIDETONE_OPTIONS.map((o) => ({
    value: String(o.value),
    label: o.label,
  }));
  const autoOffOptions: SelectOption[] = AUTO_OFF_OPTIONS.map((o) => ({
    value: String(o.value),
    label: o.label,
  }));

  // ---------------------------------------------------------------------------
  // Slider draft state — local mirror so the thumb/readout track during a drag
  // (the device value only updates after a successful commit round-trips). Each
  // draft resyncs from the device value ONLY when that value actually changes,
  // so the 250 ms state poll never clobbers a value the user is mid-drag on.
  // ---------------------------------------------------------------------------

  let transparencyDraft = $state(5);
  let micLedDraft = $state(5);
  let micVolumeDraft = $state(5);

  let lastTransparency = NaN;
  let lastMicLed = NaN;
  let lastMicVolume = NaN;

  $effect(() => {
    const v = currentTransparencyLevel;
    if (v !== lastTransparency) { lastTransparency = v; transparencyDraft = v; }
  });
  $effect(() => {
    const v = currentMicLed;
    if (v !== lastMicLed) { lastMicLed = v; micLedDraft = v; }
  });
  $effect(() => {
    const v = currentMicVolume;
    if (v !== lastMicVolume) { lastMicVolume = v; micVolumeDraft = v; }
  });

  // ---------------------------------------------------------------------------
  // Control event handlers
  // ---------------------------------------------------------------------------

  function onAncChange(mode: AncMode) {
    sendControl("anc", ancModeToValue(mode));
  }

  function onSidetoneChange(value: number) {
    sendControl("sidetone", value);
  }
</script>

<div class="device-page">
  <!-- Page header -->
  <div class="page-header">
    <h1 class="page-title">DEVICE</h1>
    <p class="page-subtitle">Battery · ANC · Sidetone · Mic · Power · Firmware</p>
  </div>

  {#if $connectionStatus !== "connected"}
    <DaemonUnavailable />
  {:else}

  <!-- Daemon OK but no device -->
  {#if hasDaemon && !devicePresent}
    <div class="state-card state-card--no-device" role="status">
      <div class="state-icon no-device-icon" aria-hidden="true">◎</div>
      <div class="state-body">
        <span class="state-title">No Arctis Nova Pro Detected</span>
        <span class="state-desc">
          Connect your headset to the base station and ensure the base station
          is powered on.
        </span>
      </div>
    </div>

    <!-- Dimmed placeholder controls — keep layout visible, show disabled state -->
    <div class="controls-layout controls-layout--dimmed" aria-hidden="true">
      {#each placeholderCards as card}
        <div class="device-card device-card--disabled">
          <div class="card-header">
            <span class="card-icon">{card.icon}</span>
            <h2 class="card-title">{card.label}</h2>
            <span class="pill pill--coming">NO DEVICE</span>
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

  <!-- Device present — render live status + controls -->
  {:else if hasDaemon && devicePresent}

    <!-- Gate banner: shown whenever a write was rejected by the daemon allowlist -->
    {#if gateHit}
      <div class="gate-banner" role="alert" aria-live="assertive">
        <span class="gate-banner__icon" aria-hidden="true">⚠</span>
        <div class="gate-banner__body">
          <strong class="gate-banner__title">Device controls pending validation</strong>
          <span class="gate-banner__desc">
            Write access to this headset has not yet been enabled by the owner.
            Controls are displayed for preview only — changes will not be sent to hardware
            until each control passes its OWNER-RUN safety gate.
          </span>
        </div>
        <button class="gate-banner__dismiss" onclick={dismissGate} aria-label="Dismiss">✕</button>
      </div>
    {/if}

    <div class="controls-layout">

      <!-- ─── BATTERY card ─────────────────────────────────────────────────── -->
      <div class="device-card device-card--live">
        <div class="card-header">
          <span class="card-icon" aria-hidden="true">▮</span>
          <h2 class="card-title">BATTERY</h2>
          {#if batteryValue !== null}
            <span
              class="pill"
              style:color={batteryColor(batteryValue)
                ? `var(${batteryColor(batteryValue)})`
                : "var(--ss-text-secondary)"}
              style:background={batteryColor(batteryValue)
                ? `color-mix(in srgb, var(${batteryColor(batteryValue)}) 12%, transparent)`
                : "rgba(255,255,255,0.06)"}
            >
              {#if isCharging}⚡ CHARGING{:else}CONNECTED{/if}
            </span>
          {:else}
            <span class="pill pill--connected">CONNECTED</span>
          {/if}
        </div>
        <div class="card-body">
          {#if batteryValue !== null}
            <div class="field-row">
              <span class="field-label">BATTERY LEVEL</span>
              <div class="battery-display">
                <div class="battery-bar">
                  <div
                    class="battery-fill"
                    style:width="{Math.min(Math.max(parseFloat(batteryValue) || 0, 0), 100)}%"
                    style:background={batteryColor(batteryValue)
                      ? `var(${batteryColor(batteryValue)})`
                      : "var(--ss-text-secondary)"}
                    class:battery-fill--pulse={isCharging}
                  ></div>
                </div>
                <span
                  class="field-value field-value--readout"
                  style:color={batteryColor(batteryValue)
                    ? `var(${batteryColor(batteryValue)})`
                    : "var(--ss-text-primary)"}
                >
                  {batteryValue}%
                </span>
              </div>
            </div>
          {/if}
          {#each nonBatteryRows as row (row.key)}
            {#if ["firmware", "model", "serial", "battery_charging"].includes(row.key)}
              <div class="field-row">
                <span class="field-label">{row.label.toUpperCase()}</span>
                <span class="field-value field-value--readout">{row.value}</span>
              </div>
            {/if}
          {/each}
        </div>
      </div>

      <!-- ─── ANC card ──────────────────────────────────────────────────────── -->
      {#if controls.has("anc")}
        <div class="device-card device-card--live">
          <div class="card-header">
            <span class="card-icon" aria-hidden="true">◈</span>
            <h2 class="card-title">ANC</h2>
            <span class="pill pill--live">
              {ANC_MODE_LABELS[currentAncMode].toUpperCase()}
            </span>
          </div>
          <div class="card-body">
            <!-- ANC mode segmented control -->
            <div class="control-row">
              <span class="field-label">MODE</span>
              <ToggleGroup
                options={ancOptions}
                value={currentAncMode}
                disabled={pending.anc || !devicePresent}
                ariaLabel="ANC Mode"
                onValueChange={(v) => onAncChange(v as AncMode)}
              />
            </div>
            {#if controlErrors.anc}
              <div class="control-error" role="alert">
                <span class="control-error__icon">✕</span>
                <span class="control-error__msg">{controlErrors.anc}</span>
              </div>
            {/if}

            <!-- Transparency level — only when anc=transparency AND field available -->
            {#if controls.has("transparency_level")}
              <div class="control-row control-row--sub">
                <span class="field-label">TRANSPARENCY LEVEL</span>
                <div class="slider-group">
                  <div class="slider-control">
                    <Slider
                      min={1}
                      max={10}
                      step={1}
                      value={transparencyDraft}
                      disabled={pending.transparency_level || !devicePresent || currentAncMode !== "transparency"}
                      onValueChange={(v) => (transparencyDraft = v)}
                      onValueCommit={(v) => sendControl("transparency_level", v)}
                      ariaLabel="Transparency level {transparencyDraft} of 10"
                    />
                  </div>
                  <span class="slider-readout">{transparencyDraft}</span>
                </div>
              </div>
              {#if controlErrors.transparency_level}
                <div class="control-error" role="alert">
                  <span class="control-error__icon">✕</span>
                  <span class="control-error__msg">{controlErrors.transparency_level}</span>
                </div>
              {/if}
            {/if}
          </div>
        </div>
      {/if}

      <!-- ─── SIDETONE card ─────────────────────────────────────────────────── -->
      {#if controls.has("sidetone")}
        <div class="device-card device-card--live">
          <div class="card-header">
            <span class="card-icon" aria-hidden="true">⏺</span>
            <h2 class="card-title">SIDETONE</h2>
            <span class="pill pill--live">
              {sidetoneLevelLabel(currentSidetone).toUpperCase()}
            </span>
          </div>
          <div class="card-body">
            <div class="control-row">
              <span class="field-label">LEVEL</span>
              <ToggleGroup
                options={sidetoneOptions}
                value={String(currentSidetone)}
                disabled={pending.sidetone || !devicePresent}
                ariaLabel="Sidetone level"
                onValueChange={(v) => onSidetoneChange(Number(v))}
              />
            </div>
            {#if controlErrors.sidetone}
              <div class="control-error" role="alert">
                <span class="control-error__icon">✕</span>
                <span class="control-error__msg">{controlErrors.sidetone}</span>
              </div>
            {/if}
            <div class="field-row">
              <span class="field-label field-label--hint">
                Hear your own voice in the headset mic
              </span>
            </div>
          </div>
        </div>
      {/if}

      <!-- ─── MIC card ──────────────────────────────────────────────────────── -->
      <div class="device-card device-card--live">
        <div class="card-header">
          <span class="card-icon" aria-hidden="true">◉</span>
          <h2 class="card-title">MIC</h2>
        </div>
        <div class="card-body">
          {#if controls.has("mic_led")}
            <div class="control-row">
              <span class="field-label">MIC LED</span>
              <div class="slider-group">
                <div class="slider-control">
                  <Slider
                    min={1}
                    max={10}
                    step={1}
                    value={micLedDraft}
                    disabled={pending.mic_led || !devicePresent}
                    onValueChange={(v) => (micLedDraft = v)}
                    onValueCommit={(v) => sendControl("mic_led", v)}
                    ariaLabel="Mic LED brightness {micLedDraft} of 10"
                  />
                </div>
                <span class="slider-readout">{micLedDraft}</span>
              </div>
            </div>
            {#if controlErrors.mic_led}
              <div class="control-error" role="alert">
                <span class="control-error__icon">✕</span>
                <span class="control-error__msg">{controlErrors.mic_led}</span>
              </div>
            {/if}
          {/if}

          {#if controls.has("mic_volume")}
            <div class="control-row">
              <span class="field-label">MIC VOLUME</span>
              <div class="slider-group">
                <div class="slider-control">
                  <Slider
                    min={1}
                    max={10}
                    step={1}
                    value={micVolumeDraft}
                    disabled={pending.mic_volume || !devicePresent}
                    onValueChange={(v) => (micVolumeDraft = v)}
                    onValueCommit={(v) => sendControl("mic_volume", v)}
                    ariaLabel="Mic volume {micVolumeDraft} of 10"
                  />
                </div>
                <span class="slider-readout">{micVolumeDraft}</span>
              </div>
            </div>
            {#if controlErrors.mic_volume}
              <div class="control-error" role="alert">
                <span class="control-error__icon">✕</span>
                <span class="control-error__msg">{controlErrors.mic_volume}</span>
              </div>
            {/if}
          {/if}

          <!-- Read-only mic fields from device state -->
          {#each micReadoutRows as row (row.key)}
            <div class="field-row">
              <span class="field-label">{row.label.toUpperCase()}</span>
              <span class="field-value field-value--readout">{row.value}</span>
            </div>
          {/each}

          {#if !controls.has("mic_led") && !controls.has("mic_volume") && micReadoutRows.length === 0}
            <div class="field-row">
              <span class="field-label">STATUS</span>
              <span class="field-value">—</span>
            </div>
          {/if}
        </div>
      </div>

      <!-- ─── POWER card ────────────────────────────────────────────────────── -->
      {#if controls.has("inactive_time")}
        <div class="device-card device-card--live">
          <div class="card-header">
            <span class="card-icon" aria-hidden="true">⏻</span>
            <h2 class="card-title">POWER</h2>
            <span class="pill pill--live">
              {autoOffLabel(currentAutoOff).toUpperCase()}
            </span>
          </div>
          <div class="card-body">
            <div class="control-row">
              <span class="field-label">AUTO-OFF</span>
              <div class="select-control">
                <Select
                  options={autoOffOptions}
                  value={String(currentAutoOff)}
                  disabled={pending.inactive_time || !devicePresent}
                  ariaLabel="Auto-off timeout"
                  onValueChange={(v) => sendControl("inactive_time", Number(v))}
                />
              </div>
            </div>
            {#if controlErrors.inactive_time}
              <div class="control-error" role="alert">
                <span class="control-error__icon">✕</span>
                <span class="control-error__msg">{controlErrors.inactive_time}</span>
              </div>
            {/if}
            <div class="field-row">
              <span class="field-label field-label--hint">
                Auto-power-off when idle
              </span>
            </div>
          </div>
        </div>
      {/if}

      <!-- ─── FIRMWARE / INFO card ──────────────────────────────────────────── -->
      {#if firmwareRows.length > 0}
        <div class="device-card device-card--live">
          <div class="card-header">
            <span class="card-icon" aria-hidden="true">ℹ</span>
            <h2 class="card-title">FIRMWARE</h2>
          </div>
          <div class="card-body">
            {#each firmwareRows as row (row.key)}
              <div class="field-row">
                <span class="field-label">{row.label.toUpperCase()}</span>
                <span class="field-value field-value--readout">{row.value}</span>
              </div>
            {/each}
          </div>
        </div>
      {/if}

      <!-- Remaining fields not in any card above -->
      {#if statusRows.length > 0}
        <div class="device-card device-card--live">
          <div class="card-header">
            <span class="card-icon" aria-hidden="true">◉</span>
            <h2 class="card-title">STATUS</h2>
            <span class="pill pill--connected">CONNECTED</span>
          </div>
          <div class="card-body">
            {#each statusRows as row (row.key)}
              <div class="field-row">
                <span class="field-label">{row.label.toUpperCase()}</span>
                <span class="field-value field-value--readout">{row.value}</span>
              </div>
            {/each}
          </div>
        </div>
      {/if}

    </div><!-- /controls-layout -->

  {/if}

  <!-- ─── DAEMON section ─────────────────────────────────────────────────── -->
  <DaemonSection />

  {/if}<!-- /connectionStatus gate -->
</div>

<script lang="ts" module>
  // Placeholder card definitions (used when no device is present)
  const placeholderCards = [
    { icon: "▮", label: "BATTERY",  rows: ["Level", "Charging"] },
    { icon: "◈", label: "ANC",      rows: ["Mode", "Transparency", "Intensity"] },
    { icon: "⏺", label: "SIDETONE", rows: ["Level"] },
    { icon: "◉", label: "MIC",      rows: ["Mic LED", "Mic Volume"] },
    { icon: "⏻", label: "POWER",    rows: ["Auto-off"] },
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
   * Gate banner
   * ========================================================================= */
  .gate-banner {
    display: flex;
    align-items: flex-start;
    gap: var(--ss-space-3);
    padding: var(--ss-space-4) var(--ss-space-5);
    background: color-mix(in srgb, var(--ss-warning) 8%, var(--ss-surface-2));
    border: var(--ss-border-width) solid color-mix(in srgb, var(--ss-warning) 30%, transparent);
    border-radius: var(--ss-radius-md);
    box-shadow: var(--ss-e1);
  }

  .gate-banner__icon {
    font-size: 16px;
    color: var(--ss-warning);
    flex-shrink: 0;
    margin-top: 1px;
  }

  .gate-banner__body {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-1);
    flex: 1;
  }

  .gate-banner__title {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-label-size);
    font-weight: var(--ss-type-label-weight);
    letter-spacing: var(--ss-type-label-letter-spacing);
    color: var(--ss-warning);
    text-transform: uppercase;
  }

  .gate-banner__desc {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-secondary);
    line-height: 1.45;
  }

  .gate-banner__dismiss {
    background: none;
    border: none;
    color: var(--ss-text-tertiary);
    font-size: 14px;
    cursor: pointer;
    padding: 0;
    line-height: 1;
    flex-shrink: 0;
    transition: color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .gate-banner__dismiss:hover {
    color: var(--ss-text-primary);
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

  /* =========================================================================
   * Controls layout
   * ========================================================================= */
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

  .pill--live {
    color: var(--ss-accent);
    background: var(--ss-accent-soft);
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

  /* =========================================================================
   * Battery display
   * ========================================================================= */
  .battery-display {
    display: flex;
    align-items: center;
    gap: var(--ss-space-3);
    flex: 1;
    justify-content: flex-end;
  }

  .battery-bar {
    width: 80px;
    height: 6px;
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-pill);
    overflow: hidden;
    flex-shrink: 0;
  }

  .battery-fill {
    height: 100%;
    border-radius: var(--ss-radius-pill);
    transition: width var(--ss-dur-base) var(--ss-ease-out);
  }

  .battery-fill--pulse {
    animation: battery-pulse 2s ease-in-out infinite;
  }

  @keyframes battery-pulse {
    0%, 100% { opacity: 1; }
    50%       { opacity: 0.5; }
  }

  @media (prefers-reduced-motion: reduce) {
    .battery-fill--pulse {
      animation: none;
    }
  }

  /* =========================================================================
   * Control rows (interactive)
   * ========================================================================= */
  .control-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--ss-space-3) var(--ss-space-4);
    border-bottom: var(--ss-border-width) solid var(--ss-border);
    gap: var(--ss-space-3);
    min-height: var(--ss-control-h);
  }

  .control-row--sub {
    background: color-mix(in srgb, var(--ss-surface-2) 40%, transparent);
  }

  /* =========================================================================
   * Slider
   * ========================================================================= */
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
    min-width: 20px;
    text-align: right;
  }

  /* =========================================================================
   * Select / dropdown
   * ========================================================================= */
  /* Constrains the bits-ui Select wrapper (which is width:100%). */
  .select-control {
    width: 160px;
    flex-shrink: 0;
  }

  /* =========================================================================
   * Inline control error
   * ========================================================================= */
  .control-error {
    display: flex;
    align-items: flex-start;
    gap: var(--ss-space-2);
    padding: var(--ss-space-2) var(--ss-space-4);
    background: color-mix(in srgb, var(--ss-danger) 8%, transparent);
    border-top: var(--ss-border-width) solid color-mix(in srgb, var(--ss-danger) 20%, transparent);
  }

  .control-error__icon {
    font-size: 11px;
    color: var(--ss-danger);
    flex-shrink: 0;
    margin-top: 1px;
  }

  .control-error__msg {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-danger);
    line-height: 1.4;
    word-break: break-word;
  }
</style>
