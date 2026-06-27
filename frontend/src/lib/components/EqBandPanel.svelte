<script lang="ts">
  import { clampBand, FREQ_MIN, FREQ_MAX, GAIN_MIN, GAIN_MAX, Q_MIN, Q_MAX, type Band } from "../eq.js";
  import { beginEditing, endEditing, pulseEditing } from "../stores/eqEditing.js";

  interface Props {
    band: Band | null;
    index: number;
    onBandChange: (index: number, band: Band) => void;
    onFlush: (index: number, band: Band) => void;
  }
  let { band, index, onBandChange, onFlush }: Props = $props();

  const KINDS: Band["kind"][] = ["peaking", "lowshelf", "highshelf"];
  const KIND_LABELS: Record<Band["kind"], string> = {
    peaking: "Peaking", lowshelf: "Low shelf", highshelf: "High shelf",
  };

  function commit(patch: Partial<Band>) {
    if (!band) return;
    const next = clampBand({ ...band, ...patch });
    pulseEditing();
    onBandChange(index, next);
    onFlush(index, next);
  }
  function onNum(field: "freqHz" | "gainDb" | "q", e: Event) {
    const v = Number((e.target as HTMLInputElement).value);
    if (Number.isNaN(v)) return; // reject; field reverts on blur via reactive value
    commit({ [field]: v } as Partial<Band>);
  }
  function resetBand() {
    if (!band) return;
    commit({ kind: "peaking", q: 1, gainDb: 0 });
  }
</script>

<div class="band-panel">
  {#if band}
    <div class="panel-head">SELECTED BAND <span class="b-num">{index + 1}</span></div>
    <label class="field">
      <span>Type</span>
      <select value={band.kind} onchange={(e) => commit({ kind: (e.target as HTMLSelectElement).value as Band["kind"] })}>
        {#each KINDS as k}<option value={k}>{KIND_LABELS[k]}</option>{/each}
      </select>
    </label>
    <label class="field">
      <span>Freq (Hz)</span>
      <input type="number" min={FREQ_MIN} max={FREQ_MAX} step="1" value={Math.round(band.freqHz)}
        onfocus={beginEditing} onblur={() => endEditing()} oninput={(e) => onNum("freqHz", e)} />
    </label>
    <label class="field">
      <span>Gain (dB)</span>
      <input type="number" min={GAIN_MIN} max={GAIN_MAX} step="0.5" value={band.gainDb}
        onfocus={beginEditing} onblur={() => endEditing()} oninput={(e) => onNum("gainDb", e)} />
    </label>
    <label class="field">
      <span>Q</span>
      <input type="number" min={Q_MIN} max={Q_MAX} step="0.1" value={band.q}
        onfocus={beginEditing} onblur={() => endEditing()} oninput={(e) => onNum("q", e)} />
    </label>
    <button class="reset-btn" onclick={resetBand}>Reset band</button>
  {:else}
    <p class="empty">Select a band to edit its values.</p>
  {/if}
</div>

<style>
  .band-panel { display: flex; flex-direction: column; gap: var(--ss-space-2); }
  .panel-head { font-family: var(--ss-font-display); text-transform: uppercase; font-size: var(--ss-type-h2-size); color: var(--ss-text-primary); }
  .b-num { color: var(--ss-accent); }
  .field { display: grid; grid-template-columns: 84px 1fr; align-items: center; gap: var(--ss-space-2); font-size: var(--ss-type-caption-size); color: var(--ss-text-secondary); }
  .field select, .field input { height: var(--ss-control-h-sm); background: var(--ss-surface-input); border: 1px solid var(--ss-border-strong); border-radius: var(--ss-radius-xs); color: var(--ss-text-primary); padding: 0 var(--ss-space-2); font-family: var(--ss-font-mono); }
  .field input:focus, .field select:focus { outline: none; border-color: var(--ss-accent-border); }
  .reset-btn { align-self: flex-start; height: 24px; padding: 0 var(--ss-space-3); background: transparent; border: 1px solid var(--ss-border-strong); border-radius: var(--ss-radius-xs); color: var(--ss-text-tertiary); cursor: pointer; }
  .reset-btn:hover { color: var(--ss-accent); border-color: var(--ss-accent-border); background: var(--ss-accent-soft); }
  .empty { color: var(--ss-text-tertiary); font-style: italic; font-size: var(--ss-type-caption-size); }
</style>
