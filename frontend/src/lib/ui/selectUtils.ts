/**
 * selectUtils.ts — Pure helpers for the styled Select wrapper.
 *
 * Extracted so they can be unit-tested without a DOM / component harness.
 */

export interface SelectOption {
  value: string;
  label: string;
}

/**
 * Label to show in the Select trigger for the current value.
 * Falls back to the raw value when no option matches (never returns undefined).
 */
export function selectedLabel(options: SelectOption[], value: string): string {
  return options.find((o) => o.value === value)?.label ?? value;
}
