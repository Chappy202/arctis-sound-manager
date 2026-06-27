<script lang="ts">
  import { engineState, loadError, init, destroy } from "../stores.js";
  import { channelAdd, channelRemove, moveStream, clearRoute } from "../ipc.js";
  import ChannelStrip from "./ChannelStrip.svelte";
  import RouteList from "./RouteList.svelte";
  import { streamsStore, initStreams, destroyStreams } from "../stores/streams.js";
  import { outputsStore, initOutputs, destroyOutputs } from "../stores/outputs.js";
  import { groupStreamsByChannel } from "../streams.js";
  import MasterStrip from "./MasterStrip.svelte";
  import MicStrip from "./MicStrip.svelte";
  import ChatmixSlider from "./ChatmixSlider.svelte";

  // init() is idempotent — safe to call here AND from AppShell.
  // Calling it here ensures the mixer works even if AppShell hasn't yet mounted.
  $effect(() => {
    init();
  });

  $effect(() => {
    initStreams();
    initOutputs();
    return () => {
      destroyStreams();
      destroyOutputs();
    };
  });

  let grouped = $derived(
    groupStreamsByChannel(
      $streamsStore,
      ($engineState?.channels ?? []).map((c) => c.id),
    ),
  );

  // Fix 3: visible error banner for drop/clear failures.
  let dropError = $state<string | null>(null);

  async function handleDropStream(streamId: string, channelId: string) {
    dropError = null;
    // Fix 2: optimistic pill move — snap and apply before the await.
    const snapshot = $streamsStore;
    streamsStore.update((list) =>
      list.map((s) => (String(s.id) === streamId ? { ...s, current_channel: channelId } : s)),
    );
    try {
      const result = await moveStream(streamId, channelId);
      engineState.set(result);
    } catch (e) {
      streamsStore.set(snapshot); // revert optimistic update
      dropError = e instanceof Error ? e.message : "Failed to move app";
      console.error("[mixer] moveStream failed:", e);
    }
  }
  async function handleClearStream(streamId: string) {
    dropError = null;
    // streamId is a node id; resolve to binary for clearRoute.
    const s = $streamsStore.find((x) => String(x.id) === streamId);
    if (!s) return;
    // Fix 2: optimistic pill clear.
    const snapshot = $streamsStore;
    streamsStore.update((list) =>
      list.map((x) => (String(x.id) === streamId ? { ...x, current_channel: null } : x)),
    );
    try {
      engineState.set(await clearRoute(s.binary));
    } catch (e) {
      streamsStore.set(snapshot); // revert optimistic update
      dropError = e instanceof Error ? e.message : "Failed to move app";
      console.error("[mixer] clearRoute failed:", e);
    }
  }

  function refresh() {
    destroy();
    init();
  }

  // ── F4: Add channel ───────────────────────────────────────────────────────
  let addId = $state("");
  let addBusy = $state(false);
  let addError = $state<string | null>(null);

  async function handleAddChannel() {
    const id = addId.trim();
    if (!id) return;
    addBusy = true;
    addError = null;
    try {
      const newState = await channelAdd(id);
      if (newState) {
        engineState.set(newState);
      }
      addId = "";
    } catch (err: unknown) {
      addError = err instanceof Error ? err.message : String(err);
    } finally {
      addBusy = false;
    }
  }

  function handleAddKeydown(e: KeyboardEvent) {
    if (e.key === "Enter") {
      handleAddChannel();
    }
  }

  // ── F4: Remove channel ────────────────────────────────────────────────────
  let removeBusy = $state(false);

  async function handleRemoveChannel(id: string) {
    if (!confirm(`Remove channel "${id}"? This cannot be undone.`)) return;
    removeBusy = true;
    try {
      const newState = await channelRemove(id);
      if (newState) {
        engineState.set(newState);
      }
    } catch (err: unknown) {
      console.error("[MixerPage] channelRemove failed:", err);
    } finally {
      removeBusy = false;
    }
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
          <MasterStrip mixerState={$engineState} unrouted={grouped.unrouted}
            onClearStream={handleClearStream} />

          {#each $engineState.channels as channel (channel.id)}
            <div role="listitem">
              <ChannelStrip
                {channel}
                streams={grouped.byChannel[channel.id] ?? []}
                outputDevices={$outputsStore}
                onDropStream={handleDropStream}
                onOutputChanged={() => {
                  // State will be refreshed via state-changed event.
                  // No extra action needed here.
                }}
                onRemove={$engineState.channels.length > 1 && !removeBusy
                  ? () => handleRemoveChannel(channel.id)
                  : undefined}
              />
            </div>
          {/each}

          <MicStrip mic={$engineState.mic} />

          <!-- ===== Add channel affordance ===== -->
          <div class="add-channel-strip" role="listitem">
            <p class="add-channel-label">ADD CHANNEL</p>
            <div class="add-channel-row">
              <input
                class="add-channel-input"
                type="text"
                placeholder="id (e.g. aux)"
                bind:value={addId}
                disabled={addBusy}
                aria-label="New channel id"
                onkeydown={handleAddKeydown}
              />
              <button
                class="add-channel-btn"
                disabled={addBusy || !addId.trim()}
                aria-label="Add channel"
                onclick={handleAddChannel}
              >+</button>
            </div>
            {#if addError}
              <p class="add-channel-error" role="alert">{addError}</p>
            {/if}
          </div>
        </div>
      {/if}
    </div>

    <!-- ===== Drop / clear error banner (Fix 3) ===== -->
    {#if dropError}
      <p class="drop-error" role="alert">
        {dropError}
        <button class="drop-error-dismiss" onclick={() => (dropError = null)} aria-label="Dismiss error">✕</button>
      </p>
    {/if}

    <!-- ===== ChatMix slider ===== -->
    <!-- hardwareActive: grey-out only when device present AND dial owns balance (Fix 1) -->
    <ChatmixSlider position={$engineState.chatmix_position}
      hardwareActive={$engineState.device_present && $engineState.dial_controls_balance} />

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

  /* ===== Add channel strip ===== */
  .add-channel-strip {
    display: flex;
    flex-direction: column;
    width: var(--ss-channel-strip-w, 120px);
    min-width: var(--ss-channel-strip-w-min, 100px);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) dashed var(--ss-border);
    border-radius: var(--ss-radius-md);
    padding: var(--ss-space-3);
    gap: var(--ss-space-2);
    flex-shrink: 0;
    justify-content: flex-start;
  }

  .add-channel-label {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-tertiary);
    margin: 0;
  }

  .add-channel-row {
    display: flex;
    gap: var(--ss-space-1);
  }

  .add-channel-input {
    flex: 1;
    min-width: 0;
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-2);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
  }

  .add-channel-input:focus {
    outline: none;
    border-color: var(--ss-accent-border);
  }

  .add-channel-input:disabled {
    cursor: not-allowed;
    color: var(--ss-text-disabled);
  }

  .add-channel-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: var(--ss-control-h-sm);
    height: var(--ss-control-h-sm);
    background: var(--ss-gradient-primary);
    border: none;
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-bright);
    font-size: 18px;
    line-height: 1;
    cursor: pointer;
    flex-shrink: 0;
    transition: filter var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .add-channel-btn:hover:not(:disabled) {
    filter: brightness(1.15);
  }

  .add-channel-btn:disabled {
    cursor: not-allowed;
    opacity: 0.4;
  }

  .add-channel-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .add-channel-error {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-danger);
    margin: 0;
    word-break: break-word;
  }

  /* ===== Drop / clear error banner (Fix 3) ===== */
  .drop-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--ss-space-3);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-danger);
    background: var(--ss-danger-soft);
    border: var(--ss-border-width) solid rgba(229, 72, 77, 0.3);
    border-radius: var(--ss-radius-xs);
    padding: var(--ss-space-2) var(--ss-space-3);
    margin: 0;
  }

  .drop-error-dismiss {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    padding: 0;
    background: transparent;
    border: none;
    border-radius: var(--ss-radius-xs);
    color: var(--ss-danger);
    font-size: 11px;
    cursor: pointer;
    flex-shrink: 0;
  }

  .drop-error-dismiss:hover {
    background: rgba(229, 72, 77, 0.15);
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
