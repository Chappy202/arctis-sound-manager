<script lang="ts">
  /**
   * BandList.svelte — Compact tabular companion to the EQ canvas.
   *
   * Shows each band as a row tinted by its identity color, with numeric
   * freq/gain/Q readouts (--ss-font-mono). Selecting a row highlights
   * the corresponding dot in the canvas (via selectedBandIndex) and vice-versa.
   *
   * For v1, the fields are read-only (display). Editing numeric fields and
   * calling setEqBand is deferred — the canvas drag is the primary edit path.
   */

  import type { Band } from "../eq.js";
  import { setEqBand } from "../ipc.js";

  interface Props {
    channelId: string;
    bands: Band[];
    selectedBandIndex: number;
    onSelectBand?: (index: number) => void;
    onBandChange?: (index: number, band: Band) => void;
  }

  let { channelId, bands, selectedBandIndex, onSelectBand, onBandChange }: Props = $props();

  const BAND_COLORS = [
    "#FF5200",
    "#0091D1",
    "#41A930",
    "#754BD3",
    "#FFBE00",
    "#2A7199",
    "#B24736",
    "#356E74",
    "#6F3969",
    "#50648C",
  ];

  const KIND_LABELS: Record<string, string> = {
    peaking: "PEQ",
    lowshelf: "LS",
    highshelf: "HS",
  };

  function fmtFreq(f: number): string {
    if (f >= 1000) return `${(f / 1000).toFixed(f >= 10000 ? 1 : 2)}k`;
    return `${Math.round(f)}`;
  }

  function fmtGain(g: number): string {
    return `${g >= 0 ? "+" : ""}${g.toFixed(1)}`;
  }

  function fmtQ(q: number): string {
    return q.toFixed(2);
  }

  function bandColor(i: number): string {
    return BAND_COLORS[i % BAND_COLORS.length];
  }

  function select(i: number) {
    onSelectBand?.(i);
  }
</script>

<div class="band-list" role="list" aria-label="EQ bands">
  <!-- Header row -->
  <div class="band-list-head" aria-hidden="true">
    <span class="col-num">#</span>
    <span class="col-kind">TYPE</span>
    <span class="col-freq">FREQ</span>
    <span class="col-gain">GAIN</span>
    <span class="col-q">Q</span>
  </div>

  {#each bands as band, i}
    {@const color = bandColor(i)}
    {@const isSelected = i === selectedBandIndex}
    <button
      class="band-row"
      class:selected={isSelected}
      style="
        --band-color: {color};
        --band-color-soft: {color}22;
        --band-color-border: {color}55;
      "
      aria-label="Band {i + 1}: {KIND_LABELS[band.kind] ?? band.kind}, {fmtFreq(band.freqHz)} Hz, {fmtGain(band.gainDb)} dB, Q {fmtQ(band.q)}"
      onclick={() => select(i)}
    >
      <!-- Band number indicator -->
      <span class="col-num band-num" style="color: {color};">{i + 1}</span>

      <!-- Kind pill -->
      <span class="col-kind kind-pill" style="background: {color}22; color: {color};">
        {KIND_LABELS[band.kind] ?? band.kind}
      </span>

      <!-- Numeric readouts -->
      <span class="col-freq readout">{fmtFreq(band.freqHz)}<span class="unit">Hz</span></span>
      <span class="col-gain readout" class:positive={band.gainDb > 0} class:negative={band.gainDb < 0}>
        {fmtGain(band.gainDb)}<span class="unit">dB</span>
      </span>
      <span class="col-q readout">{fmtQ(band.q)}</span>
    </button>
  {/each}
</div>

<style>
  .band-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    width: 100%;
  }

  .band-list-head {
    display: grid;
    grid-template-columns: 28px 48px 1fr 1fr 1fr;
    gap: var(--ss-space-2);
    padding: 0 var(--ss-space-3) var(--ss-space-1);
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-micro-size);
    font-weight: var(--ss-type-micro-weight);
    letter-spacing: var(--ss-type-micro-letter-spacing);
    text-transform: uppercase;
    color: var(--ss-text-tertiary);
  }

  .band-row {
    display: grid;
    grid-template-columns: 28px 48px 1fr 1fr 1fr;
    gap: var(--ss-space-2);
    align-items: center;
    padding: var(--ss-space-2) var(--ss-space-3);
    border-radius: var(--ss-radius-xs);
    background: transparent;
    border: 1px solid transparent;
    cursor: pointer;
    text-align: left;
    transition:
      background var(--ss-dur-fast) var(--ss-ease-standard),
      border-color var(--ss-dur-fast) var(--ss-ease-standard);
  }

  .band-row:hover {
    background: var(--band-color-soft, rgba(255, 82, 0, 0.08));
    border-color: var(--band-color-border, rgba(255, 82, 0, 0.2));
  }

  .band-row.selected {
    background: var(--band-color-soft, rgba(255, 82, 0, 0.08));
    border-color: var(--band-color-border, rgba(255, 82, 0, 0.3));
  }

  .band-row:focus-visible {
    outline: 2px solid var(--band-color, var(--ss-accent));
    outline-offset: 1px;
  }

  .band-num {
    font-family: var(--ss-font-mono);
    font-size: 11px;
    font-weight: 700;
    text-align: center;
  }

  .kind-pill {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    height: 18px;
    padding: 0 var(--ss-space-1);
    border-radius: var(--ss-radius-xs);
    font-family: var(--ss-font-ui);
    font-size: 9px;
    font-weight: 700;
    letter-spacing: 0.05em;
  }

  .readout {
    font-family: var(--ss-font-mono);
    font-size: var(--ss-type-readout-size);
    font-variant-numeric: tabular-nums;
    color: var(--ss-text-primary);
    white-space: nowrap;
  }

  .readout.positive {
    color: #41A930;
  }

  .readout.negative {
    color: #E5484D;
  }

  .unit {
    font-size: 9px;
    color: var(--ss-text-tertiary);
    margin-left: 1px;
    font-weight: 400;
  }

  .col-num    { text-align: center; }
  .col-kind   { text-align: center; }
  .col-freq   { text-align: right; }
  .col-gain   { text-align: right; }
  .col-q      { text-align: right; }
</style>
