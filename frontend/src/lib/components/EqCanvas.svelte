<script lang="ts">
  /**
   * EqCanvas.svelte — Parametric EQ canvas with draggable band dots.
   *
   * Props:
   *   channelId  — which channel to edit (used in setEqBand IPC calls)
   *   bands      — current band state (Band[])
   *   onBandChange — callback when a band changes (to keep parent in sync)
   *
   * Gesture model:
   *   - Drag a band dot left/right   → adjusts freqHz (log scale)
   *   - Drag a band dot up/down      → adjusts gainDb (linear ±12 dB)
   *   - Scroll wheel on a dot        → adjusts Q (0.3–10, log-ish stepping)
   *   - Arrow keys on focused dot    → nudge gain (up/down) or freq (left/right)
   *   - Shift + arrow keys           → coarse nudge (5× step)
   *
   * IPC: setEqBand is throttled (~50 ms trailing) during drag; a final call
   * fires on pointerup to ensure the last value is flushed.
   *
   * NOTE: Band parameters (freq/q/gain) are initialised from defaults because
   * the daemon's get-state response does not yet return per-band values inside
   * ChannelSnapshot. Changes written via set-eq-band are live but will revert
   * to defaults on page reload until the engine enhancement lands.
   * See task-6-brief.md "Open Questions".
   */

  import { onMount, onDestroy } from "svelte";
  import {
    freqToX,
    gainToY,
    xToFreq,
    yToGain,
    logFreqAxis,
    summedCurveDb,
    clampBand,
    FREQ_MIN,
    FREQ_MAX,
    GAIN_MIN,
    GAIN_MAX,
    type Band,
  } from "../eq.js";
  import { setEqBand } from "../ipc.js";
  import { dragging } from "../stores/eqDragging.js";

  // ---------------------------------------------------------------------------
  // Props
  // ---------------------------------------------------------------------------

  interface Props {
    channelId: string;
    bands: Band[];
    selectedBandIndex?: number;
    onBandChange?: (index: number, band: Band) => void;
    onSelectBand?: (index: number) => void;
  }

  let {
    channelId,
    bands = $bindable([]),
    selectedBandIndex = $bindable(-1),
    onBandChange,
    onSelectBand,
  }: Props = $props();

  // ---------------------------------------------------------------------------
  // Canvas setup
  // ---------------------------------------------------------------------------

  let canvasEl: HTMLCanvasElement | undefined = $state();
  let wrapperEl: HTMLDivElement | undefined = $state();
  let ctx: CanvasRenderingContext2D | null = null;
  let W = 800;
  let H = 300;

  // Band colors from CSS vars (we read them once after mount)
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

  // ---------------------------------------------------------------------------
  // Drag state
  // ---------------------------------------------------------------------------

  let dragIndex = -1;
  let dragStartX = 0;
  let dragStartY = 0;
  let dragStartFreq = 0;
  let dragStartGain = 0;
  let activeIndex = $state(-1); // dot visually "active" (dragging or focused)

  // For throttled IPC calls
  let throttleTimer: ReturnType<typeof setTimeout> | null = null;
  const THROTTLE_MS = 50;

  // Floating readout position
  let readoutX = $state(0);
  let readoutY = $state(0);
  let showReadout = $state(false);

  // reduce-motion preference
  let reducedMotion = false;

  // ---------------------------------------------------------------------------
  // Freq axis samples for the curve (re-computed once; stable)
  // ---------------------------------------------------------------------------

  const CURVE_SAMPLES = 256;
  const freqAxis = logFreqAxis(CURVE_SAMPLES);

  // ---------------------------------------------------------------------------
  // Drawing
  // ---------------------------------------------------------------------------

  function draw() {
    if (!ctx || !canvasEl) return;

    // Clear
    ctx.clearRect(0, 0, W, H);

    // Background (surface-2 colour)
    ctx.fillStyle = "#212121";
    ctx.fillRect(0, 0, W, H);

    drawGrid();
    drawCurve();
    drawBellShadings();
    drawBandDots();
  }

  function drawGrid() {
    if (!ctx) return;

    const FREQ_LABELS = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
    const GAIN_LABELS = [-12, -9, -6, -3, 0, 3, 6, 9, 12];

    ctx.save();

    // Faint gridlines
    ctx.strokeStyle = "rgba(255,255,255,0.06)";
    ctx.lineWidth = 1;
    ctx.setLineDash([]);

    for (const f of FREQ_LABELS) {
      const x = Math.round(freqToX(f, W)) + 0.5;
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, H);
      ctx.stroke();
    }

    for (const g of GAIN_LABELS) {
      const y = Math.round(gainToY(g, H)) + 0.5;
      if (g === 0) {
        // 0 dB line — brighter
        ctx.strokeStyle = "rgba(255,255,255,0.18)";
        ctx.lineWidth = 1.5;
      } else {
        ctx.strokeStyle = "rgba(255,255,255,0.06)";
        ctx.lineWidth = 1;
      }
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(W, y);
      ctx.stroke();
    }

    // Frequency labels (bottom)
    ctx.fillStyle = "rgba(122, 124, 128, 0.9)"; // --ss-text-tertiary
    ctx.font = "10px 'JetBrains Mono', 'SF Mono', monospace";
    ctx.textAlign = "center";
    ctx.textBaseline = "bottom";

    for (const f of FREQ_LABELS) {
      const x = freqToX(f, W);
      const label = f >= 1000 ? `${f / 1000}k` : `${f}`;
      ctx.fillText(label, x, H - 2);
    }

    // Gain labels (left side)
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    for (const g of GAIN_LABELS) {
      if (g === 0) continue; // skip 0 — too busy
      const y = gainToY(g, H);
      ctx.fillStyle = "rgba(122, 124, 128, 0.6)";
      ctx.fillText(`${g > 0 ? "+" : ""}${g}`, 4, y);
    }

    ctx.restore();
  }

  function drawCurve() {
    if (!ctx || bands.length === 0) return;

    const dbs = summedCurveDb(bands, freqAxis);

    ctx.save();

    // Build path
    ctx.beginPath();
    for (let i = 0; i < freqAxis.length; i++) {
      const x = freqToX(freqAxis[i], W);
      const y = gainToY(dbs[i], H);
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }

    // Stroke: --ss-accent (#FF5200), 2.5px
    ctx.strokeStyle = "#FF5200";
    ctx.lineWidth = 2.5;
    ctx.lineJoin = "round";
    ctx.stroke();

    // Fill from curve to 0 dB line
    const zeroY = gainToY(0, H);
    ctx.lineTo(freqToX(FREQ_MAX, W), zeroY);
    ctx.lineTo(freqToX(FREQ_MIN, W), zeroY);
    ctx.closePath();
    ctx.fillStyle = "rgba(255, 82, 0, 0.10)"; // --ss-accent-soft variant
    ctx.fill();

    ctx.restore();
  }

  function drawBellShadings() {
    if (!ctx) return;
    // Draw a faint bell-shaped Q region for each band
    for (let i = 0; i < bands.length; i++) {
      const band = bands[i];
      const color = BAND_COLORS[i % BAND_COLORS.length];
      const cx = freqToX(band.freqHz, W);
      const cy = gainToY(band.gainDb, H);

      // The bell width in pixels ≈ how many Hz the Q spans (±0.5 octave / Q)
      // Rough visual: bandwidth = f0/Q  → half-bw in x-pixels
      const bwHz = band.freqHz / band.q;
      const loHz = Math.max(FREQ_MIN, band.freqHz - bwHz / 2);
      const hiHz = Math.min(FREQ_MAX, band.freqHz + bwHz / 2);
      const x0 = freqToX(loHz, W);
      const x1 = freqToX(hiHz, W);
      const bellW = x1 - x0;

      if (bellW < 2) continue;

      ctx.save();
      // Horizontal gradient for bell shading
      const grad = ctx.createRadialGradient(cx, cy, 0, cx, cy, bellW);
      grad.addColorStop(0, hexAlpha(color, 0.18));
      grad.addColorStop(1, hexAlpha(color, 0));
      ctx.fillStyle = grad;
      ctx.fillRect(x0 - bellW / 2, 0, bellW * 2, H);
      ctx.restore();
    }
  }

  function drawBandDots() {
    if (!ctx) return;

    for (let i = 0; i < bands.length; i++) {
      const band = bands[i];
      const color = BAND_COLORS[i % BAND_COLORS.length];
      const x = freqToX(band.freqHz, W);
      const y = gainToY(band.gainDb, H);

      const isActive = i === activeIndex;
      const isSelected = i === selectedBandIndex;
      const radius = isActive ? 10 : isSelected ? 8 : 6;

      ctx.save();

      if (isActive) {
        // Glow: --ss-glow-accent
        ctx.shadowColor = "rgba(255, 82, 0, 0.5)";
        ctx.shadowBlur = 14;
      }

      // Outer ring for selected
      if (isSelected && !isActive) {
        ctx.beginPath();
        ctx.arc(x, y, radius + 3, 0, Math.PI * 2);
        ctx.strokeStyle = hexAlpha(color, 0.5);
        ctx.lineWidth = 1.5;
        ctx.stroke();
      }

      // Dot fill
      ctx.beginPath();
      ctx.arc(x, y, radius, 0, Math.PI * 2);
      ctx.fillStyle = isActive ? "#FF5200" : color;
      ctx.fill();

      // Dot border
      ctx.strokeStyle = isActive ? "#FFFFFF" : hexAlpha(color, 0.6);
      ctx.lineWidth = isActive ? 2 : 1.5;
      ctx.stroke();

      // Band index label
      ctx.shadowBlur = 0;
      ctx.fillStyle = "#FFFFFF";
      ctx.font = `bold 9px 'JetBrains Mono', monospace`;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(`${i + 1}`, x, y);

      ctx.restore();
    }
  }

  // ---------------------------------------------------------------------------
  // Hit-testing helpers
  // ---------------------------------------------------------------------------

  /**
   * Returns the index of the band dot at (px, py), or -1.
   * Hit radius is 16px (≥32px effective diameter per DESIGN.md a11y requirement).
   */
  function hitTest(px: number, py: number): number {
    const HIT_R = 16;
    for (let i = bands.length - 1; i >= 0; i--) {
      const x = freqToX(bands[i].freqHz, W);
      const y = gainToY(bands[i].gainDb, H);
      const dx = px - x;
      const dy = py - y;
      if (dx * dx + dy * dy <= HIT_R * HIT_R) return i;
    }
    return -1;
  }

  function canvasCoords(e: PointerEvent | WheelEvent): [number, number] {
    if (!canvasEl) return [0, 0];
    const rect = canvasEl.getBoundingClientRect();
    const scaleX = W / rect.width;
    const scaleY = H / rect.height;
    return [(e.clientX - rect.left) * scaleX, (e.clientY - rect.top) * scaleY];
  }

  // ---------------------------------------------------------------------------
  // IPC (throttled)
  // ---------------------------------------------------------------------------

  function flushBand(index: number, band: Band) {
    setEqBand(channelId, index, band.kind, band.freqHz, band.q, band.gainDb).catch(
      (e) => console.warn("[EqCanvas] setEqBand failed:", e),
    );
  }

  function throttledFlush(index: number, band: Band) {
    if (throttleTimer !== null) return;
    throttleTimer = setTimeout(() => {
      throttleTimer = null;
      flushBand(index, band);
    }, THROTTLE_MS);
  }

  // ---------------------------------------------------------------------------
  // Pointer events
  // ---------------------------------------------------------------------------

  function onPointerDown(e: PointerEvent) {
    if (!canvasEl) return;
    const [px, py] = canvasCoords(e);
    const idx = hitTest(px, py);
    if (idx === -1) return;

    e.preventDefault();
    canvasEl.setPointerCapture(e.pointerId);

    dragIndex = idx;
    dragStartX = px;
    dragStartY = py;
    dragStartFreq = bands[idx].freqHz;
    dragStartGain = bands[idx].gainDb;
    dragging.set(true);

    activeIndex = idx;
    if (selectedBandIndex !== idx) {
      selectedBandIndex = idx;
      onSelectBand?.(idx);
    }

    updateReadoutPos(px, py);
    showReadout = true;
    draw();
  }

  function onPointerMove(e: PointerEvent) {
    if (dragIndex === -1 || !canvasEl) return;
    e.preventDefault();

    const [px, py] = canvasCoords(e);
    const dx = px - dragStartX;
    const dy = py - dragStartY;

    // freq: drag left/right using log scale from start position
    const newFreq = clampBand({
      ...bands[dragIndex],
      freqHz: xToFreq(freqToX(dragStartFreq, W) + dx, W),
    }).freqHz;

    // gain: drag up/down, linear
    const newGain = clampBand({
      ...bands[dragIndex],
      gainDb: yToGain(gainToY(dragStartGain, H) + dy, H),
    }).gainDb;

    const updated: Band = { ...bands[dragIndex], freqHz: newFreq, gainDb: newGain };
    bands = bands.map((b, i) => (i === dragIndex ? updated : b));
    onBandChange?.(dragIndex, updated);

    throttledFlush(dragIndex, updated);
    updateReadoutPos(px, py);
    draw();
  }

  function onPointerUp(e: PointerEvent) {
    if (dragIndex === -1) return;
    e.preventDefault();

    // Clear throttle and do a final flush
    if (throttleTimer !== null) {
      clearTimeout(throttleTimer);
      throttleTimer = null;
    }
    flushBand(dragIndex, bands[dragIndex]);

    activeIndex = -1;
    dragIndex = -1;
    dragging.set(false);
    showReadout = false;
    draw();
  }

  function onPointerCancel(e: PointerEvent) {
    dragIndex = -1;
    activeIndex = -1;
    dragging.set(false);
    showReadout = false;
    draw();
  }

  // ---------------------------------------------------------------------------
  // Scroll / Q adjustment
  // ---------------------------------------------------------------------------

  function onWheel(e: WheelEvent) {
    if (!canvasEl) return;
    const [px, py] = canvasCoords(e);
    const idx = hitTest(px, py);
    if (idx === -1) return;

    e.preventDefault();

    // Scroll up → increase Q (narrower bell); scroll down → decrease Q
    const delta = e.deltaY > 0 ? -0.1 : 0.1;
    // Use multiplicative stepping for log-feel: new Q = old Q * factor
    const factor = e.deltaY > 0 ? 0.85 : 1.18;
    const newQ = clampBand({ ...bands[idx], q: bands[idx].q * factor }).q;

    const updated: Band = { ...bands[idx], q: newQ };
    bands = bands.map((b, i) => (i === idx ? updated : b));
    onBandChange?.(idx, updated);
    flushBand(idx, updated);

    activeIndex = idx;
    selectedBandIndex = idx;
    onSelectBand?.(idx);

    updateReadoutPos(px, py);
    showReadout = true;

    // Auto-hide readout after a short pause
    if (readoutHideTimer) clearTimeout(readoutHideTimer);
    readoutHideTimer = setTimeout(() => {
      showReadout = false;
      activeIndex = -1;
      draw();
    }, 1200);

    draw();
  }

  let readoutHideTimer: ReturnType<typeof setTimeout> | null = null;

  // ---------------------------------------------------------------------------
  // Keyboard navigation (focused dot)
  // ---------------------------------------------------------------------------

  function onKeyDown(e: KeyboardEvent) {
    if (selectedBandIndex === -1) return;

    const coarse = e.shiftKey;
    const FREQ_STEP = coarse ? 0.1 : 0.02; // fraction of log range
    const GAIN_STEP = coarse ? 1 : 0.25;

    let handled = true;
    const band = bands[selectedBandIndex];
    let updated = { ...band };

    switch (e.key) {
      case "ArrowLeft": {
        const xCur = freqToX(band.freqHz, W);
        updated.freqHz = clampBand({ ...band, freqHz: xToFreq(xCur - W * FREQ_STEP, W) }).freqHz;
        break;
      }
      case "ArrowRight": {
        const xCur = freqToX(band.freqHz, W);
        updated.freqHz = clampBand({ ...band, freqHz: xToFreq(xCur + W * FREQ_STEP, W) }).freqHz;
        break;
      }
      case "ArrowUp":
        updated.gainDb = clampBand({ ...band, gainDb: band.gainDb + GAIN_STEP }).gainDb;
        break;
      case "ArrowDown":
        updated.gainDb = clampBand({ ...band, gainDb: band.gainDb - GAIN_STEP }).gainDb;
        break;
      default:
        handled = false;
    }

    if (handled) {
      e.preventDefault();
      bands = bands.map((b, i) => (i === selectedBandIndex ? updated : b));
      onBandChange?.(selectedBandIndex, updated);
      flushBand(selectedBandIndex, updated);
      draw();
    }
  }

  // ---------------------------------------------------------------------------
  // Readout chip position
  // ---------------------------------------------------------------------------

  function updateReadoutPos(px: number, py: number) {
    if (!canvasEl) return;
    const rect = canvasEl.getBoundingClientRect();
    const scaleX = rect.width / W;
    const scaleY = rect.height / H;
    // Position in CSS pixels relative to wrapper
    readoutX = px * scaleX;
    readoutY = py * scaleY - 48;
  }

  // ---------------------------------------------------------------------------
  // ResizeObserver
  // ---------------------------------------------------------------------------

  let resizeObserver: ResizeObserver | null = null;

  function resizeCanvas() {
    if (!canvasEl || !wrapperEl) return;
    const rect = wrapperEl.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const cssW = Math.max(rect.width, 200);
    const cssH = Math.max(rect.height, 160);
    W = cssW * dpr;
    H = cssH * dpr;
    canvasEl.width = W;
    canvasEl.height = H;
    canvasEl.style.width = `${cssW}px`;
    canvasEl.style.height = `${cssH}px`;
    ctx = canvasEl.getContext("2d");
    if (ctx) ctx.scale(dpr, dpr);
    // Remap W/H to CSS pixels for all coordinate math
    W = cssW;
    H = cssH;
    draw();
  }

  // ---------------------------------------------------------------------------
  // Lifecycle
  // ---------------------------------------------------------------------------

  onMount(() => {
    reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    if (canvasEl && wrapperEl) {
      resizeObserver = new ResizeObserver(() => resizeCanvas());
      resizeObserver.observe(wrapperEl);
      resizeCanvas();
    }
  });

  onDestroy(() => {
    resizeObserver?.disconnect();
    if (throttleTimer !== null) clearTimeout(throttleTimer);
    if (readoutHideTimer !== null) clearTimeout(readoutHideTimer);
  });

  // Redraw whenever bands or selection changes
  $effect(() => {
    // track reactivity
    void bands;
    void selectedBandIndex;
    draw();
  });

  // ---------------------------------------------------------------------------
  // Helpers
  // ---------------------------------------------------------------------------

  function hexAlpha(hex: string, alpha: number): string {
    // Parse #RRGGBB and return rgba()
    const r = parseInt(hex.slice(1, 3), 16);
    const g = parseInt(hex.slice(3, 5), 16);
    const b = parseInt(hex.slice(5, 7), 16);
    return `rgba(${r},${g},${b},${alpha})`;
  }

  // Format Hz for readout
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

  const activeBand = $derived(
    activeIndex >= 0 && activeIndex < bands.length ? bands[activeIndex] : null,
  );
