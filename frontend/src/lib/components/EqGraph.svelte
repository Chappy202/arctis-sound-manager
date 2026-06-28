<script lang="ts">
  /**
   * EqGraph.svelte — Controlled SVG parametric-EQ editor.
   * Renders a log-frequency grid, the summed response curve, and one focusable
   * <circle role="slider"> handle per band. Holds NO band state — the parent
   * owns the single source of truth and receives changes via onBandChange.
   */
  import {
    freqToX, gainToY, xToFreq, yToGain, logFreqAxis, summedCurveDb, clampBand,
    FREQ_MIN, FREQ_MAX, GAIN_MIN, GAIN_MAX, type Band,
  } from "../eq.js";
  import { setEqBand } from "../ipc.js";
  import { beginEditing, endEditing, pulseEditing } from "../stores/eqEditing.js";

  interface Props {
    bands: Band[];
    selectedIndex: number;
    onBandChange: (index: number, band: Band) => void;
    onSelect: (index: number) => void;
    onFlush?: (index: number, band: Band) => void;
    channelId?: string;
  }
  let { bands, selectedIndex, onBandChange, onSelect, onFlush, channelId = "" }: Props = $props();

  // Internal viewBox coordinate space (CSS scales the SVG to fit).
  const VW = 1000;
  const VH = 360;

  const BAND_COLORS = [
    "#FF5200", "#0091D1", "#41A930", "#754BD3", "#FFBE00",
    "#2A7199", "#B24736", "#356E74", "#6F3969", "#50648C",
  ];
  const FREQ_LABELS = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
  const GAIN_LABELS = [-12, -6, 0, 6, 12];
  // 160 samples is visually indistinguishable from 240 at this width while
  // cutting per-recompute biquad math by a third (matters during drags).
  const CURVE_SAMPLES = 160;
  const freqAxis = logFreqAxis(CURVE_SAMPLES);

  // Throttled flush during drag.
  let throttleTimer: ReturnType<typeof setTimeout> | null = null;
  const THROTTLE_MS = 50;

  function flush(index: number, band: Band) {
    if (onFlush) { onFlush(index, band); return; }
    setEqBand(channelId, index, band.kind, band.freqHz, band.q, band.gainDb)
      .catch((e) => console.warn("[EqGraph] setEqBand failed:", e));
  }
  function throttledFlush(index: number) {
    if (throttleTimer !== null) return;
    throttleTimer = setTimeout(() => { throttleTimer = null; flush(index, bands[index]); }, THROTTLE_MS);
  }

  // ── Derived geometry ──────────────────────────────────────────────────────
  const curvePath = $derived.by(() => {
    const dbs = summedCurveDb(bands, freqAxis);
    let d = "";
    for (let i = 0; i < freqAxis.length; i++) {
      const x = freqToX(freqAxis[i], VW);
      const y = gainToY(dbs[i], VH);
      d += (i === 0 ? "M" : "L") + x.toFixed(2) + " " + y.toFixed(2) + " ";
    }
    return d.trim();
  });
  const fillPath = $derived(
    `${curvePath} L ${freqToX(FREQ_MAX, VW).toFixed(2)} ${gainToY(0, VH).toFixed(2)} ` +
    `L ${freqToX(FREQ_MIN, VW).toFixed(2)} ${gainToY(0, VH).toFixed(2)} Z`
  );

  function handleX(b: Band) { return freqToX(b.freqHz, VW); }
  function handleY(b: Band) { return gainToY(b.gainDb, VH); }

  // ── Pointer drag (per-handle) ─────────────────────────────────────────────
  let dragIndex = -1;
  let svgEl: SVGSVGElement | undefined = $state();

  function toViewBox(e: PointerEvent): [number, number] {
    if (!svgEl) return [0, 0];
    const r = svgEl.getBoundingClientRect();
    return [((e.clientX - r.left) / r.width) * VW, ((e.clientY - r.top) / r.height) * VH];
  }

  function onHandleDown(e: PointerEvent, i: number) {
    e.preventDefault();
    (e.currentTarget as Element).setPointerCapture(e.pointerId);
    dragIndex = i;
    beginEditing();
    if (selectedIndex !== i) onSelect(i);
  }
  function onHandleMove(e: PointerEvent, i: number) {
    if (dragIndex !== i) return;
    e.preventDefault();
    const [vx, vy] = toViewBox(e);
    const next = clampBand({ ...bands[i], freqHz: xToFreq(vx, VW), gainDb: yToGain(vy, VH) });
    onBandChange(i, next);
    throttledFlush(i);
  }
  function onHandleUp(e: PointerEvent, i: number) {
    if (dragIndex !== i) return;
    e.preventDefault();
    if (throttleTimer !== null) { clearTimeout(throttleTimer); throttleTimer = null; }
    flush(i, bands[i]);
    dragIndex = -1;
    endEditing();
  }
  function onHandleCancel(_e: PointerEvent, i: number) {
    if (dragIndex !== i) return;
    if (throttleTimer !== null) { clearTimeout(throttleTimer); throttleTimer = null; }
    dragIndex = -1;
    endEditing();
  }

  // ── Scroll = Q ────────────────────────────────────────────────────────────
  function onHandleWheel(e: WheelEvent, i: number) {
    e.preventDefault();
    const factor = e.deltaY > 0 ? 0.85 : 1.18;
    const next = clampBand({ ...bands[i], q: bands[i].q * factor });
    onSelect(i);
    onBandChange(i, next);
    flush(i, next);
    pulseEditing();
  }

  // ── Keyboard (APG slider) ─────────────────────────────────────────────────
  function onHandleKey(e: KeyboardEvent, i: number) {
    const b = bands[i];
    const coarse = e.shiftKey;
    const gainStep = coarse ? 1 : 0.25;
    const qStep = coarse ? 0.5 : 0.1;
    let next: Band | null = null;
    switch (e.key) {
      case "ArrowUp":
        next = e.altKey ? { ...b, q: b.q + qStep } : { ...b, gainDb: b.gainDb + gainStep }; break;
      case "ArrowDown":
        next = e.altKey ? { ...b, q: b.q - qStep } : { ...b, gainDb: b.gainDb - gainStep }; break;
      case "ArrowLeft": {
        const x = freqToX(b.freqHz, VW); next = { ...b, freqHz: xToFreq(x - VW * (coarse ? 0.05 : 0.02), VW) }; break;
      }
      case "ArrowRight": {
        const x = freqToX(b.freqHz, VW); next = { ...b, freqHz: xToFreq(x + VW * (coarse ? 0.05 : 0.02), VW) }; break;
      }
      case "Home": next = { ...b, gainDb: GAIN_MIN }; break;
      case "End": next = { ...b, gainDb: GAIN_MAX }; break;
      default: return;
    }
    e.preventDefault();
    const clamped = clampBand(next);
    onSelect(i);
    onBandChange(i, clamped);
    flush(i, clamped);
    pulseEditing();
  }

  function onHandleDblClick(i: number) {
    const next = clampBand({ ...bands[i], gainDb: 0 });
    onBandChange(i, next);
    flush(i, next);
    pulseEditing();
  }

  function fmt(b: Band) {
    const f = b.freqHz >= 1000 ? `${(b.freqHz / 1000).toFixed(1)}k` : `${Math.round(b.freqHz)}`;
    return `${b.kind}, ${f} Hz, ${b.gainDb >= 0 ? "+" : ""}${b.gainDb.toFixed(1)} dB, Q ${b.q.toFixed(2)}`;
  }
