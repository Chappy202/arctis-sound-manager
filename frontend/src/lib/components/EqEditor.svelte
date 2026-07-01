<script lang="ts">
  /**
   * EqEditor.svelte — Reusable framed EQ editor shared by the channel EQ page
   * and the mic EQ. Wraps the shared EqGraph in consistent framing so both
   * present identically (this is what kept the mic graph from being stretched).
   *
   * Two sizing modes:
   *   - default: aspect-locked to the EqGraph design ratio (1000x360);
   *     height follows width. Used for secondary/inline EQs (mic).
   *   - fill:    grows to fill its flex parent (the EQ page hero graph).
   *
   * The band list and gesture hints are opt-in so each page composes only the
   * pieces it needs while keeping the graph framing identical.
   */
  import EqGraph from "./EqGraph.svelte";
  import BandList from "./BandList.svelte";
  import { type Band } from "../eq.js";

  interface Props {
    bands: Band[];
    selectedIndex: number;
    onBandChange: (index: number, band: Band) => void;
    onSelect: (index: number) => void;
    onFlush?: (index: number, band: Band) => void;
    channelId?: string;
    /** Grow to fill the parent (hero) instead of aspect-locking. */
    fill?: boolean;
    /** Show the gesture-hint line under the graph. */
    hints?: boolean;
    /** Render the band list below the graph. */
    bandList?: boolean;
  }

  let {
    bands,
    selectedIndex,
    onBandChange,
    onSelect,
    onFlush,
    channelId = "",
    fill = false,
    hints = false,
    bandList = true,
  }: Props = $props();
</script>

<div class="eq-editor" class:eq-editor--fill={fill}>
  <div class="eq-editor-graph">
    <EqGraph {bands} {selectedIndex} {onBandChange} {onSelect} {onFlush} {channelId} />
  </div>

  {#if hints}
    <div class="gesture-hint" aria-hidden="true">
      <span>Drag = freq / gain</span><span class="hint-sep">·</span>
      <span>Scroll = Q</span><span class="hint-sep">·</span>
      <span>Dbl-click = flatten band</span><span class="hint-sep">·</span>
      <span>Arrows = nudge · Alt+↑↓ = Q</span>
    </div>
  {/if}

  {#if bandList}
    <div class="eq-editor-bands">
      <BandList {bands} {selectedIndex} onSelectBand={onSelect} />
    </div>
  {/if}
</div>

<style>
  .eq-editor {
    display: flex;
    flex-direction: column;
    gap: var(--ss-space-3);
    min-height: 0;
  }

  .eq-editor--fill {
    flex: 1;
  }

  /* Aspect-locked to the EqGraph design ratio (1000x360) so inline EQs keep a
     pleasant shape — height follows width. (The graph's viewBox now tracks its
     rendered size via ResizeObserver, so any ratio renders undistorted; this
     ratio is a layout choice, not a distortion guard.) */
  .eq-editor-graph {
    width: 100%;
    aspect-ratio: 1000 / 360;
    border: var(--ss-border-width) solid var(--ss-border);
    border-radius: var(--ss-radius-md);
    overflow: hidden;
    box-shadow: var(--ss-e1);
  }

  /* Hero mode: grow to fill the parent instead of aspect-locking. */
  .eq-editor--fill .eq-editor-graph {
    flex: 1;
    aspect-ratio: auto;
    min-height: 240px;
  }

  .eq-editor-bands {
    width: 100%;
  }

  .gesture-hint {
    display: flex;
    align-items: center;
    gap: var(--ss-space-2);
    flex-wrap: wrap;
    font-family: var(--ss-font-ui);
    font-size: var(--ss-type-caption-size);
    color: var(--ss-text-tertiary);
    padding: 0 var(--ss-space-1);
  }

  .hint-sep {
    color: var(--ss-border-strong);
  }
</style>
