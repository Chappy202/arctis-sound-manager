<script lang="ts">
  /**
   * Checkbox.svelte — Styled checkbox over bits-ui Checkbox.
   *
   * Small square box with accent fill on checked state. Fully controlled:
   * parent owns `checked`; `onCheckedChange` is the callback. No local state.
   */
  import { Checkbox } from "bits-ui";

  interface Props {
    checked: boolean;
    onCheckedChange: (checked: boolean) => void;
    disabled?: boolean;
    ariaLabel?: string;
    id?: string;
  }

  let {
    checked,
    onCheckedChange,
    disabled = false,
    ariaLabel,
    id,
  }: Props = $props();

  // Fully-controlled binding.
  const getChecked = () => checked;
  const setChecked = (v: boolean) => {
    onCheckedChange(v);
  };
</script>

<Checkbox.Root
  class="checkbox-root"
  bind:checked={getChecked, setChecked}
  {disabled}
  {id}
  aria-label={ariaLabel}
>
  <span class="cb-indicator" aria-hidden="true">✓</span>
</Checkbox.Root>

<style>
  /* ===== Checkbox box (Checkbox.Root renders as a <button>) ===== */
  :global(.checkbox-root) {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-xs);
    cursor: pointer;
    padding: 0;
    flex-shrink: 0;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  :global(.checkbox-root[data-state="checked"]) {
    background: var(--ss-accent);
    border-color: var(--ss-accent);
  }

  :global(.checkbox-root[data-disabled]) {
    opacity: 0.4;
    cursor: not-allowed;
  }

  :global(.checkbox-root:focus-visible) {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  /* ===== Check glyph ===== */
  .cb-indicator {
    display: none;
    font-size: 10px;
    line-height: 1;
    color: white;
    font-weight: 700;
    pointer-events: none;
  }

  :global(.checkbox-root[data-state="checked"]) .cb-indicator {
    display: block;
  }
</style>