</script>

<div
  class="eq-canvas-wrapper"
  bind:this={wrapperEl}
  role="presentation"
>
  <canvas
    bind:this={canvasEl}
    class="eq-canvas"
    aria-label="Parametric EQ frequency response. Use Tab to select a band, arrow keys to adjust frequency and gain, scroll wheel to adjust Q."
    tabindex="0"
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerCancel}
    onwheel={onWheel}
    onkeydown={onKeyDown}
  ></canvas>

  <!-- Floating readout chip for the active/dragged band -->
  {#if showReadout && activeBand !== null}
    <div
      class="readout-chip"
      style="left: {readoutX}px; top: {readoutY}px;"
      aria-hidden="true"
    >
      <span class="readout-freq">{fmtFreq(activeBand.freqHz)} Hz</span>
      <span class="readout-sep">·</span>
      <span class="readout-gain">{fmtGain(activeBand.gainDb)} dB</span>
      <span class="readout-sep">·</span>
      <span class="readout-q">Q {fmtQ(activeBand.q)}</span>
    </div>
  {/if}
</div>

<style>
  .eq-canvas-wrapper {
    position: relative;
    width: 100%;
    height: 100%;
    background: var(--ss-surface-2);
    border-radius: var(--ss-radius-md);
    overflow: hidden;
  }

  .eq-canvas {
    display: block;
    width: 100%;
    height: 100%;
    cursor: crosshair;
    touch-action: none;
    /* Focus ring for keyboard users */
  }

  .eq-canvas:focus-visible {
    outline: 2px solid var(--ss-accent);
    outline-offset: 2px;
  }

  /* Floating readout chip (mono, Sonar-style) */
  .readout-chip {
    position: absolute;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    background: rgba(33, 33, 33, 0.92);
    border: 1px solid rgba(255, 82, 0, 0.45);
    border-radius: 4px;
    font-family: var(--ss-font-mono);
    font-size: 11px;
    font-weight: 500;
    color: var(--ss-text-bright);
    white-space: nowrap;
    pointer-events: none;
    transform: translateX(-50%);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.5);
    z-index: 10;
  }

  .readout-sep {
    color: var(--ss-text-tertiary);
    font-size: 9px;
  }

  .readout-freq {
    color: var(--ss-accent);
  }

  .readout-gain {
    color: var(--ss-text-bright);
  }

  .readout-q {
    color: var(--ss-text-secondary);
  }
</style>
