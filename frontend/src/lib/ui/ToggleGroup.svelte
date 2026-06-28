<script lang="ts">
  /**
   * ToggleGroup.svelte — Segmented single-select over bits-ui ToggleGroup.
   *
   * Replaces the hand-rolled `.segmented` / `.seg-btn` button groups (role="group"
   * + aria-pressed). Fully controlled: parent owns `value`; `onValueChange` fires
   * when the user picks an item. No internal state.
   *
   * bits-ui's single ToggleGroup lets a click on the active item deselect it
   * (value → ""). These segmented controls are exclusive radios — there is always
   * exactly one active option — so we swallow empty values and re-emit nothing,
   * matching the old buttons (clicking the active one was a no-op / re-send).
   */
  import { ToggleGroup } from "bits-ui";

  interface ToggleOption {
    value: string;
    label: string;
  }

  interface Props {
    options: ToggleOption[];
    /** Currently selected value (controlled). */
    value: string;
    /** Called when the user picks a different item. */
    onValueChange: (value: string) => void;
    disabled?: boolean;
    /** Accessible label for the group as a whole. */
    ariaLabel?: string;
  }

  let { options, value, onValueChange, disabled = false, ariaLabel }: Props = $props();

  // Fully-controlled binding: bits-ui reads via getter, routes picks back through
  // onValueChange. Ignore empty (deselect) so the active item can't be unset.
  const getVal = () => value;
  const setVal = (v: string) => {
    if (v) onValueChange(v);
  };
</script>

<ToggleGroup.Root
  type="single"
  class="ui-seg"
  bind:value={getVal, setVal}
  {disabled}
  aria-label={ariaLabel}
>
  {#each options as opt (opt.value)}
    <ToggleGroup.Item class="ui-seg-btn" value={opt.value} aria-label={opt.label}>
      {opt.label}
    </ToggleGroup.Item>
  {/each}
</ToggleGroup.Root>

<style>
  /* ===== Container — mirrors the old .segmented ===== */
  :global(.ui-seg) {
    display: flex;
    background: var(--ss-surface-input);
    border-radius: var(--ss-radius-sm);
    padding: 2px;
    gap: 2px;
  }

  /* ===== Item — mirrors the old .seg-btn ===== */
  :global(.ui-seg-btn) {
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-secondary);
    background: transparent;
    border: none;
    border-radius: calc(var(--ss-radius-sm) - 1px);
    padding: 4px var(--ss-space-3);
    cursor: pointer;
    transition:
      background var(--ss-dur-base) var(--ss-ease-standard),
      color var(--ss-dur-fast) var(--ss-ease-standard);
    min-height: var(--ss-control-h-sm);
    white-space: nowrap;
  }

  :global(.ui-seg-btn:hover:not([data-disabled]):not([data-state="on"])) {
    color: var(--ss-text-primary);
    background: color-mix(in srgb, var(--ss-surface-input-alt) 60%, transparent);
  }

  :global(.ui-seg-btn[data-state="on"]) {
    background: var(--ss-accent);
    color: var(--ss-text-bright);
  }

  :global(.ui-seg-btn[data-disabled]) {
    opacity: 0.45;
    cursor: not-allowed;
  }

  :global(.ui-seg-btn:focus-visible) {
    outline: 2px solid var(--ss-accent);
    outline-offset: 1px;
  }

  :global(.ui-seg[data-disabled]) {
    opacity: 0.45;
  }
</style>
