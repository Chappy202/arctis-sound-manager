<script lang="ts">
  /**
   * DaemonSection.svelte — Daemon lifecycle controls for the Device page.
   *
   * Loads its own status on mount (daemonStatus IPC call); all lifecycle
   * actions (start / stop / restart / autostart toggle) update the local
   * status snapshot in-place so the UI stays consistent without waiting for
   * a state-changed broadcast.
   */
  import { onMount } from "svelte";
  import type { DaemonStatus } from "../ipc.js";
  import {
    daemonStatus,
    daemonStart,
    daemonStop,
    daemonRestart,
    daemonSetAutostart,
  } from "../ipc.js";
  import {
    statusLabel,
    dotKind,
    canStart,
    canStop,
    canRestart,
    autostartDisabledReason,
  } from "../daemonControl.js";
  import Switch from "../ui/Switch.svelte";

  // ---------------------------------------------------------------------------
  // State
  // ---------------------------------------------------------------------------

  let status = $state<DaemonStatus | null>(null);
  let busy = $state(false);
  let msg = $state("");

  onMount(() => {
    daemonStatus()
      .then((s) => (status = s))
      .catch((e) => (msg = String(e)));
  });

  // ---------------------------------------------------------------------------
  // Action handlers — shared busy flag guards concurrent clicks
  // ---------------------------------------------------------------------------

  async function onStart() {
    busy = true;
    try {
      status = await daemonStart();
      msg = "";
    } catch (e) {
      msg = String(e);
    } finally {
      busy = false;
    }
  }

  async function onStop() {
    busy = true;
    try {
      status = await daemonStop();
      msg = "";
    } catch (e) {
      msg = String(e);
    } finally {
      busy = false;
    }
  }

  async function onRestart() {
    busy = true;
    try {
      status = await daemonRestart();
      msg = "";
    } catch (e) {
      msg = String(e);
    } finally {
      busy = false;
    }
  }

  async function onToggleAutostart(enabled: boolean) {
    busy = true;
    try {
      status = await daemonSetAutostart(enabled);
      msg = "";
    } catch (e) {
      msg = String(e);
    } finally {
      busy = false;
    }
  }

  // ---------------------------------------------------------------------------
  // Derived
  // ---------------------------------------------------------------------------

  const disabledReason = $derived(status ? autostartDisabledReason(status) : null);
</script>

