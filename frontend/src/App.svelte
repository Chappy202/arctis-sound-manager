<script lang="ts">
  import { onMount } from 'svelte';
  import AppShell from './lib/components/AppShell.svelte';
  import MixerPage from './lib/components/MixerPage.svelte';
  import EqPage from './lib/components/EqPage.svelte';
  import DevicePage from './lib/components/DevicePage.svelte';
  import SpatialPage from './lib/components/SpatialPage.svelte';
  import MicPage from './lib/components/MicPage.svelte';
  import { currentPage } from './lib/stores/page.js';
  import { checkForUpdate, type UpdateInfo } from './lib/updater.js';

  let pendingUpdate: UpdateInfo | null = null;
  let updateInstalling = false;

  onMount(() => {
    // Background update check — non-blocking, errors are swallowed in checkForUpdate.
    checkForUpdate().then((update) => {
      if (update) {
        pendingUpdate = update;
      }
    });
  });

  async function installUpdate() {
    if (!pendingUpdate) return;
    updateInstalling = true;
    try {
      await pendingUpdate.downloadAndInstall();
      // App relaunches automatically after install.
    } catch (err) {
      console.error('[updater] install failed:', err);
      updateInstalling = false;
    }
  }
</script>

<AppShell>
  {#if pendingUpdate}
    <!-- Update available banner — minimal affordance, user must confirm. -->
    <div class="update-banner" role="status">
      <span>Update available: v{pendingUpdate.version}</span>
      <button onclick={installUpdate} disabled={updateInstalling}>
        {updateInstalling ? 'Installing…' : 'Install & Relaunch'}
      </button>
    </div>
  {/if}

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

<style>
  .update-banner {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.5rem 1rem;
    background: #1a3a5c;
    color: #90cdf4;
    font-size: 0.85rem;
    border-bottom: 1px solid #2d6a9f;
  }
  .update-banner button {
    padding: 0.2rem 0.75rem;
    background: #2d6a9f;
    color: #e8f4fd;
    border: none;
    border-radius: 4px;
    cursor: pointer;
    font-size: 0.8rem;
  }
  .update-banner button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
</style>
