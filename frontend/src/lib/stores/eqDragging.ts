/**
 * eqDragging.ts — Shared store tracking whether an EQ canvas pointer drag is in progress.
 *
 * EqCanvas.svelte sets this true on pointerdown and false on pointerup/pointercancel.
 * EqPage.svelte reads it (via get()) before refreshing bands from a state-changed
 * event, to avoid clobbering values the user is actively editing on the canvas.
 */
import { writable } from "svelte/store";

export const dragging = writable(false);
