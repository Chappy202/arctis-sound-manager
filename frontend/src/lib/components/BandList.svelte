<script lang="ts">
  import { type Band } from "../eq.js";
  interface Props {
    bands: Band[];
    selectedIndex: number;
    onSelectBand: (index: number) => void;
  }
  let { bands, selectedIndex, onSelectBand }: Props = $props();
  const KIND_SHORT: Record<Band["kind"], string> = { peaking: "PK", lowshelf: "LS", highshelf: "HS" };
  function fmtFreq(f: number) { return f >= 1000 ? `${(f / 1000).toFixed(f >= 10000 ? 1 : 2)}k` : `${Math.round(f)}`; }
</script>

<ul class="band-list" role="listbox" aria-label="EQ bands">
  {#each bands as b, i (i)}
    <li>
      <button class="band-row" class:selected={i === selectedIndex}
        role="option" aria-selected={i === selectedIndex} onclick={() => onSelectBand(i)}>
        <span class="bn">{i + 1}</span>
        <span class="bk">{KIND_SHORT[b.kind]}</span>
        <span class="bf">{fmtFreq(b.freqHz)}Hz</span>
        <span class="bg">{b.gainDb >= 0 ? "+" : ""}{b.gainDb.toFixed(1)}dB</span>
        <span class="bq">Q{b.q.toFixed(1)}</span>
      </button>
    </li>
  {/each}
</ul>

<style>
  .band-list { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(2, 1fr); gap: 2px; }
  .band-row { display: flex; align-items: center; gap: var(--ss-space-2); width: 100%; padding: var(--ss-space-1) var(--ss-space-2); background: var(--ss-surface-2); border: 1px solid transparent; border-radius: var(--ss-radius-xs); cursor: pointer; font-family: var(--ss-font-mono); font-size: var(--ss-type-caption-size); color: var(--ss-text-secondary); }
  .band-row:hover { background: var(--ss-surface-3); }
  .band-row.selected { border-color: var(--ss-accent-border); color: var(--ss-text-primary); }
  .bn { color: var(--ss-accent); width: 16px; }
  .bf { margin-left: auto; }
</style>
