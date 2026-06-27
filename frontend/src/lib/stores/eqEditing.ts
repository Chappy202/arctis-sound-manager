/**
 * eqEditing.ts — Tracks whether an EQ edit is in progress, across ALL input
 * modalities (pointer drag, scroll-Q, keyboard, numeric-field focus). The EQ
 * page reads this before applying a background state refresh, so no refresh
 * clobbers an in-progress edit. Supersedes the old pointer-only dragging store.
 */
import { writable } from "svelte/store";

export const eqEditing = writable(false);

let settleTimer: ReturnType<typeof setTimeout> | null = null;

/** Begin a held edit session (e.g. pointerdown, numeric-field focus). */
export function beginEditing(): void {
  if (settleTimer) { clearTimeout(settleTimer); settleTimer = null; }
  eqEditing.set(true);
}

/** End a held edit session after a short settle window (e.g. pointerup, blur). */
export function endEditing(settleMs = 300): void {
  if (settleTimer) clearTimeout(settleTimer);
  settleTimer = setTimeout(() => { eqEditing.set(false); settleTimer = null; }, settleMs);
}

/** One discrete edit (wheel, key, numeric commit): hold then auto-release. */
export function pulseEditing(settleMs = 300): void {
  beginEditing();
  endEditing(settleMs);
}
