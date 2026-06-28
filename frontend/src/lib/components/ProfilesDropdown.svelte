<script lang="ts">
  import { engineState } from "../stores.js";
  import { switchProfile, profileRename, profileDelete, profileExport, profileImport } from "../ipc.js";

  /** Controlled open state for the dropdown menu. */
  let open = $state(false);
  let menuEl: HTMLUListElement | undefined = $state();
  let triggerEl: HTMLButtonElement | undefined = $state();

  let switching = $state(false);

  /** Inline rename state. */
  let renamingProfile = $state<string | null>(null);
  let renameValue = $state("");
  let renameInput: HTMLInputElement | undefined = $state();

  /** Import state — paste TOML inline. */
  let importingProfile = $state(false);
  let importToml = $state("");
  let importTextarea: HTMLTextAreaElement | undefined = $state();

  /** Error/info feedback (transient). */
  let feedbackMsg = $state<string | null>(null);
  let feedbackTimer: ReturnType<typeof setTimeout> | null = null;

  function showFeedback(msg: string) {
    feedbackMsg = msg;
    if (feedbackTimer) clearTimeout(feedbackTimer);
    feedbackTimer = setTimeout(() => { feedbackMsg = null; }, 3500);
  }

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
    renamingProfile = null;
    renameValue = "";
    importingProfile = false;
    importToml = "";
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

  function startRename(profile: string) {
    renamingProfile = profile;
    renameValue = profile;
  }

  async function commitRename() {
    const old = renamingProfile;
    const newName = renameValue.trim();
    if (!old || !newName || switching) return;
    if (newName === old) { renamingProfile = null; renameValue = ""; return; }
    switching = true;
    renamingProfile = null;
    renameValue = "";
    try {
      const next = await profileRename(old, newName);
      engineState.set(next);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      showFeedback(`Rename failed: ${msg}`);
      console.error("[ProfilesDropdown] profileRename failed:", e);
    } finally {
      switching = false;
    }
  }

  async function deleteProfile(name: string) {
    if (switching) return;
    const confirmed = window.confirm(`Delete profile "${name}"? This cannot be undone.`);
    if (!confirmed) return;
    switching = true;
    try {
      const next = await profileDelete(name);
      engineState.set(next);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      showFeedback(`Delete failed: ${msg}`);
      console.error("[ProfilesDropdown] profileDelete failed:", e);
    } finally {
      switching = false;
    }
  }

  async function exportProfile(name: string) {
    if (switching) return;
    try {
      const toml = await profileExport(name);
      // Trigger a browser file download of the TOML text.
      const blob = new Blob([toml], { type: "text/plain;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${name}.toml`;
      a.click();
      URL.revokeObjectURL(url);
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      showFeedback(`Export failed: ${msg}`);
      console.error("[ProfilesDropdown] profileExport failed:", e);
    }
  }

  function startImport() {
    importingProfile = true;
    importToml = "";
  }

  async function commitImport() {
    const toml = importToml.trim();
    if (!toml || switching) return;
    switching = true;
    importingProfile = false;
    importToml = "";
    try {
      const next = await profileImport(toml);
      engineState.set(next);
      showFeedback("Profile imported successfully");
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      showFeedback(`Import failed: ${msg}`);
      console.error("[ProfilesDropdown] profileImport failed:", e);
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
    if (renamingProfile && renameInput) {
      requestAnimationFrame(() => {
        renameInput?.focus();
        renameInput?.select();
      });
    }
  });

  $effect(() => {
    if (importingProfile && importTextarea) {
      requestAnimationFrame(() => importTextarea?.focus());
    }
  });

  const displayName = $derived($engineState?.active_profile ?? "—");
  const profiles = $derived($engineState?.profiles ?? []);
  const activeProfile = $derived($engineState?.active_profile ?? "");

  /** A profile can be deleted only if it is not active AND there are 2+ profiles. */
  function canDelete(name: string): boolean {
    return name !== activeProfile && profiles.length > 1;
  }
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

  {#if feedbackMsg}
    <div class="profiles-feedback" role="status" aria-live="polite">{feedbackMsg}</div>
  {/if}

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
            {#if renamingProfile === profile}
              <!-- Inline rename form for this profile -->
              <div class="rename-row" role="presentation">
                <input
                  bind:this={renameInput}
                  bind:value={renameValue}
                  class="rename-input"
                  type="text"
                  placeholder="New name…"
                  maxlength={48}
                  aria-label="Rename profile {profile}"
                  onkeydown={(e) => {
                    if (e.key === "Enter") { e.preventDefault(); commitRename(); }
                    else if (e.key === "Escape") { renamingProfile = null; renameValue = ""; }
                  }}
                />
                <button
                  class="rename-confirm"
                  onclick={commitRename}
                  disabled={!renameValue.trim() || renameValue.trim() === profile}
                  aria-label="Confirm rename"
                >
                  OK
                </button>
                <button
                  class="rename-cancel"
                  onclick={() => { renamingProfile = null; renameValue = ""; }}
                  aria-label="Cancel rename"
                >
                  ✕
                </button>
              </div>
            {:else}
              <div class="profile-row" role="presentation">
                <button
                  role="option"
                  aria-selected={profile === activeProfile}
                  class="profile-option"
                  class:active={profile === activeProfile}
                  onclick={() => selectProfile(profile)}
                >
                  <span class="option-check" aria-hidden="true">
                    {#if profile === activeProfile}✓{/if}
                  </span>
                  <span class="option-name">{profile}</span>
                </button>
                <!-- Per-profile action buttons -->
                <div class="profile-row-actions" aria-label="Actions for {profile}">
                  <button
                    class="profile-icon-btn"
                    title="Rename"
                    aria-label="Rename {profile}"
                    onclick={(e) => { e.stopPropagation(); startRename(profile); }}
                    disabled={switching}
                  >✎</button>
                  <button
                    class="profile-icon-btn"
                    title="Export TOML"
                    aria-label="Export {profile}"
                    onclick={(e) => { e.stopPropagation(); exportProfile(profile); }}
                    disabled={switching}
                  >↓</button>
                  <button
                    class="profile-icon-btn danger"
                    title={canDelete(profile) ? "Delete" : profile === activeProfile ? "Can't delete active profile" : "Can't delete last profile"}
                    aria-label="Delete {profile}"
                    onclick={(e) => { e.stopPropagation(); deleteProfile(profile); }}
                    disabled={switching || !canDelete(profile)}
                    aria-disabled={!canDelete(profile)}
                  >✕</button>
                </div>
              </div>
            {/if}
          </li>
        {/each}

        {#if profiles.length > 0}
          <li class="menu-divider" role="separator" aria-hidden="true"></li>
        {/if}

        {#if importingProfile}
          <li class="import-row" role="presentation">
            <textarea
              bind:this={importTextarea}
              bind:value={importToml}
              class="import-textarea"
              placeholder="Paste profile TOML here…"
              rows={4}
              aria-label="Paste profile TOML to import"
              onkeydown={(e) => {
                if (e.key === "Escape") { importingProfile = false; importToml = ""; }
              }}
            ></textarea>
            <div class="import-actions">
              <button
                class="new-profile-confirm"
                onclick={commitImport}
                disabled={!importToml.trim() || switching}
                aria-label="Import profile"
              >
                Import
              </button>
              <button
                class="profile-action cancel-btn"
                onclick={() => { importingProfile = false; importToml = ""; }}
                aria-label="Cancel import"
              >
                Cancel
              </button>
            </div>
          </li>
        {:else}
          <li role="presentation">
            <button
              role="menuitem"
              data-action="import"
              class="profile-action"
              onclick={() => startImport()}
            >
              <span class="action-icon" aria-hidden="true">↑</span>
              Import from TOML…
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

  /* ===== Confirm button (shared by import form) ===== */
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

  /* ===== Feedback banner ===== */
  .profiles-feedback {
    position: absolute;
    top: calc(100% + var(--ss-space-1));
    right: 0;
    z-index: 201;
    min-width: 180px;
    padding: var(--ss-space-2) var(--ss-space-3);
    background: var(--ss-surface-3);
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-sm);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-secondary);
    box-shadow: var(--ss-e2);
  }

  /* ===== Profile row with inline action buttons ===== */
  .profile-row {
    display: flex;
    align-items: center;
    width: 100%;
  }

  .profile-row .profile-option {
    flex: 1;
  }

  .profile-row-actions {
    display: flex;
    align-items: center;
    gap: 2px;
    padding-right: var(--ss-space-2);
    opacity: 0;
    transition: opacity var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .profile-row:hover .profile-row-actions,
  .profile-row:focus-within .profile-row-actions {
    opacity: 1;
  }

  .profile-icon-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border: none;
    border-radius: var(--ss-radius-xs);
    background: transparent;
    color: var(--ss-text-tertiary);
    font-size: 11px;
    cursor: pointer;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
    flex-shrink: 0;
  }

  .profile-icon-btn:hover:not(:disabled) {
    background: var(--ss-surface-2);
    color: var(--ss-text-secondary);
  }

  .profile-icon-btn.danger:hover:not(:disabled) {
    background: rgba(229, 72, 77, 0.15);
    color: var(--ss-danger, #e5484d);
  }

  .profile-icon-btn:disabled {
    opacity: 0.3;
    cursor: not-allowed;
  }

  .profile-icon-btn:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 1px;
  }

  /* ===== Inline rename form ===== */
  .rename-row {
    display: flex;
    gap: var(--ss-space-1);
    align-items: center;
    padding: var(--ss-space-1) var(--ss-space-2);
  }

  .rename-input {
    flex: 1;
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-2);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-accent-border);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    min-width: 0;
  }

  .rename-input:focus {
    outline: none;
    border-color: var(--ss-accent);
  }

  .rename-confirm {
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-2);
    background: var(--ss-gradient-primary);
    border: none;
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-bright);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-button-size);
    font-weight: var(--ss-type-button-weight);
    cursor: pointer;
    white-space: nowrap;
    transition: opacity var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .rename-confirm:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .rename-confirm:hover:not(:disabled) {
    filter: brightness(1.1);
  }

  .rename-cancel {
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-2);
    background: transparent;
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-tertiary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-body-size);
    cursor: pointer;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .rename-cancel:hover {
    background: var(--ss-surface-2);
    color: var(--ss-text-secondary);
  }

  /* ===== Import TOML form ===== */
  .import-row {
    padding: var(--ss-space-2) var(--ss-space-3);
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-2);
  }

  .import-textarea {
    width: 100%;
    min-width: 0;
    padding: var(--ss-space-2);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border-strong);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-caption-size);
    resize: vertical;
    box-sizing: border-box;
  }

  .import-textarea:focus {
    outline: none;
    border-color: var(--ss-accent-border);
  }

  .import-actions {
    display: flex;
    gap: var(--ss-space-2);
    align-items: center;
  }

  .cancel-btn {
    color: var(--ss-text-tertiary);
    font-size: var(--ss-type-body-size);
  }
</style>
