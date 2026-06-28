<script lang="ts">
  import { clearRoute } from "../ipc.js";
  import { engineState } from "../stores.js";

  let routes = $derived($engineState?.routes ?? []);

  // Per-row remove state: set of app binaries currently being removed
  let removing = $state(new Set<string>());
  let removeError = $state<string | null>(null);

  async function handleRemove(app: string) {
    if (removing.has(app)) return;
    removing = new Set([...removing, app]);
    removeError = null;
    try {
      const next = await clearRoute(app);
      engineState.set(next);
    } catch (e) {
      removeError = e instanceof Error ? e.message : "Failed to remove route";
    } finally {
      removing = new Set([...removing].filter((a) => a !== app));
    }
  }
</script>

<details class="remembered">
  <summary class="route-summary">Remembered routes ({routes.length})</summary>
  <section class="route-list-card">
    <header class="route-header">
      <h2 class="route-heading" id="route-list-heading">REMEMBERED ROUTES</h2>
      <span class="route-count" aria-label="{routes.length} routes">{routes.length}</span>
    </header>

    {#if routes.length === 0}
      <p class="route-empty">No routes remembered yet — drag an app onto a channel.</p>
    {:else}
      <ul class="route-table" role="list" aria-label="Remembered app routes">
        <li class="route-table-head" aria-hidden="true">
          <span>APP BINARY</span>
          <span>→</span>
          <span>TARGET SINK</span>
          <span></span>
        </li>
        {#each routes as [app, sink] (app)}
          <li class="route-row" aria-label="{app} routed to {sink}">
            <span class="route-app" title={app}>{app}</span>
            <span class="route-arrow" aria-hidden="true">→</span>
            <span class="route-sink" title={sink}>{sink}</span>
            <button
              class="route-remove-btn"
              disabled={removing.has(app)}
              onclick={() => handleRemove(app)}
              aria-label="Remove route for {app}"
              title="Remove route for {app}"
            >
              {removing.has(app) ? "…" : "✕"}
            </button>
          </li>
        {/each}
        {#if removeError}
          <li class="route-remove-error" role="alert">{removeError}</li>
        {/if}
      </ul>
    {/if}
  </section>
</details>

<style>
  .route-list-card {
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    padding: var(--ss-space-5);
    box-shadow: var(--ss-e1);
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-4);
  }

  /* ===== Header ===== */
  .route-header {
    display: flex;
    align-items: center;
    gap: var(--ss-space-3);
    border-bottom: var(--ss-border-width) solid var(--ss-border);
    padding-bottom: var(--ss-space-3);
  }

  .route-heading {
    font-family: var(--ss-font-display);
    font-size: var(--ss-type-h2-size);
    font-weight: var(--ss-type-h2-weight);
    letter-spacing: var(--ss-type-h2-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-primary);
    margin: 0;
  }

  .route-count {
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

  /* ===== Route table ===== */
  .route-table {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .route-table-head {
    display: grid;
    grid-template-columns: 1fr auto 1fr auto;
    gap: var(--ss-space-3);
    padding: 0 var(--ss-space-2) var(--ss-space-1);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-tertiary);
  }

  .route-row {
    display: grid;
    grid-template-columns: 1fr auto 1fr auto;
    gap: var(--ss-space-3);
    align-items: center;
    padding: var(--ss-space-2);
    border-radius: var(--ss-radius-xs);
    background: var(--ss-surface-2);
    border: var(--ss-border-width) solid var(--ss-border);
  }

  .route-row:hover {
    background: var(--ss-surface-3);
  }

  .route-remove-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    padding: 0;
    background: transparent;
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-tertiary);
    font-size: 11px;
    cursor: pointer;
    flex-shrink: 0;
    transition:
      color var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard),
      background var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .route-remove-btn:hover:not(:disabled) {
    color: var(--ss-danger);
    border-color: var(--ss-danger);
    background: var(--ss-danger-soft);
  }

  .route-remove-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .route-remove-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .route-remove-error {
    list-style: none;
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-danger);
    padding: var(--ss-space-2) var(--ss-space-3);
    background: var(--ss-danger-soft);
    border-radius: var(--ss-radius-xs);
    border: var(--ss-border-width) solid rgba(229, 72, 77, 0.3);
  }

  .route-app,
  .route-sink {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .route-arrow {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-tertiary);
    flex-shrink: 0;
  }

  .route-empty {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-tertiary);
    margin: 0;
    font-style: italic;
  }

  /* ===== Collapsible details wrapper ===== */
  .remembered {
    /* no extra chrome — the inner card provides the surface */
  }

  .route-summary {
    cursor: pointer;
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ss-text-tertiary);
    padding: var(--ss-space-2) 0;
    list-style: none;
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
  }

  .route-summary::-webkit-details-marker { display: none; }

  .route-summary::before {
    content: "▶";
    font-size: 10px;
    transition: transform var(--ss-dur-fast) var(--ss-ease-standard);
  }

  details[open] .route-summary::before {
    transform: rotate(90deg);
  }
</style>
