<script lang="ts">
  /**
   * Switch.svelte — Styled toggle over bits-ui Switch.
   *
   * Matches SpatialPage's existing .toggle visual. Fully controlled:
   * parent owns `checked`; `onCheckedChange` is the callback. No local state.
   */
  import { Switch } from "bits-ui";

  interface Props {
    checked: boolean;
    onCheckedChange: (checked: boolean) => void;
    disabled?: boolean;
    ariaLabel?: string;
    id?: string;
    /** "md" = 36×20 track (default), "sm" = 32×18 track. */
    size?: "md" | "sm";
  }

  let {
    checked,
    onCheckedChange,
    disabled = false,
    ariaLabel,
    id,
    size = "md",
  }: Props = $props();

  // Fully-controlled binding.
  const getChecked = () => checked;
  const setChecked = (v: boolean) => {
    onCheckedChange(v);
  };
</script>

<Switch.Root
  class="switch-root{size === 'sm' ? ' switch--sm' : ''}"
  bind:checked={getChecked, setChecked}
  {disabled}
  {id}
  aria-label={ariaLabel}
>
  <Switch.Thumb class="switch-thumb" />
</Switch.Root>

<style>
  /* ===== Track (Switch.Root renders as a <button>) ===== */
  :global(.switch-root) {
    display: inline-flex;
    align-items: center;
    width: 36px;
    height: 20px;
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-pill);
    border: 1px solid var(--ss-border);
    position: relative;
    cursor: pointer;
    padding: 0;
    flex-shrink: 0;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  :global(.switch-root[data-state="checked"]) {
    background: var(--ss-accent);
    border-color: var(--ss-accent);
  }

  :global(.switch-root[data-disabled]) {
    opacity: 0.4;
    cursor: not-allowed;
  }

  :global(.switch-root:focus-visible) {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  /* ===== Thumb (Switch.Thumb renders as a <span>) ===== */
  :global(.switch-thumb) {
    position: absolute;
    left: 2px;
    width: 14px;
    height: 14px;
    background: var(--ss-text-tertiary);
    border-radius: var(--ss-radius-pill);
    transition:
      transform var(--ss-dur-fast) var(--ss-ease-standard),
      background var(--ss-dur-fast) var(--ss-ease-standard);
  }

  :global(.switch-root[data-state="checked"] .switch-thumb) {
    transform: translateX(16px);
    background: white;
  }

  /* ===== Size: sm ===== */
  :global(.switch--sm) {
    width: 32px;
    height: 18px;
  }

  :global(.switch--sm .switch-thumb) {
    width: 12px;
    height: 12px;
  }

  :global(.switch--sm[data-state="checked"] .switch-thumb) {
    transform: translateX(14px);
  }
</style>
