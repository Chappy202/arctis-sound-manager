<script lang="ts">
  import { engineState, init, destroy } from "../stores.js";
  import { channelAdd, channelRemove, moveStream, clearRoute } from "../ipc.js";
  import ChannelStrip from "./ChannelStrip.svelte";
  import RouteList from "./RouteList.svelte";
  import { streamsStore, initStreams, destroyStreams } from "../stores/streams.js";
  import { outputsStore, initOutputs, destroyOutputs } from "../stores/outputs.js";
  import { groupStreamsByChannel } from "../streams.js";
  import MasterStrip from "./MasterStrip.svelte";
  import MicStrip from "./MicStrip.svelte";
  import ChatmixSlider from "./ChatmixSlider.svelte";
  import { orderChannels } from "./mixerLayout.js";
  import DaemonUnavailable from "./DaemonUnavailable.svelte";
  import { connectionStatus } from "../stores/connection.js";

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

  // Memoize the canonical channel order so it's computed once per state change
  // instead of inline in the {#each}; the keyed block then reuses ChannelStrip
  // instances by id.
  let orderedChannels = $derived(orderChannels($engineState?.channels ?? []));

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
  let addOpen = $state(false);

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
      addOpen = false;
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
      dropError = err instanceof Error ? err.message : String(err);
    } finally {
      removeBusy = false;
    }
  }
</script>

