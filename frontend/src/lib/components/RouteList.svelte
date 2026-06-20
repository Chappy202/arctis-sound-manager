<script lang="ts">
  import { setRoute } from "../ipc.js";
  import { engineState } from "../stores.js";

  let routes = $derived($engineState?.routes ?? []);

  let newApp = $state("");
  let newSink = $state("");
  let adding = $state(false);
  let addError = $state<string | null>(null);

  async function handleAdd() {
    const app = newApp.trim();
    const sink = newSink.trim();
    if (!app || !sink || adding) return;

    adding = true;
    addError = null;
    try {
      const next = await setRoute(app, sink);
      engineState.set(next);
      newApp = "";
      newSink = "";
    } catch (e) {
      addError = e instanceof Error ? e.message : "Failed to add route";
    } finally {
      adding = false;
    }
  }

  function handleAddKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      handleAdd();
    }
  }
</script>

<section class="route-list-card" aria-labelledby="route-list-heading">
  <header class="route-header">
    <h2 class="route-heading" id="route-list-heading">APP ROUTES</h2>
    <span class="route-count" aria-label="{routes.length} routes">{routes.length}</span>
  </header>

  {#if routes.length === 0}
    <p class="route-empty">No routes configured. Add one below to redirect an app's audio.</p>
  {:else}
    <ul class="route-table" role="list" aria-label="Configured app routes">
      <li class="route-table-head" aria-hidden="true">
        <span>APP BINARY</span>
        <span>→</span>
        <span>TARGET SINK</span>
      </li>
      {#each routes as [app, sink], i}
        <li class="route-row" aria-label="{app} routed to {sink}">
          <span class="route-app" title={app}>{app}</span>
          <span class="route-arrow" aria-hidden="true">→</span>
          <span class="route-sink" title={sink}>{sink}</span>
        </li>
      {/each}
    </ul>
  {/if}

  <!-- Add route form -->
  <form class="route-form" onsubmit={(e) => { e.preventDefault(); handleAdd(); }}>
    <div class="route-inputs">
      <div class="route-field">
        <label class="field-label" for="route-app">APP BINARY</label>
        <input
          id="route-app"
          type="text"
          class="route-input"
          placeholder="e.g. spotify"
          bind:value={newApp}
          disabled={adding}
          onkeydown={handleAddKeydown}
          aria-label="Application binary name"
          autocomplete="off"
          spellcheck={false}
        />
      </div>
      <span class="form-arrow" aria-hidden="true">→</span>
      <div class="route-field">
        <label class="field-label" for="route-sink">TARGET SINK</label>
        <input
          id="route-sink"
          type="text"
          class="route-input"
          placeholder="e.g. game"
          bind:value={newSink}
          disabled={adding}
          onkeydown={handleAddKeydown}
          aria-label="Target audio sink"
          autocomplete="off"
          spellcheck={false}
        />
      </div>
      <button
        type="submit"
        class="route-add-btn"
        disabled={adding || !newApp.trim() || !newSink.trim()}
        aria-label="Add route"
      >
        {adding ? "…" : "Add"}
      </button>
    </div>

    {#if addError}
      <p class="route-error" role="alert">{addError}</p>
    {/if}
  </form>
</section>

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
    grid-template-columns: 1fr auto 1fr;
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
    grid-template-columns: 1fr auto 1fr;
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

  /* ===== Add route form ===== */
  .route-form {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-2);
    border-top: var(--ss-border-width) solid var(--ss-border);
    padding-top: var(--ss-space-3);
  }

  .route-inputs {
    display: flex;
    align-items: flex-end;
    gap: var(--ss-space-3);
    flex-wrap: wrap;
  }

  .route-field {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-1);
    flex: 1;
    min-width: 120px;
  }

  .field-label {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-tertiary);
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

  .form-arrow {
    font-family: var(--ss-font-mono);
    font-size: 16px;
    color: var(--ss-text-tertiary);
    padding-bottom: calc((var(--ss-field-h) - 1em) / 2);
  }

  .route-add-btn {
    height: var(--ss-field-h);
    padding: 0 var(--ss-space-5);
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
    transition: opacity var(--ss-dur-fast) var(--ss-ease-standard);
    flex-shrink: 0;
    align-self: flex-end;
  }

  .route-add-btn:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .route-add-btn:active:not(:disabled) {
    filter: brightness(0.9);
  }

  .route-add-btn:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .route-add-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .route-error {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-danger);
    margin: 0;
    padding: var(--ss-space-2) var(--ss-space-3);
    background: var(--ss-danger-soft);
    border-radius: var(--ss-radius-xs);
    border: var(--ss-border-width) solid rgba(229, 72, 77, 0.3);
  }
</style>
