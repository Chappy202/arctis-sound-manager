<script lang="ts">
  import { Dialog } from "bits-ui";
  import { engineState } from "../stores.js";
  import { profileNew } from "../ipc.js";
  import { validateProfileName } from "./newProfileUtils.js";

  let open = $state(false);
  let name = $state("");
  let busy = $state(false);
  let error = $state<string | null>(null);
  let nameInput: HTMLInputElement | undefined = $state();

  const profiles = $derived($engineState?.profiles ?? []);
  const check = $derived(validateProfileName(name, profiles));

  // Reset form on close; focus input on open.
  $effect(() => {
    if (open) {
      requestAnimationFrame(() => nameInput?.focus());
    } else {
      name = "";
      error = null;
    }
  });

  async function create() {
    const v = validateProfileName(name, profiles);
    if (!v.ok) { error = v.error ?? "Invalid name"; return; }
    busy = true; error = null;
    try {
      engineState.set(await profileNew(v.name));
      open = false; name = "";
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally { busy = false; }
  }
</script>

<!-- "+" trigger button -->
<Dialog.Root bind:open>
  <Dialog.Trigger class="new-profile-trigger" aria-label="New profile">
    <span class="new-profile-plus" aria-hidden="true">+</span>
  </Dialog.Trigger>

  <Dialog.Portal>
    <Dialog.Overlay class="new-profile-overlay" />
    <Dialog.Content class="new-profile-content">
      <Dialog.Title class="new-profile-title">New profile</Dialog.Title>

      <div class="new-profile-body">
        <input
          bind:this={nameInput}
          class="new-profile-input"
          type="text"
          placeholder="Profile name…"
          maxlength={48}
          aria-label="Profile name"
          bind:value={name}
          onkeydown={(e) => {
            if (e.key === "Enter") { e.preventDefault(); create(); }
          }}
          disabled={busy}
        />
        {#if error}
          <p class="new-profile-error" role="alert">{error}</p>
        {/if}
      </div>

      <div class="new-profile-footer">
        <button
          class="btn-create"
          onclick={create}
          disabled={!check.ok || busy}
          aria-label="Create profile"
        >
          {busy ? "Creating…" : "Create"}
        </button>
        <Dialog.Close class="btn-cancel">Cancel</Dialog.Close>
      </div>
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>

<style>
  /* ===== "+" Trigger ===== */
  :global(.new-profile-trigger) {
    display: flex;
    align-items: center;
    justify-content: center;
    height: var(--ss-field-h);
    width: var(--ss-field-h);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-sm);
    cursor: pointer;
    color: var(--ss-accent);
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
    flex-shrink: 0;
  }

  :global(.new-profile-trigger:hover) {
    background: var(--ss-surface-2);
    border-color: var(--ss-border-strong);
  }

  :global(.new-profile-trigger:focus-visible) {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .new-profile-plus {
    font-size: 18px;
    font-weight: 700;
    line-height: 1;
    color: var(--ss-accent);
  }

  /* ===== Overlay ===== */
  :global(.new-profile-overlay) {
    position: fixed;
    inset: 0;
    z-index: 400;
    background: rgba(0, 0, 0, 0.6);
  }

  /* ===== Dialog Content ===== */
  :global(.new-profile-content) {
    position: fixed;
    left: 50%;
    top: 50%;
    z-index: 401;
    transform: translate(-50%, -50%);
    width: min(380px, calc(100vw - 2rem));
    background: var(--ss-surface-2);
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-md);
    box-shadow: var(--ss-e2);
    padding: var(--ss-space-5);
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-4);
    outline: none;
  }

  /* ===== Title ===== */
  :global(.new-profile-title) {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-h3-size);
    font-weight: var(--ss-type-h3-weight);
    letter-spacing: var(--ss-type-h3-letter-spacing);
    color: var(--ss-text-primary);
    margin: 0;
  }

  /* ===== Body ===== */
  .new-profile-body {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-2);
  }

  .new-profile-input {
    width: 100%;
    height: var(--ss-field-h);
    padding: 0 var(--ss-space-3);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-sm);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    box-sizing: border-box;
    transition: border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .new-profile-input:focus {
    outline: none;
    border-color: var(--ss-accent-border);
  }

  .new-profile-input:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .new-profile-error {
    margin: 0;
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-danger, #e5484d);
  }

  /* ===== Footer ===== */
  .new-profile-footer {
    display: flex;
    justify-content: flex-end;
    gap: var(--ss-space-2);
    align-items: center;
  }

  .btn-create {
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-4);
    background: var(--ss-gradient-primary);
    border: none;
    border-radius: var(--ss-radius-sm);
    color: var(--ss-text-bright);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-button-size);
    font-weight: var(--ss-type-button-weight);
    letter-spacing: var(--ss-type-button-letter-spacing);
    text-transform: var(--ss-type-button-transform);
    cursor: pointer;
    white-space: nowrap;
    transition: opacity var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .btn-create:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .btn-create:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .btn-create:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  :global(.btn-cancel) {
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-3);
    background: transparent;
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-sm);
    color: var(--ss-text-tertiary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    cursor: pointer;
    white-space: nowrap;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  :global(.btn-cancel:hover) {
    background: var(--ss-surface-1);
    color: var(--ss-text-secondary);
  }

  :global(.btn-cancel:focus-visible) {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }
</style>
