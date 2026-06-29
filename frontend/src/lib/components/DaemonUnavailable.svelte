<script lang="ts">
  import { connectionStatus, reconnect } from "../stores/connection.js";
  import { loadError } from "../stores.js";
  import { daemonStart } from "../ipc.js";
  import { viewFor } from "../daemonUnavailable.js";

  const view = $derived(viewFor($connectionStatus));
  let busy = $state(false);
  let actionError = $state<string | null>(null);

  async function onStart() {
    busy = true; actionError = null;
    try { await daemonStart(); await reconnect(); }
    catch (e) { actionError = String(e); }
    finally { busy = false; }
  }
  async function onRetry() {
    busy = true; actionError = null;
    try { await reconnect(); }
    catch (e) { actionError = String(e); }
    finally { busy = false; }
  }
</script>

{#if view === "connecting"}
  <div class="du-card" role="status" aria-live="polite">
    <div class="du-spinner" aria-hidden="true"></div>
    <p class="du-title">Connecting to daemon…</p>
  </div>
{:else if view === "disconnected"}
  <div class="du-card" role="alert" aria-live="assertive">
    <span class="du-icon" aria-hidden="true">◉</span>
    <p class="du-title">Daemon not running</p>
    <p class="du-body">The Arctis Sound Manager service isn't reachable, so changes won't apply.</p>
    {#if $loadError}<p class="du-error-detail">{$loadError}</p>{/if}
    <div class="du-actions">
      <button class="du-btn du-btn--primary" onclick={onStart} disabled={busy}>Start daemon</button>
      <button class="du-btn" onclick={onRetry} disabled={busy}>Retry</button>
    </div>
    {#if actionError}<p class="du-action-error">{actionError}</p>{/if}
  </div>
{/if}

<style>
  .du-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--ss-space-3);
    text-align: center;
    margin: auto;
    max-width: 460px;
    padding: var(--ss-space-6);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    color: var(--ss-text-primary);
  }

  .du-icon {
    font-size: 28px;
    color: var(--ss-danger);
  }

  /* 18px literal — matches MixerPage .daemon-title size; --ss-type-h3-size excluded per brief */
  .du-title {
    font-family: var(--ss-font-display);
    font-size: 18px;
    margin: 0;
  }

  .du-body {
    color: var(--ss-text-secondary);
    font-size: var(--ss-type-body-size);
    margin: 0;
  }

  /* 11px literal — matches --ss-type-caption-size value; token excluded per brief */
  .du-error-detail {
    font-family: var(--ss-font-mono);
    font-size: 11px;
    color: var(--ss-text-disabled);
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-xs);
    padding: var(--ss-space-1) var(--ss-space-2);
    margin: 0;
    word-break: break-all;
  }

  .du-actions {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    margin-top: var(--ss-space-2);
  }

  /* Secondary / neutral button (Retry) */
  .du-btn {
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-4);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-xs);
    background: var(--ss-surface-input);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-ui);
    cursor: pointer;
  }

  .du-btn:disabled {
    opacity: 0.5;
    cursor: default;
  }

  /* Primary / accent button (Start daemon) — mirrors MixerPage .retry-btn exactly */
  .du-btn--primary {
    height: var(--ss-control-h);
    padding: 0 var(--ss-space-6);
    background: var(--ss-gradient-primary);
    border: none;
    border-radius: var(--ss-radius-sm);
    /* --ss-on-accent doesn't exist; --ss-text-bright (#fff) is the correct token */
    color: var(--ss-text-bright);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-button-size);
    font-weight: var(--ss-type-button-weight);
    letter-spacing: var(--ss-type-button-letter-spacing);
    text-transform: uppercase;
    cursor: pointer;
    transition: filter var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .du-btn--primary:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .du-btn--primary:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  /* 11px literal — same note as .du-error-detail */
  .du-action-error {
    color: var(--ss-danger);
    font-size: 11px;
    margin: 0;
  }

  .du-spinner {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    border: 3px solid var(--ss-border);
    border-top-color: var(--ss-accent);
    animation: du-spin 0.8s linear infinite;
  }

  @keyframes du-spin {
    to { transform: rotate(360deg); }
  }
</style>
