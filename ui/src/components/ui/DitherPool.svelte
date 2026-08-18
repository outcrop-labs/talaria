<script lang="ts">
  import DitherLayer, { bleedFor, rectIn, spreadFor, type RectShape } from './DitherLayer.svelte'
  import type { DitherSource, DitherTone } from '@/lib/dither'

  /**
   * A halo that pools under whichever child is currently active, and glides
   * when the selection moves.
   *
   * Drop it as the FIRST child of a `relative` container; it finds its own
   * parent and measures the active control inside it. That is deliberate —
   * Tabs and Segmented both own their markup and hand out no refs, so a
   * component that demanded an element reference would have to be threaded
   * through both of them. A selector reads their existing ARIA instead, which
   * they already set correctly because it is what tells a screen reader which
   * cell is chosen.
   *
   * `key` exists only to retrigger the measurement: the active element changes
   * identity on selection, and there is nothing to observe on a querySelector.
   * Pass whatever the caller calls "the current value".
   */
  let {
    key,
    selector = '[aria-pressed="true"],[aria-current="true"],[aria-selected="true"]',
    tone = 'accent',
    spread,
    strength = 0.95,
    pad = 0,
    falloff,
  }: {
    key: unknown
    selector?: string
    tone?: DitherTone
    /** Override the reach. Leave unset: it is derived from the measured cell
     *  by `spreadFor`, which keeps it a fraction of the control — a segmented
     *  cell is ~16px tall, and a fixed 26px reach on that is a cloud with a
     *  control somewhere inside it. */
    spread?: number
    strength?: number
    pad?: number
    /** Steeper decay for wide, short controls — a long row needs ~3 to read as
     *  concentric the way a small cell does at the default 2. */
    falloff?: number
  } = $props()

  // Derived, never passed: a bleed smaller than the spread slices the halo
  // square at the canvas edge, which is a box, not a glow. Sized for the
  // ceiling, since the canvas exists before the cell has been measured.
  const bleed = bleedFor([spreadFor(9999)])

  let anchor = $state<HTMLSpanElement | null>(null)
  let rect = $state<RectShape | null>(null)
  let radius = $state(0)

  /** Proportional to the measured control — see `spreadFor`. */
  const reach = $derived(spread ?? (rect ? spreadFor(Math.min(rect.w, rect.h)) : 0))

  // Measured after the DOM has settled on the new selection, and again just
  // past the mark's own 200ms crossfade — the engine tweens between the two
  // readings, so the pool rides the thumb rather than teleporting ahead of it.
  $effect(() => {
    void key
    const container = anchor?.parentElement
    if (!container) return
    const measure = () => {
      const el = container.querySelector<HTMLElement>(selector)
      if (!el) {
        rect = null
        return
      }
      rect = rectIn(container, el, pad, bleed)
      radius = parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0
    }
    measure()
    const settle = window.setTimeout(measure, 230)
    return () => window.clearTimeout(settle)
  })

  // `inner: 0, rim: 0` — the pool is strictly outside the cell. The active cell
  // already draws its own raised fill, and dots behind a mono uppercase label
  // at 10px destroy it.
  const sources = $derived<DitherSource[]>(
    rect ? [{ id: 'pool', kind: 'rect', ...rect, radius, spread: reach, strength, inner: 0, rim: 0, falloff, tone }] : [],
  )
</script>

<span bind:this={anchor} class="contents"></span>
<!-- Finer grain, as on Button — see the note there. -->
<DitherLayer {sources} {bleed} organic={0.12} alphaFloor={0.02} maxAlpha={0.85} />
