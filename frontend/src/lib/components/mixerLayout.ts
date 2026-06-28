/**
 * mixerLayout.ts — Pure helpers for MixerPage channel ordering.
 *
 * Extracted as pure functions so they are fully unit-testable without mounting
 * any Svelte component or touching the DOM (no jsdom / happy-dom required).
 */

/** Canonical left-to-right order for the standard channels between Master and Mic. */
export const CANONICAL_CHANNEL_ORDER = ["game", "chat", "media", "aux"] as const;

/**
 * Return channels sorted: canonical standard order first (game, chat, media, aux),
 * then any non-standard / custom channels in their original relative order, appended.
 *
 * - Missing standard channels are simply absent — no placeholders are inserted.
 * - The input array is never mutated.
 */
export function orderChannels<T extends { id: string }>(channels: T[]): T[] {
  const canonicalIndex = new Map<string, number>(
    CANONICAL_CHANNEL_ORDER.map((id, i) => [id, i]),
  );

  const standard: T[] = [];
  const custom: T[] = [];

  for (const ch of channels) {
    if (canonicalIndex.has(ch.id)) {
      standard.push(ch);
    } else {
      custom.push(ch);
    }
  }

  // Sort standard channels into canonical order; custom channels keep their
  // original relative order (stable — insertion order from the for-loop above).
  standard.sort(
    (a, b) =>
      (canonicalIndex.get(a.id) ?? Infinity) -
      (canonicalIndex.get(b.id) ?? Infinity),
  );

  return [...standard, ...custom];
}
