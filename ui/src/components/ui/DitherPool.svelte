<script lang="ts">
  import { useField } from '@/lib/field-registry.svelte'
  import { FIELD_BAND } from '@/lib/field-effects'
  import type { DitherSource, DitherTone } from '@/lib/dither'

  /**
   * A halo that pools under whichever child is currently active, and moves
   * when the selection moves.
   *
   * Drop it as a child of a `relative` container; it finds its own parent and
   * locates the active control inside it. That indirection is deliberate —
   * Tabs and Segmented both own their markup and hand out no refs, so a
   * component demanding an element reference would have to be threaded through
   * both. A selector reads their existing ARIA instead, which they already set
   * correctly because it is what tells a screen reader which cell is chosen.
   *
   * `key` retriggers the lookup: the active element changes identity on
   * selection and there is nothing to observe on a querySelector. Pass whatever
   * the caller calls "the current value".
   *
   * THE MEASURING IS GONE. This used to compute the active cell's rect in the
   * container's coordinate space, against a canvas grown by a bleed, and hold
   * that rect in state. The surface reads the element's box itself at draw
   * time, so all of that — `rectIn`, the bleed, the settle timer that
   * re-measured after the selection animation — collapses into handing over
   * the element. It also fixed a class of bug by construction: a pooled halo
   * can no longer be stale, because there is no stored measurement to go
   * stale.
   */
  let {
    key,
    selector = '[aria-pressed="true"],[aria-current="true"],[aria-selected="true"]',
    tone = 'accent',
    spread,
    strength = 0.95,
    falloff,
  }: {
    key: unknown
    selector?: string
    tone?: DitherTone
    /** Override the reach. Leave unset for the house band. */
    spread?: number
    strength?: number
    /** Steeper decay for wide, short controls — a long row needs ~3 to read as
     *  concentric the way a small cell does at the default 2. */
    falloff?: number
  } = $props()

  let anchor = $state<HTMLSpanElement | null>(null)
  let active = $state<HTMLElement | null>(null)
  let radius = $state(6)

  // Re-found on selection, and again just past the mark's own 200ms crossfade
  // — not to re-measure (the surface does that per draw) but because the
  // active element itself may not exist yet when the key changes.
  $effect(() => {
    void key
    const container = anchor?.parentElement
    if (!container) return
    const find = () => {
      const el = container.querySelector<HTMLElement>(selector)
      active = el
      if (el) radius = parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0
    }
    find()
    const settle = window.setTimeout(find, 230)
    return () => window.clearTimeout(settle)
  })

  // `inner: 0, rim: 0` — the pool is strictly outside the cell. The active cell
  // already draws its own raised fill, and dots behind a mono uppercase label
  // at 10px destroy it.
  useField(
    () => active,
    (): DitherSource[] =>
      active
        ? [
            {
              id: 'pool',
              kind: 'rect',
              x: 0,
              y: 0,
              w: 0,
              h: 0,
              radius,
              spread: spread ?? FIELD_BAND,
              strength,
              inner: 0,
              rim: 0,
              falloff,
              tone,
            },
          ]
        : [],
  )
</script>

<span bind:this={anchor} class="contents"></span>
