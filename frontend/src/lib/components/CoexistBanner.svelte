<script lang="ts">
  import { coexistStatus, coexistDisable, type CoexistReport, type CoexistDisableResult } from "../ipc.js";

  /** Whether the banner has been dismissed by the user this session. */
  let dismissed = $state(false);

  /** The detection report from the daemon. null = not yet loaded. */
  let report = $state<CoexistReport | null>(null);

  /** Set when disable is in progress. */
  let disabling = $state(false);

  /** Set after disable completes. */
  let disableResult = $state<CoexistDisableResult | null>(null);

  /** Set when disable fails. */
  let disableError = $state<string | null>(null);

  // Load the coexist status once on mount.
  // Non-fatal: if the daemon is unavailable, we silently skip the banner.
  $effect(() => {
    coexistStatus()
      .then((r) => {
        report = r;
      })
      .catch(() => {
        // Daemon not available or legacy check failed — banner stays hidden.
        report = null;
      });
  });

  async function handleDisable() {
    if (disabling) return;
    disabling = true;
    disableError = null;
    disableResult = null;
    try {
      const result = await coexistDisable(false);
      disableResult = result;
      if (result.all_ok) {
        // Re-check: legacy stack should now be gone.
        try {
          report = await coexistStatus();
        } catch {
          report = null;
        }
      }
    } catch (e) {
      disableError = e instanceof Error ? e.message : String(e);
    } finally {
      disabling = false;
    }
  }

  function dismiss() {
    dismissed = true;
  }

  const showBanner = $derived(
    !dismissed && report !== null && report.any_detected && disableResult === null
  );
  const showResult = $derived(disableResult !== null && !dismissed);
</script>

{#if showBanner}
  <div class="coexist-banner" role="alert" aria-live="polite">
    <div class="banner-icon" aria-hidden="true">⚠</div>
    <div class="banner-body">
      <span class="banner-title">Legacy SteelSeries stack detected</span>
      <span class="banner-detail">
        {#if report!.legacy_loopbacks.length > 0}
          Loopback nodes: {report!.legacy_loopbacks.join(", ")}.
        {/if}
        {#if report!.hrir_switch_present}
          hrir-switch script present.
        {/if}
        This may conflict with Arctis Sound Manager.
      </span>
    </div>
    <div class="banner-actions">
      <button
        class="btn-disable"
        onclick={handleDisable}
        disabled={disabling}
        aria-busy={disabling}
      >
        {disabling ? "Disabling…" : "Disable it"}
      </button>
      <button class="btn-dismiss" onclick={dismiss} aria-label="Dismiss banner">
        ✕
      </button>
    </div>
  </div>
{/if}

{#if showResult}
  <div
    class="coexist-result"
    class:result-ok={disableResult!.all_ok}
    class:result-fail={!disableResult!.all_ok}
    role="status"
    aria-live="polite"
  >
    <div class="result-body">
      {#if disableResult!.all_ok}
        <span class="result-icon" aria-hidden="true">✓</span>
        <span>Legacy stack disabled ({disableResult!.successes}/{disableResult!.actions_attempted} actions).</span>
      {:else}
        <span class="result-icon" aria-hidden="true">✗</span>
        <span>
          Partial: {disableResult!.successes}/{disableResult!.actions_attempted} actions succeeded.
          {disableResult!.failures.length} failed.
        </span>
      {/if}
      {#if disableResult!.owner_note}
        <span class="result-note">{disableResult!.owner_note}</span>
      {/if}
    </div>
    <button class="btn-dismiss" onclick={dismiss} aria-label="Dismiss result">✕</button>
  </div>
{/if}

{#if disableError}
  <div class="coexist-error" role="alert">
    <span>Error disabling legacy stack: {disableError}</span>
    <button class="btn-dismiss" onclick={() => { disableError = null; }} aria-label="Dismiss error">✕</button>
  </div>
{/if}

<style>
  .coexist-banner,
  .coexist-result,
  .coexist-error {
    display: flex;
    align-items: center;
    gap: var(--ss-space-3);
    padding: var(--ss-space-2) var(--ss-page-padding);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    border-bottom: var(--ss-border-width) solid var(--ss-border);
  }

  .coexist-banner {
    background: color-mix(in srgb, var(--ss-warning) 12%, var(--ss-bg-root));
    color: var(--ss-text-primary);
  }

  .coexist-result.result-ok {
    background: color-mix(in srgb, var(--ss-success) 12%, var(--ss-bg-root));
    color: var(--ss-text-primary);
  }

  .coexist-result.result-fail {
    background: color-mix(in srgb, var(--ss-danger) 12%, var(--ss-bg-root));
    color: var(--ss-text-primary);
  }

  .coexist-error {
    background: color-mix(in srgb, var(--ss-danger) 16%, var(--ss-bg-root));
    color: var(--ss-text-primary);
  }

  .banner-icon,
  .result-icon {
    font-size: 14px;
    flex-shrink: 0;
    color: var(--ss-warning);
  }

  .result-ok .result-icon {
    color: var(--ss-success);
  }

  .result-fail .result-icon {
    color: var(--ss-danger);
  }

  .banner-body,
  .result-body {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--ss-space-2);
    flex: 1;
    min-width: 0;
  }

  .banner-title {
    font-weight: 600;
    color: var(--ss-text-bright);
    white-space: nowrap;
  }

  .banner-detail,
  .result-note {
    color: var(--ss-text-secondary);
    font-size: var(--ss-type-micro-size);
  }

  .banner-actions {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    flex-shrink: 0;
  }

  .btn-disable {
    padding: var(--ss-space-1) var(--ss-space-3);
    background: var(--ss-accent);
    color: var(--ss-text-bright);
    border: none;
    border-radius: var(--ss-radius-sm);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    font-weight: 600;
    cursor: pointer;
    transition: opacity var(--ss-dur-fast) var(--ss-ease-standard);
    white-space: nowrap;
  }

  .btn-disable:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-disable:hover:not(:disabled) {
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
