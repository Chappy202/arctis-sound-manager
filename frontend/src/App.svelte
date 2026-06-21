<script lang="ts">
  import AppShell from './lib/components/AppShell.svelte';
  import MixerPage from './lib/components/MixerPage.svelte';
  import EqPage from './lib/components/EqPage.svelte';
  import DevicePage from './lib/components/DevicePage.svelte';
  import SpatialPage from './lib/components/SpatialPage.svelte';
  import MicPage from './lib/components/MicPage.svelte';
  import { currentPage } from './lib/stores/page.js';
</script>

<AppShell>
  {#if $currentPage === 'mixer'}
    <MixerPage />
  {:else if $currentPage === 'eq'}
    <EqPage />
  {:else if $currentPage === 'device'}
    <DevicePage />
  {:else if $currentPage === 'spatial'}
    <SpatialPage />
  {:else if $currentPage === 'mic'}
    <MicPage />
  {/if}
</AppShell>
