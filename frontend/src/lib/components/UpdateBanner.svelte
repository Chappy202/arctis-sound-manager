<script lang="ts">
  import {
    checkForUpdate,
    reduceProgress,
    progressLabel,
    initialProgress,
    type UpdateInfo,
    type DownloadProgress,
  } from "../updater.js";

  /** The available update, or null when up to date / offline. */
  let pendingUpdate = $state<UpdateInfo | null>(null);

  /** True from the moment install starts until relaunch (or failure). */
  let installing = $state(false);

  /** Live download/install progress. */
  let progress = $state<DownloadProgress>(initialProgress);

  /** Set when the install fails, so the user can retry. */
  let errorMsg = $state<string | null>(null);

  /** Dismissed for this session (only when not installing). */
  let dismissed = $state(false);

  // Background update check on mount. Non-fatal: errors are swallowed inside
  // checkForUpdate (returns null), so a failed check just hides the banner.
  $effect(() => {
    checkForUpdate().then((update) => {
      if (update) pendingUpdate = update;
    });
  });

  async function installUpdate() {
    if (!pendingUpdate || installing) return;
    installing = true;
    errorMsg = null;
    progress = initialProgress;
    try {
      await pendingUpdate.downloadAndInstall((e) => {
        progress = reduceProgress(progress, e);
      });
      // On success the app relaunches automatically — nothing more to do.
    } catch (err) {
      // Timeout / network / signature failure — surface it and let the user retry.
      console.error("[updater] install failed:", err);
      errorMsg = err instanceof Error ? err.message : String(err);
      installing = false;
      progress = initialProgress;
    }
  }

  const label = $derived(installing ? progressLabel(progress) : "Install & Relaunch");
  const showBanner = $derived(pendingUpdate !== null && (!dismissed || installing));
</script>

{#if showBanner}
  <div class="update-banner" role="status" aria-live="polite">
    <!-- Progress fill sits behind the content while installing. -->
    {#if installing && progress.percent !== null}
      <div class="progress-fill" style="width: {progress.percent}%"></div>
    {:else if installing}
      <div class="progress-fill indeterminate"></div>
    {/if}

    <div class="banner-content">
      <span class="banner-icon" aria-hidden="true">⭑</span>
      <span class="banner-title">Update available</span>
      <span class="banner-version">v{pendingUpdate!.version}</span>
      {#if errorMsg}
        <span class="banner-error" role="alert">Install failed: {errorMsg}</span>
      {/if}

      <div class="banner-actions">
        <button
          class="btn-install"
          onclick={installUpdate}
          disabled={installing}
          aria-busy={installing}
        >
          {label}
        </button>
        {#if !installing}
          <button
            class="btn-dismiss"
            onclick={() => (dismissed = true)}
            aria-label="Dismiss update notification"
          >
            ✕
          </button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  /* Full-width bar pinned above the whole app (rendered at window top by
     AppShell, outside the content container — so no page padding / max-width). */
  .update-banner {
    position: relative;
    width: 100%;
    background: color-mix(in srgb, var(--ss-accent) 14%, var(--ss-bg-root));
    border-bottom: var(--ss-border-width) solid var(--ss-border);
    overflow: hidden;
  }

  .progress-fill {
    position: absolute;
    inset: 0 auto 0 0;
    background: var(--ss-accent-soft);
    transition: width var(--ss-dur-fast) var(--ss-ease-standard);
    z-index: 0;
  }

  .progress-fill.indeterminate {
    width: 35%;
    animation: indeterminate 1.2s ease-in-out infinite;
  }

  @keyframes indeterminate {
    0% { transform: translateX(-100%); }
    100% { transform: translateX(320%); }
  }

  .banner-content {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: var(--ss-space-3);
    padding: var(--ss-space-2) var(--ss-page-padding);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
  }

  .banner-icon {
    color: var(--ss-accent);
    flex-shrink: 0;
  }

  .banner-title {
    font-weight: 600;
    color: var(--ss-text-bright);
    white-space: nowrap;
  }

  .banner-version {
    font-family: var(--ss-font-mono);
    color: var(--ss-text-secondary);
    font-variant-numeric: tabular-nums;
  }

  .banner-error {
    color: var(--ss-danger);
    font-size: var(--ss-type-micro-size);
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .banner-actions {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    margin-left: auto;
    flex-shrink: 0;
  }

  .btn-install {
    padding: var(--ss-space-1) var(--ss-space-3);
    background: var(--ss-accent);
    color: var(--ss-text-bright);
    border: none;
    border-radius: var(--ss-radius-sm);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
    transition: opacity var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .btn-install:disabled {
    opacity: 0.75;
    cursor: default;
  }

  .btn-install:hover:not(:disabled) {
    opacity: 0.85;
  }

  .btn-dismiss {
    padding: var(--ss-space-1);
    background: none;
    border: none;
    color: var(--ss-text-tertiary);
    font-size: 12px;
    cursor: pointer;
    border-radius: var(--ss-radius-xs);
    line-height: 1;
    flex-shrink: 0;
    transition: color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .btn-dismiss:hover {
    color: var(--ss-text-secondary);
  }
</style>
