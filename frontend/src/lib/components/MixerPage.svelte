<script lang="ts">
  import { engineState, loadError, init, destroy } from "../stores.js";
  import ChannelStrip from "./ChannelStrip.svelte";
  import RouteList from "./RouteList.svelte";

  // init() is idempotent — safe to call here AND from AppShell.
  // Calling it here ensures the mixer works even if AppShell hasn't yet mounted.
  $effect(() => {
    init();
  });

  function refresh() {
    destroy();
    init();
  }
</script>

<div class="mixer-page">
  {#if $loadError}
    <!-- ===== Daemon-down empty state ===== -->
    <div class="daemon-down-card" role="alert" aria-live="assertive">
      <div class="daemon-icon" aria-hidden="true">◉</div>
      <h2 class="daemon-title">Daemon not running</h2>
      <p class="daemon-desc">
        Start it with <code class="daemon-cmd">asm-cli daemon</code> then click Retry.
      </p>
      <button class="retry-btn" onclick={refresh} aria-label="Retry connecting to daemon">
        Retry
      </button>
    </div>

  {:else if !$engineState}
    <!-- ===== Loading state ===== -->
    <div class="loading-state" aria-busy="true" aria-live="polite">
      <div class="loading-spinner" aria-hidden="true"></div>
      <p class="loading-text">Connecting to daemon…</p>
    </div>

  {:else}
    <!-- ===== Mixer content ===== -->
    <div class="mixer-header">
      <div class="mixer-title-row">
        <h1 class="mixer-title">MIXER</h1>
        {#if !$engineState.device_present}
          <span class="no-device-badge" role="status" aria-label="No device detected">
            NO DEVICE
          </span>
        {/if}
      </div>
      <p class="active-profile-hint">
        Profile: <span class="profile-accent">{$engineState.active_profile}</span>
      </p>
    </div>

    <!-- ===== Channel strips row ===== -->
    <div class="channels-container">
      {#if $engineState.channels.length === 0}
        <p class="channels-empty">No channels reported by the daemon.</p>
      {:else}
        <div
          class="channels-row"
          role="list"
          aria-label="Audio channel strips"
        >
          {#each $engineState.channels as channel (channel.id)}
            <div role="listitem">
              <ChannelStrip
                {channel}
                onOutputChanged={() => {
                  // State will be refreshed via state-changed event.
                  // No extra action needed here.
                }}
              />
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <!-- ===== Route list ===== -->
    <div class="routes-container">
      <RouteList />
    </div>
  {/if}
</div>

<style>
  .mixer-page {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-6);
    min-height: 100%;
  }

  /* ===== Daemon-down card ===== */
  .daemon-down-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--ss-space-4);
    padding: var(--ss-space-12) var(--ss-space-8);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    box-shadow: var(--ss-e1);
    text-align: center;
    max-width: 420px;
    margin: var(--ss-space-8) auto;
  }

  .daemon-icon {
    font-size: 36px;
    color: var(--ss-danger);
    line-height: 1;
    opacity: 0.7;
  }

  .daemon-title {
    font-family: var(--ss-font-display);
    font-size: var(--ss-type-h2-size);
    font-weight: var(--ss-type-h2-weight);
    letter-spacing: var(--ss-type-h2-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-primary);
    margin: 0;
  }

  .daemon-desc {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-secondary);
    margin: 0;
    line-height: 1.5;
  }

  .daemon-cmd {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-body-size);
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-xs);
    padding: 2px var(--ss-space-2);
    color: var(--ss-accent);
  }

  .retry-btn {
    height: var(--ss-control-h);
    padding: 0 var(--ss-space-6);
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
    transition: filter var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .retry-btn:hover {
    filter: brightness(1.1);
  }

  .retry-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  /* ===== Loading state ===== */
  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--ss-space-4);
    padding: var(--ss-space-16) 0;
  }

  .loading-spinner {
    width: 32px;
    height: 32px;
    border: 3px solid var(--ss-border-strong);
    border-top-color: var(--ss-accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .loading-text {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-tertiary);
    margin: 0;
  }

  /* ===== Mixer header ===== */
  .mixer-header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: var(--ss-space-3);
  }

  .mixer-title-row {
    display: flex;
    align-items: center;
    gap: var(--ss-space-3);
  }

  .mixer-title {
    font-family: var(--ss-font-display);
    font-size: var(--ss-type-display-size);
    font-weight: var(--ss-type-display-weight);
    line-height: var(--ss-type-display-line-height);
    letter-spacing: var(--ss-type-display-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-bright);
    margin: 0;
  }

  .no-device-badge {
    display: inline-flex;
    align-items: center;
    height: 18px;
    padding: 0 var(--ss-space-2);
    background: var(--ss-danger-soft);
    border: var(--ss-border-width) solid rgba(229, 72, 77, 0.35);
    border-radius: var(--ss-radius-pill);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-danger);
  }

  .active-profile-hint {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    margin: 0;
  }

  .profile-accent {
    color: var(--ss-text-secondary);
    font-weight: 600;
  }

  /* ===== Channel strips ===== */
  .channels-container {
    overflow-x: auto;
    /* Subtle scroll fade on the right edge */
    -webkit-overflow-scrolling: touch;
  }

  .channels-row {
    display: flex;
    gap: var(--ss-space-3);
    padding-bottom: var(--ss-space-2); /* room for scrollbar */
    min-width: min-content;
  }

  .channels-empty {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-tertiary);
    font-style: italic;
    margin: 0;
  }

  /* ===== Routes ===== */
  /* .routes-container — RouteList is a self-contained card, no extra styles needed */

  @media (prefers-reduced-motion: reduce) {
    .loading-spinner {
      animation: none;
      border-top-color: var(--ss-text-tertiary);
    }
  }
</style>
