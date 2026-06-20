<script lang="ts">
  import { engineState } from "../stores.js";
  import { switchProfile, profileNew } from "../ipc.js";

  /** Controlled open state for the dropdown menu. */
  let open = $state(false);
  let menuEl: HTMLUListElement | undefined = $state();
  let triggerEl: HTMLButtonElement | undefined = $state();

  /** Inline "new profile" name entry state. */
  let creatingNew = $state(false);
  let newProfileName = $state("");
  let newProfileInput: HTMLInputElement | undefined = $state();
  let switching = $state(false);

  function toggleOpen() {
    open = !open;
    if (open && menuEl) {
      // Focus first item after paint.
      requestAnimationFrame(() => {
        const first = menuEl?.querySelector<HTMLElement>("[role='option']");
        first?.focus();
      });
    }
  }

  function closeMenu() {
    open = false;
    creatingNew = false;
    newProfileName = "";
    triggerEl?.focus();
  }

  async function selectProfile(name: string) {
    if (!$engineState || name === $engineState.active_profile || switching) return;
    switching = true;
    closeMenu();
    try {
      const next = await switchProfile(name);
      engineState.set(next);
    } catch (e) {
      console.error("[ProfilesDropdown] switchProfile failed:", e);
    } finally {
      switching = false;
    }
  }

  async function createProfile() {
    const name = newProfileName.trim();
    if (!name || switching) return;
    switching = true;
    creatingNew = false;
    open = false;
    newProfileName = "";
    try {
      const next = await profileNew(name);
      engineState.set(next);
    } catch (e) {
      console.error("[ProfilesDropdown] profileNew failed:", e);
    } finally {
      switching = false;
    }
  }

  function handleTriggerKey(e: KeyboardEvent) {
    if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
      e.preventDefault();
      open = true;
      requestAnimationFrame(() => {
        const first = menuEl?.querySelector<HTMLElement>("[role='option']");
        first?.focus();
      });
    } else if (e.key === "Escape") {
      closeMenu();
    }
  }

  function handleMenuKey(e: KeyboardEvent) {
    if (!menuEl) return;
    const items = Array.from(menuEl.querySelectorAll<HTMLElement>("[role='option'], [data-action]"));
    const idx = items.indexOf(document.activeElement as HTMLElement);

    if (e.key === "ArrowDown") {
      e.preventDefault();
      items[(idx + 1) % items.length]?.focus();
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      items[(idx - 1 + items.length) % items.length]?.focus();
    } else if (e.key === "Escape") {
      closeMenu();
    } else if (e.key === "Enter") {
      (document.activeElement as HTMLElement)?.click();
    }
  }

  function handleOutsideClick(e: MouseEvent) {
    const target = e.target as Node;
    if (!triggerEl?.contains(target) && !menuEl?.contains(target)) {
      closeMenu();
    }
  }

  $effect(() => {
    if (open) {
      document.addEventListener("mousedown", handleOutsideClick);
      return () => document.removeEventListener("mousedown", handleOutsideClick);
    }
  });

  $effect(() => {
    if (creatingNew && newProfileInput) {
      requestAnimationFrame(() => newProfileInput?.focus());
    }
  });

  const displayName = $derived($engineState?.active_profile ?? "—");
  const profiles = $derived($engineState?.profiles ?? []);
</script>

