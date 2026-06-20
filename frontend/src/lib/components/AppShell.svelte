<script lang="ts">
  import { currentPage, type Page } from '../stores/page.js';

  type NavItem = {
    id: Page;
    label: string;
    icon: string;
    disabled?: boolean;
  };

  const navItems: NavItem[] = [
    { id: 'mixer',   label: 'MIXER',   icon: '⊞' },
    { id: 'eq',      label: 'EQ',      icon: '〰' },
    { id: 'device',  label: 'DEVICE',  icon: '◉' },
    { id: 'spatial', label: 'SPATIAL', icon: '◎', disabled: true },
    { id: 'mic',     label: 'MIC',     icon: '⏺', disabled: true },
  ];

  function navigate(page: Page) {
    currentPage.set(page);
  }
</script>

<div class="app-shell">
  <!-- ===== Left nav rail ===== -->
  <nav class="nav-rail" aria-label="Main navigation">
    <div class="nav-logo" aria-label="Arctis Sound Manager">
      <span class="logo-mark" aria-hidden="true">▶</span>
    </div>

    <ul class="nav-list" role="list">
      {#each navItems as item}
        <li>
          <button
            class="nav-item"
            class:active={$currentPage === item.id}
            disabled={item.disabled}
            aria-label={item.label}
            aria-current={$currentPage === item.id ? 'page' : undefined}
            aria-disabled={item.disabled}
            title={item.label}
            onclick={() => !item.disabled && navigate(item.id)}
          >
            <span class="nav-icon" aria-hidden="true">{item.icon}</span>
            <span class="nav-label">{item.label}</span>
          </button>
        </li>
      {/each}
    </ul>
  </nav>

  <!-- ===== Main area ===== -->
  <div class="main-area">
    <!-- Top bar -->
    <header class="topbar">
      <div class="topbar-left">
        <span class="device-name">Arctis Nova Pro Wireless</span>
        <span class="connection-dot connected" aria-label="Connected" title="Connected"></span>
      </div>
      <div class="topbar-right">
        <div class="battery-indicator" aria-label="Battery: —" title="Battery status">
          <span class="battery-icon" aria-hidden="true">▮</span>
          <span class="battery-value">—</span>
        </div>
        <div class="profiles-dropdown" role="button" aria-label="Active profile: Default" aria-haspopup="listbox" tabindex="0">
          <span class="profile-name">Default</span>
          <span class="profile-caret" aria-hidden="true">▾</span>
        </div>
      </div>
    </header>

    <!-- Content area -->
    <main class="content-area" id="main-content">
      <div class="content-inner">
        <slot />
      </div>
    </main>
  </div>
</div>

<style>
  .app-shell {
    display: flex;
    width: 100%;
    height: 100vh;
    background: var(--ss-bg-root);
    overflow: hidden;
  }

  /* ===== Left nav rail ===== */
  .nav-rail {
    display: flex;
    flex-direction: column;
    width: var(--ss-nav-w);
    min-width: var(--ss-nav-w);
    background: var(--ss-bg-root);
    border-right: var(--ss-border-width) solid var(--ss-border);
    padding: var(--ss-space-2) 0;
    z-index: 10;
  }

  .nav-logo {
    display: flex;
    align-items: center;
    justify-content: center;
    height: var(--ss-topbar-h);
    color: var(--ss-accent);
    font-size: 20px;
    border-bottom: var(--ss-border-width) solid var(--ss-border);
    margin-bottom: var(--ss-space-2);
  }

  .logo-mark {
    color: var(--ss-accent);
  }

  .nav-list {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-1);
    padding: 0 var(--ss-space-1);
  }

  .nav-item {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--ss-space-1);
    width: 100%;
    height: calc(var(--ss-nav-w) - var(--ss-space-2));
    border-radius: var(--ss-radius-md);
    color: var(--ss-text-secondary);
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
    position: relative;
    cursor: pointer;
    border: none;
    background: none;
  }

  .nav-item:hover:not(:disabled) {
    color: var(--ss-text-bright);
    background: var(--ss-surface-1);
  }

  .nav-item.active {
    color: var(--ss-accent);
    background: var(--ss-accent-soft);
  }

  /* Left-edge active indicator */
  .nav-item.active::before {
    content: '';
    position: absolute;
    left: calc(-1 * var(--ss-space-1));
    top: 50%;
    transform: translateY(-50%);
    width: 3px;
    height: 60%;
    background: var(--ss-accent);
    border-radius: 0 var(--ss-radius-xs) var(--ss-radius-xs) 0;
  }

  .nav-item:disabled {
    color: var(--ss-text-disabled);
    cursor: not-allowed;
  }

  .nav-icon {
    font-size: 18px;
    line-height: 1;
  }

  .nav-label {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: var(--ss-type-micro-transform);
    line-height: 1;
  }

  /* ===== Main area ===== */
  .main-area {
    display: flex;
    flex-direction: column;
    flex: 1;
    overflow: hidden;
  }

  /* ===== Top bar ===== */
  .topbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: var(--ss-topbar-h);
    min-height: var(--ss-topbar-h);
    padding: 0 var(--ss-page-padding);
    background: var(--ss-bg-root);
    border-bottom: var(--ss-border-width) solid var(--ss-border);
    z-index: 5;
  }

  .topbar-left {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
  }

  .device-name {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-h3-size);
    font-weight: var(--ss-type-h3-weight);
    color: var(--ss-text-primary);
    letter-spacing: var(--ss-type-h3-letter-spacing);
  }

  .connection-dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: var(--ss-radius-pill);
    background: var(--ss-text-disabled);
  }

  .connection-dot.connected {
    background: var(--ss-success);
  }

  /* .disconnected and .connecting states added in Task 4 (device wiring) */
  :global(.connection-dot.disconnected) {
    background: var(--ss-danger);
  }

  :global(.connection-dot.connecting) {
    background: var(--ss-warning);
    animation: pulse 1.5s ease-in-out infinite;
  }

  @keyframes pulse {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
  }

  .topbar-right {
    display: flex;
    align-items: center;
    gap: var(--ss-space-4);
  }

  .battery-indicator {
    display: flex;
    align-items: center;
    gap: var(--ss-space-1);
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-readout-size);
    color: var(--ss-text-secondary);
  }

  .battery-icon {
    color: var(--ss-text-tertiary);
  }

  .battery-value {
    font-variant-numeric: tabular-nums;
  }

  .profiles-dropdown {
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
  }

  .profiles-dropdown:hover {
    background: var(--ss-surface-2);
    border-color: var(--ss-border-strong);
  }

  .profile-name {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-label-size);
    font-weight: var(--ss-type-label-weight);
    color: var(--ss-text-primary);
    white-space: nowrap;
  }

  .profile-caret {
    color: var(--ss-text-tertiary);
    font-size: 12px;
  }

  /* ===== Content area ===== */
  .content-area {
    flex: 1;
    overflow: auto;
    background: var(--ss-bg-base);
  }

  .content-inner {
    max-width: var(--ss-content-max-w);
    margin: 0 auto;
    padding: var(--ss-page-padding);
    height: 100%;
  }
</style>