<div class="daemon-section">
  <!-- Card header -->
  <div class="card-header">
    <span class="card-icon" aria-hidden="true">⬡</span>
    <h2 class="card-title">DAEMON</h2>
  </div>

  <!-- Card body -->
  <div class="card-body">
    {#if status !== null}
      <!-- ─── Status row ───────────────────────────────────────────────────── -->
      <div class="field-row">
        <span class="field-label">STATUS</span>
        <div class="status-value">
          <span
            class="status-dot"
            class:status-dot--ok={dotKind(status) === "ok"}
            class:status-dot--off={dotKind(status) === "off"}
            aria-hidden="true"
          ></span>
          <span class="field-value field-value--readout">{statusLabel(status)}</span>
        </div>
      </div>

      <!-- ─── Binary path sub-line ─────────────────────────────────────────── -->
      <div class="field-row field-row--hint">
        <span class="binary-path">
          {status.binary_path ?? "asm-cli not found — set $ASM_CLI_BIN"}
        </span>
      </div>

      <!-- ─── Start / Stop / Restart buttons ──────────────────────────────── -->
      <div class="control-row">
        <span class="field-label">CONTROLS</span>
        <div class="actions-group">
          <button
            class="daemon-btn"
            disabled={!canStart(status) || busy}
            onclick={onStart}
            aria-label="Start daemon"
          >
            Start
          </button>
          <button
            class="daemon-btn daemon-btn--danger"
            disabled={!canStop(status) || busy}
            onclick={onStop}
            aria-label="Stop daemon"
          >
            Stop
          </button>
          <button
            class="daemon-btn"
            disabled={!canRestart(status) || busy}
            onclick={onRestart}
            aria-label="Restart daemon"
          >
            Restart
          </button>
        </div>
      </div>

      <!-- ─── Autostart switch ─────────────────────────────────────────────── -->
      <div class="control-row">
        <div class="autostart-label-group">
          <span class="field-label">START AT LOGIN</span>
          {#if disabledReason !== null}
            <span class="autostart-hint" title={disabledReason}>{disabledReason}</span>
          {/if}
        </div>
        <Switch
          checked={status.autostart_enabled}
          disabled={disabledReason !== null || busy}
          onCheckedChange={onToggleAutostart}
          ariaLabel="Start daemon at login"
          size="sm"
        />
      </div>

      <!-- ─── Inline feedback ──────────────────────────────────────────────── -->
      {#if msg}
        <div class="daemon-error" role="alert">
          <span class="daemon-error__icon" aria-hidden="true">✕</span>
          <span class="daemon-error__msg">{msg}</span>
        </div>
      {/if}

    {:else if msg}
      <!-- Initial load error -->
      <div class="daemon-error daemon-error--load" role="alert">
        <span class="daemon-error__icon" aria-hidden="true">✕</span>
        <span class="daemon-error__msg">{msg}</span>
      </div>

    {:else}
      <!-- Loading -->
      <div class="field-row">
        <span class="field-label field-label--hint">Loading daemon status…</span>
      </div>
    {/if}
  </div>
</div>

<style>
  /* =========================================================================
   * Outer card — mirrors .device-card / .device-card--live from DevicePage
   * ========================================================================= */
  .daemon-section {
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-md);
    box-shadow: var(--ss-e1);
    overflow: hidden;
  }

  /* =========================================================================
   * Card header — mirrors .card-header / .card-icon / .card-title
   * ========================================================================= */
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

  /* =========================================================================
   * Card body — mirrors .card-body from DevicePage
   * ========================================================================= */
  .card-body {
    display: flex;
    flex-direction: column;
  }

  /* =========================================================================
   * Field rows — mirrors .field-row / .field-label / .field-value
   * ========================================================================= */
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

  .field-row--hint {
    min-height: unset;
    padding-top: var(--ss-space-1);
    padding-bottom: var(--ss-space-2);
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
   * Status dot + value pair
   * ========================================================================= */
  .status-value {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
  }

  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: var(--ss-radius-pill);
    flex-shrink: 0;
    background: var(--ss-text-disabled);
  }

  .status-dot--ok {
    background: var(--ss-success);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--ss-success) 25%, transparent);
  }

  .status-dot--off {
    background: var(--ss-text-disabled);
  }

  /* =========================================================================
   * Binary path — muted sub-line
   * ========================================================================= */
  .binary-path {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    word-break: break-all;
  }

  /* =========================================================================
   * Control row — mirrors .control-row from DevicePage
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

  .control-row:last-child {
    border-bottom: none;
  }

  /* =========================================================================
   * Action buttons (Start / Stop / Restart)
   * ========================================================================= */
  .actions-group {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    flex-shrink: 0;
  }

  .daemon-btn {
    padding: var(--ss-space-1) var(--ss-space-3);
    background: var(--ss-surface-input-alt);
    color: var(--ss-text-primary);
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-sm);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-button-size);
    font-weight: var(--ss-type-button-weight);
    letter-spacing: var(--ss-type-button-letter-spacing);
    text-transform: var(--ss-type-button-transform);
    cursor: pointer;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard),
      opacity var(--ss-dur-fast) var(--ss-ease-standard);
    white-space: nowrap;
    height: var(--ss-control-h-sm);
    display: inline-flex;
    align-items: center;
  }

  .daemon-btn:hover:not(:disabled) {
    background: var(--ss-surface-3);
    color: var(--ss-text-bright);
  }

  .daemon-btn:active:not(:disabled) {
    background: var(--ss-surface-input);
  }

  .daemon-btn:disabled {
    opacity: 0.38;
    cursor: not-allowed;
  }

  .daemon-btn:focus-visible {
    outline: 2px solid var(--ss-accent-border);
    outline-offset: 2px;
  }

  .daemon-btn--danger:hover:not(:disabled) {
    background: color-mix(in srgb, var(--ss-danger) 20%, var(--ss-surface-3));
    color: var(--ss-danger);
    border-color: color-mix(in srgb, var(--ss-danger) 40%, transparent);
  }

  /* =========================================================================
   * Autostart row label group
   * ========================================================================= */
  .autostart-label-group {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-1);
    flex: 1;
    min-width: 0;
  }

  .autostart-hint {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    line-height: 1.3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  /* =========================================================================
   * Inline error / feedback
   * ========================================================================= */
  .daemon-error {
    display: flex;
    align-items: flex-start;
    gap: var(--ss-space-2);
    padding: var(--ss-space-2) var(--ss-space-4);
    background: color-mix(in srgb, var(--ss-danger) 8%, transparent);
    border-top: var(--ss-border-width) solid color-mix(in srgb, var(--ss-danger) 20%, transparent);
  }

  .daemon-error--load {
    border-top: none;
    border-bottom: none;
  }

  .daemon-error__icon {
    font-size: 11px;
    color: var(--ss-danger);
    flex-shrink: 0;
    margin-top: 1px;
  }

  .daemon-error__msg {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-danger);
    line-height: 1.4;
    word-break: break-word;
  }
</style>