<div class="profiles-wrapper">
  <button
    bind:this={triggerEl}
    class="profiles-trigger"
    class:open
    class:switching
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-label="Active profile: {displayName}. Click to switch profiles."
    onclick={toggleOpen}
    onkeydown={handleTriggerKey}
    disabled={switching}
  >
    <span class="profile-icon" aria-hidden="true">◈</span>
    <span class="profile-name">{displayName}</span>
    {#if switching}
      <span class="profile-spinner" aria-hidden="true"></span>
    {:else}
      <span class="profile-caret" aria-hidden="true" class:rotated={open}>▾</span>
    {/if}
  </button>

  {#if open}
    <div class="profiles-menu-container" role="presentation">
      <ul
        bind:this={menuEl}
        class="profiles-menu"
        role="listbox"
        aria-label="Profiles"
        onkeydown={handleMenuKey}
      >
        {#each profiles as profile}
          <li role="presentation">
            <button
              role="option"
              aria-selected={profile === $engineState?.active_profile}
              class="profile-option"
              class:active={profile === $engineState?.active_profile}
              onclick={() => selectProfile(profile)}
            >
              <span class="option-check" aria-hidden="true">
                {#if profile === $engineState?.active_profile}✓{/if}
              </span>
              <span class="option-name">{profile}</span>
            </button>
          </li>
        {/each}

        {#if profiles.length > 0}
          <li class="menu-divider" role="separator" aria-hidden="true"></li>
        {/if}

        {#if creatingNew}
          <li class="new-profile-row" role="presentation">
            <input
              bind:this={newProfileInput}
              bind:value={newProfileName}
              class="new-profile-input"
              type="text"
              placeholder="Profile name…"
              maxlength={48}
              aria-label="New profile name"
              onkeydown={(e) => {
                if (e.key === "Enter") { e.preventDefault(); createProfile(); }
                else if (e.key === "Escape") { creatingNew = false; newProfileName = ""; }
              }}
            />
            <button
              class="new-profile-confirm btn-secondary"
              onclick={createProfile}
              disabled={!newProfileName.trim()}
              aria-label="Create profile"
            >
              Create
            </button>
          </li>
        {:else}
          <li role="presentation">
            <button
              role="menuitem"
              data-action="new"
              class="profile-action"
              onclick={() => { creatingNew = true; }}
            >
              <span class="action-icon" aria-hidden="true">+</span>
              New profile…
            </button>
          </li>
        {/if}
      </ul>
    </div>
  {/if}
</div>

<style>
  .profiles-wrapper {
    position: relative;
  }

  /* ===== Trigger button ===== */
  .profiles-trigger {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    height: var(--ss-field-h);
    padding: 0 var(--ss-space-3);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-sm);
    cursor: pointer;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
    white-space: nowrap;
    min-width: 140px;
  }

  .profiles-trigger:hover:not(:disabled) {
    background: var(--ss-surface-2);
    border-color: var(--ss-border-strong);
  }

  .profiles-trigger:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  .profiles-trigger.open {
    border-color: var(--ss-accent-border);
    background: var(--ss-surface-2);
  }

  .profiles-trigger:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }

  .profile-icon {
    color: var(--ss-accent);
    font-size: 14px;
    line-height: 1;
    flex-shrink: 0;
  }

  .profile-name {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-label-size);
    font-weight: var(--ss-type-label-weight);
    letter-spacing: var(--ss-type-label-letter-spacing);
    color: var(--ss-text-primary);
    flex: 1;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .profile-caret {
    color: var(--ss-text-tertiary);
    font-size: 11px;
    flex-shrink: 0;
    transition: transform var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .profile-caret.rotated {
    transform: rotate(180deg);
  }

  /* Spinning indicator while switching */
  .profile-spinner {
    display: inline-block;
    width: 12px;
    height: 12px;
    border: 2px solid var(--ss-border-strong);
    border-top-color: var(--ss-accent);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
    flex-shrink: 0;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  /* ===== Dropdown menu ===== */
  .profiles-menu-container {
    position: absolute;
    top: calc(100% + var(--ss-space-1));
    right: 0;
    z-index: 200;
  }

  .profiles-menu {
    list-style: none;
    padding: var(--ss-space-1) 0;
    margin: 0;
    min-width: 180px;
    background: var(--ss-surface-3);
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-sm);
    box-shadow: var(--ss-e2);
  }

  /* ===== Profile option rows ===== */
  .profile-option {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    width: 100%;
    padding: var(--ss-space-2) var(--ss-space-3);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-secondary);
    background: transparent;
    border: none;
    cursor: pointer;
    text-align: left;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .profile-option:hover {
    background: var(--ss-surface-2);
    color: var(--ss-text-primary);
  }

  .profile-option:focus-visible {
    outline: none;
    background: var(--ss-surface-2);
    color: var(--ss-text-primary);
    box-shadow: inset 2px 0 0 var(--ss-accent);
  }

  .profile-option.active {
    background: var(--ss-accent-soft);
    color: var(--ss-text-bright);
  }

  .option-check {
    width: 14px;
    flex-shrink: 0;
    color: var(--ss-accent);
    font-size: 11px;
    text-align: center;
  }

  .option-name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* ===== Divider ===== */
  .menu-divider {
    height: var(--ss-border-width);
    background: var(--ss-border);
    margin: var(--ss-space-1) 0;
  }

  /* ===== Action row (New profile) ===== */
  .profile-action {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    width: 100%;
    padding: var(--ss-space-2) var(--ss-space-3);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    color: var(--ss-text-tertiary);
    background: transparent;
    border: none;
    cursor: pointer;
    text-align: left;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .profile-action:hover,
  .profile-action:focus-visible {
    background: var(--ss-surface-2);
    color: var(--ss-text-secondary);
    outline: none;
  }

  .action-icon {
    color: var(--ss-accent);
    font-size: 14px;
    font-weight: 700;
    line-height: 1;
    width: 14px;
    text-align: center;
  }

  /* ===== Inline new-profile form ===== */
  .new-profile-row {
    display: flex;
    gap: var(--ss-space-2);
    align-items: center;
    padding: var(--ss-space-2) var(--ss-space-3);
  }

  .new-profile-input {
    flex: 1;
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-2);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    min-width: 0;
  }

  .new-profile-input:focus {
    outline: none;
    border-color: var(--ss-accent-border);
  }

  .new-profile-confirm {
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-3);
    background: var(--ss-gradient-primary);
    border: none;
    border-radius: var(--ss-radius-xs);
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

  .new-profile-confirm:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .new-profile-confirm:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .new-profile-confirm:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  @media (prefers-reduced-motion: reduce) {
    .profile-spinner {
      animation: none;
    }
  }
</style>