</script>

<div class="eq-graph-wrap">
  <svg bind:this={svgEl} viewBox="0 0 {VW} {VH}" preserveAspectRatio="none"
       class="eq-graph" role="group" aria-label="Parametric EQ frequency response editor">
    <!-- grid -->
    {#each FREQ_LABELS as f}
      <line x1={freqToX(f, VW)} y1="0" x2={freqToX(f, VW)} y2={VH} class="grid" />
      <text x={freqToX(f, VW)} y={VH - 4} class="axis-x">{f >= 1000 ? `${f / 1000}k` : f}</text>
    {/each}
    {#each GAIN_LABELS as g}
      <line x1="0" y1={gainToY(g, VH)} x2={VW} y2={gainToY(g, VH)} class:zero={g === 0} class="grid" />
      {#if g !== 0}<text x="6" y={gainToY(g, VH) - 3} class="axis-y">{g > 0 ? `+${g}` : g}</text>{/if}
    {/each}

    <!-- curve -->
    <path d={fillPath} class="curve-fill" />
    <path d={curvePath} class="curve-line" />

    <!-- handles -->
    {#each bands as b, i (i)}
      <g class="handle" class:selected={i === selectedIndex}>
        <!-- larger transparent hit/focus target -->
        <circle cx={handleX(b)} cy={handleY(b)} r="18" class="hit"
          role="slider" tabindex="0"
          aria-label={`Band ${i + 1} (${b.kind})`}
          aria-valuemin={GAIN_MIN} aria-valuemax={GAIN_MAX} aria-valuenow={b.gainDb}
          aria-valuetext={`Band ${i + 1}, ${fmt(b)}`}
          onpointerdown={(e) => onHandleDown(e, i)}
          onpointermove={(e) => onHandleMove(e, i)}
          onpointerup={(e) => onHandleUp(e, i)}
          onpointercancel={(e) => onHandleCancel(e, i)}
          onwheel={(e) => onHandleWheel(e, i)}
          onkeydown={(e) => onHandleKey(e, i)}
          ondblclick={() => onHandleDblClick(i)}
          onfocus={() => onSelect(i)}
        />
        <circle cx={handleX(b)} cy={handleY(b)} r={i === selectedIndex ? 9 : 6}
          fill={BAND_COLORS[i % BAND_COLORS.length]} class="dot" />
        <text x={handleX(b)} y={handleY(b) + 3} class="dot-num">{i + 1}</text>
      </g>
    {/each}
  </svg>
</div>

<style>
  .eq-graph-wrap { width: 100%; height: 100%; background: var(--ss-surface-2); border-radius: var(--ss-radius-md); overflow: hidden; }
  .eq-graph { display: block; width: 100%; height: 100%; touch-action: none; }
  .grid { stroke: rgba(255,255,255,0.06); stroke-width: 1; }
  .grid.zero { stroke: rgba(255,255,255,0.18); stroke-width: 1.5; }
  .axis-x { fill: rgba(122,124,128,0.9); font: 10px var(--ss-font-mono, monospace); text-anchor: middle; }
  .axis-y { fill: rgba(122,124,128,0.6); font: 10px var(--ss-font-mono, monospace); }
  .curve-line { fill: none; stroke: var(--ss-accent, #FF5200); stroke-width: 2.5; stroke-linejoin: round; vector-effect: non-scaling-stroke; }
  .curve-fill { fill: rgba(255,82,0,0.10); stroke: none; }
  .hit { fill: transparent; cursor: grab; }
  .hit:active { cursor: grabbing; }
  .hit:focus-visible { outline: none; }
  .handle:focus-within .dot, .handle.selected .dot { stroke: #fff; stroke-width: 2; }
  /* The visible dot is painted on top of the transparent .hit target; without
     this it would swallow pointer events over its centre, leaving only the ring
     around it grabbable. pointer-events:none lets the full r=18 hit target win. */
  .dot { stroke: rgba(255,255,255,0.6); stroke-width: 1.5; pointer-events: none; }
  .dot-num { fill: #fff; font: bold 9px var(--ss-font-mono, monospace); text-anchor: middle; pointer-events: none; }
</style>
