<script lang="ts">
  import AppShell from './lib/components/AppShell.svelte';
  import MixerPage from './lib/components/MixerPage.svelte';
  import { currentPage } from './lib/stores/page.js';
</script>

<AppShell>
  {#if $currentPage === 'mixer'}
    <MixerPage />
  {:else if $currentPage === 'eq'}
    <div class="placeholder-page">
      <h1 class="page-title">EQ</h1>
      <p class="page-desc">Parametric EQ canvas · Up to 10 bands · Presets</p>
      <p class="page-hint">EQ page — coming in Task 6</p>
    </div>
  {:else if $currentPage === 'device'}
    <div class="placeholder-page">
      <h1 class="page-title">DEVICE</h1>
      <p class="page-desc">Battery · ANC · Sidetone · Mic · Power · Firmware</p>
      <p class="page-hint">Device page — coming in Task 7</p>
    </div>
  {:else if $currentPage === 'spatial'}
    <div class="placeholder-page">
      <h1 class="page-title">SPATIAL</h1>
      <p class="page-desc">Spatial audio · Presets · Room controls</p>
      <p class="page-hint">Spatial page — coming in a future task</p>
    </div>
  {:else if $currentPage === 'mic'}
    <div class="placeholder-page">
      <h1 class="page-title">MIC</h1>
      <p class="page-desc">Input level · Gain · Sidetone · Noise suppression</p>
      <p class="page-hint">Mic page — coming in a future task</p>
    </div>
  {/if}
</AppShell>

<style>
  .placeholder-page {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-4);
    padding: var(--ss-space-8) 0;
  }

  .page-title {
    font-family: var(--ss-font-display);
    font-size: var(--ss-type-display-size);
    font-weight: var(--ss-type-display-weight);
    line-height: var(--ss-type-display-line-height);
    letter-spacing: var(--ss-type-display-letter-spacing);
    text-transform: var(--ss-type-display-transform);
    color: var(--ss-text-bright);
  }

  .page-desc {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-lg-size);
    color: var(--ss-text-secondary);
  }

  .page-hint {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    padding: var(--ss-space-3) var(--ss-space-4);
    background: var(--ss-surface-1);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    display: inline-block;
  }
</style>