<div class="mixer-page">
  {#if $connectionStatus !== "connected"}
    <DaemonUnavailable />
  {:else}
    <!-- ===== Mixer content ===== -->
    <!-- $engineState is always non-null when connectionStatus === "connected" -->
    {#if $engineState}
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
    {#if $engineState.channels.length === 0}
      <p class="channels-empty">No channels reported by the daemon.</p>
    {:else}
      <div
        class="channels-row"
        role="list"
        aria-label="Audio channel strips"
      >
        <!-- Master (role="listitem" is on MasterStrip's root element) -->
        <MasterStrip
          mixerState={$engineState}
          unrouted={grouped.unrouted}
          onClearStream={handleClearStream}
          onError={(m) => (dropError = m)}
        />

        <!-- Standard + custom channels in canonical order -->
        {#each orderedChannels as channel (channel.id)}
          <!-- role="listitem" is on ChannelStrip's root <article> element -->
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
            onError={(m) => (dropError = m)}
          />
        {/each}

        <!-- Mic (role="listitem" is on MicStrip's root element) -->
        <MicStrip mic={$engineState.mic} onError={(m) => (dropError = m)} />

        <!-- ===== Add channel — compact end-of-row affordance ===== -->
        <div
          class="add-affordance"
          class:add-affordance--open={addOpen}
          role="listitem"
        >
          {#if !addOpen}
            <button
              class="add-open-btn"
              aria-label="Add custom channel"
              title="Add custom channel"
              onclick={() => { addOpen = true; addError = null; }}
            >+</button>
          {:else}
            <div class="add-form">
              <p class="add-form-label">ADD CHANNEL</p>
              <div class="add-form-row">
                <input
                  class="add-form-input"
                  type="text"
                  placeholder="id (e.g. aux)"
                  bind:value={addId}
                  disabled={addBusy}
                  aria-label="New channel id"
                  onkeydown={handleAddKeydown}
                />
                <button
                  class="add-form-submit"
                  disabled={addBusy || !addId.trim()}
                  aria-label="Add channel"
                  onclick={handleAddChannel}
                >+</button>
              </div>
              {#if addError}
                <p class="add-form-error" role="alert">{addError}</p>
              {/if}
              <button
                class="add-form-cancel"
                aria-label="Cancel add channel"
                onclick={() => { addOpen = false; addError = null; addId = ""; }}
              >Cancel</button>
            </div>
          {/if}
        </div>
      </div>
    {/if}

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
      hardwareActive={$engineState.device_present && $engineState.dial_controls_balance}
      onError={(m) => (dropError = m)} />

    <!-- ===== Route list — collapsible, de-emphasised ===== -->
    <details class="routes-disclosure">
      <summary class="routes-summary">Remembered routes</summary>
      <div class="routes-body">
        <RouteList />
      </div>
    </details>
    {/if}<!-- /$engineState inner guard -->
  {/if}<!-- /$connectionStatus gate -->
</div>

<style>
  .mixer-page {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-6);
    min-height: 100%;
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

  /* ===== Channel strips — full-width flex row ===== */
  .channels-row {
    display: flex;
    gap: var(--ss-space-3);
    width: 100%;
    flex-wrap: wrap;
    align-items: stretch;
  }

  .channels-empty {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-tertiary);
    font-style: italic;
    margin: 0;
  }

  /* ===== Add-channel affordance (end-of-row) ===== */
  .add-affordance {
    display: flex;
    flex-direction: column;
    flex: 0 0 auto;
    align-self: stretch;
  }

  /* Closed state: slim "+" button, same height as the strip row */
  .add-open-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 40px;
    height: 100%;
    min-height: 60px;
    background: var(--ss-surface-1);
    border: var(--ss-border-width) dashed var(--ss-border);
    border-radius: var(--ss-radius-md);
    color: var(--ss-text-tertiary);
    font-size: 20px;
    line-height: 1;
    cursor: pointer;
    transition:
      border-color var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard),
      background var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .add-open-btn:hover {
    border-color: var(--ss-border-strong);
    color: var(--ss-text-secondary);
    background: var(--ss-surface-2);
  }

  .add-open-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  /* Open state: compact inline form */
  .add-affordance--open {
    flex: 0 0 auto;
    min-width: 160px;
  }

  .add-form {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-2);
    padding: var(--ss-space-3);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) dashed var(--ss-border);
    border-radius: var(--ss-radius-md);
    height: 100%;
    box-sizing: border-box;
  }

  .add-form-label {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-tertiary);
    margin: 0;
  }

  .add-form-row {
    display: flex;
    gap: var(--ss-space-1);
  }

  .add-form-input {
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

  .add-form-input:focus {
    outline: none;
    border-color: var(--ss-accent-border);
  }

  .add-form-input:disabled {
    cursor: not-allowed;
    color: var(--ss-text-disabled);
  }

  .add-form-submit {
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

  .add-form-submit:hover:not(:disabled) {
    filter: brightness(1.15);
  }

  .add-form-submit:disabled {
    cursor: not-allowed;
    opacity: 0.4;
  }

  .add-form-submit:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .add-form-error {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-danger);
    margin: 0;
    word-break: break-word;
  }

  .add-form-cancel {
    display: flex;
    align-items: center;
    justify-content: center;
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-2);
    background: transparent;
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-tertiary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    cursor: pointer;
    transition:
      border-color var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .add-form-cancel:hover {
    border-color: var(--ss-border-strong);
    color: var(--ss-text-secondary);
  }

  .add-form-cancel:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
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

  /* ===== Routes — collapsible section ===== */
  .routes-disclosure {
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    background: var(--ss-surface-1);
    overflow: hidden;
  }

  .routes-summary {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    padding: var(--ss-space-3) var(--ss-space-4);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-tertiary);
    cursor: pointer;
    user-select: none;
    list-style: none;
    transition: color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  /* Remove default disclosure triangle in WebKit */
  .routes-summary::-webkit-details-marker {
    display: none;
  }

  /* Custom chevron via ::before */
  .routes-summary::before {
    content: "›";
    display: inline-block;
    font-size: 14px;
    line-height: 1;
    color: var(--ss-text-tertiary);
    transition: transform var(--ss-dur-fast) var(--ss-ease-standard);
    transform: rotate(0deg);
  }

  .routes-disclosure[open] .routes-summary::before {
    transform: rotate(90deg);
  }

  .routes-summary:hover {
    color: var(--ss-text-secondary);
  }

  .routes-summary:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: -2px;
  }

  .routes-body {
    padding: 0 var(--ss-space-4) var(--ss-space-4);
  }


</style>
