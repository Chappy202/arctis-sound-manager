<script lang="ts">
  /**
   * Select.svelte — Styled single-select over bits-ui Select.
   *
   * Fully controlled: parent owns `value`; `onValueChange` is the change
   * callback. No internal state. Pure presentation wrapper.
   */
  import { Select } from "bits-ui";
  import { selectedLabel, type SelectOption } from "./selectUtils.js";

  interface Props {
    options: SelectOption[];
    /** Currently selected value (controlled). */
    value: string;
    /** Called when the user picks a new item. */
    onValueChange: (value: string) => void;
    disabled?: boolean;
    ariaLabel?: string;
    id?: string;
  }

  let {
    options,
    value,
    onValueChange,
    disabled = false,
    ariaLabel,
    id,
  }: Props = $props();

  // Fully-controlled binding: bits-ui always reads the prop via getter,
  // and routes user selections back through onValueChange.
  const getVal = () => value;
  const setVal = (v: string) => {
    onValueChange(v);
  };
</script>

<div class="select-wrapper">
  <Select.Root type="single" bind:value={getVal, setVal} {disabled}>
    <Select.Trigger class="select-trigger" {id} aria-label={ariaLabel}>
      <span class="trigger-label">{selectedLabel(options, value)}</span>
      <span class="select-caret" aria-hidden="true">▾</span>
    </Select.Trigger>
    <Select.Portal>
      <Select.Content class="select-content">
        <Select.Viewport class="select-viewport">
          {#each options as opt (opt.value)}
            <Select.Item class="select-item" value={opt.value} label={opt.label}>
              {opt.label}
            </Select.Item>
          {/each}
        </Select.Viewport>
      </Select.Content>
    </Select.Portal>
  </Select.Root>
</div>

<style>
  /* ===== Trigger (control button) ===== */
  .select-wrapper {
    position: relative;
    display: inline-flex;
    width: 100%;
  }

  :global(.select-trigger) {
    display: flex;
    align-items: center;
    width: 100%;
    height: var(--ss-control-h-sm);
    padding: 0 var(--ss-space-5) 0 var(--ss-space-2);
    background: var(--ss-surface-input);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-xs);
    color: var(--ss-text-primary);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    cursor: pointer;
    text-align: left;
    position: relative;
    transition:
      border-color var(--ss-dur-fast) var(--ss-ease-standard),
      background var(--ss-dur-fast) var(--ss-ease-standard);
  }

  :global(.select-trigger:hover) {
    border-color: var(--ss-border-strong);
    background: var(--ss-surface-2);
  }

  :global(.select-trigger:focus-visible),
  :global(.select-trigger[data-state="open"]) {
    outline: none;
    border-color: var(--ss-accent-border);
  }

  :global(.select-trigger[data-disabled]) {
    color: var(--ss-text-disabled);
    cursor: not-allowed;
  }

  .trigger-label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .select-caret {
    position: absolute;
    right: var(--ss-space-2);
    color: var(--ss-text-tertiary);
    font-size: 9px;
    pointer-events: none;
    flex-shrink: 0;
  }

  /* ===== Dropdown content ===== */
  :global(.select-content) {
    background: var(--ss-surface-3);
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-sm);
    box-shadow: var(--ss-e2);
    z-index: 50;
    overflow: hidden;
    min-width: var(--bits-select-anchor-width);
  }

  :global(.select-viewport) {
    padding: var(--ss-space-1);
  }

  :global(.select-item) {
    display: flex;
    align-items: center;
    padding: var(--ss-space-1) var(--ss-space-2);
    border-radius: var(--ss-radius-xs);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-primary);
    cursor: pointer;
    user-select: none;
    outline: none;
    transition: background var(--ss-dur-instant) var(--ss-ease-standard);
  }

  :global(.select-item[data-highlighted]) {
    background: var(--ss-accent-soft);
    color: var(--ss-text-primary);
  }

  :global(.select-item[data-selected]) {
    background: var(--ss-accent-soft);
    color: var(--ss-text-primary);
  }

  :global(.select-item[data-disabled]) {
    color: var(--ss-text-disabled);
    cursor: not-allowed;
  }
</style>
