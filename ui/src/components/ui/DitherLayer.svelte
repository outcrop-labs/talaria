<script lang="ts" module>
  export interface RectShape {
    x: number
    y: number
    w: number
    h: number
  }

  /**
   * How far a halo should reach around a control THIS size.
   *
   * A fixed reach cannot work across the app: 22px around a 36px button is
   * most of the button again, and the fields of neighbouring controls in a
   * toolbar merge into one cloud — while the same 22px around a 200px card is
   * a hairline. The halo has to be a FRACTION of what it surrounds.
   *
   * Keyed to the short side, because that is what the eye reads as the
   * control's size: a wide, short button is short. Clamped at both ends so a
   * tiny chip still gets something visible and a large surface does not get a
   * weather system.
   */
  export function spreadFor(shortSide: number): number {
    // A BAND AROUND THE BORDER, which is what this is for — not a cloud and
    // not a whisper. 0.32 of the short side put 12px around a 44px button, so
    // neighbouring controls merged into one field. Dropping to 0.16 with a
    // steep falloff overcorrected the other way: 28 dots reaching 2px, which
    // reads as scattered specks rather than as an edge.
    //
    // The reach is short AND the density is high (strength ~0.95 at the
    // boundary), so the dots hug the border and fade out across a few px.
    // Density is what makes it read as a band; distance is what made it read
    // as a cloud.
    return Math.max(4, Math.min(9, Math.round(shortSide * 0.18)))
  }

  /** The bleed a field needs so nothing is clipped: the widest reach in the
   *  sources, plus a margin for the dot grid itself.
   *
   *  Deriving it beats picking a number, because the failure is silent in code
   *  and loud on screen — a spread larger than the bleed does not fade, it gets
   *  cut off square, and the control looks like it is wearing a box. */
  export function bleedFor(spreads: number[]): number {
    return Math.ceil(Math.max(0, ...spreads)) + 6
  }

  /** An element's box in its container's coordinate space — what a RectSource
   *  wants. `pad` grows the rect so the halo starts outside the control's edge. */
  export function rectIn(container: HTMLElement, el: HTMLElement, pad = 0, bleed = 0): RectShape {
    const c = container.getBoundingClientRect()
    const r = el.getBoundingClientRect()
    // MEASURE FROM THE PADDING BOX, not the border box. `getBoundingClientRect`
    // returns the border box, but an absolutely-positioned child — which the
    // canvas is — is offset from its containing block's PADDING box. On a
    // container with a 1px border those differ by exactly that border, and the
    // whole field lands a pixel off the thing it is supposed to ring.
    // `clientLeft`/`clientTop` are the border widths.
    const originX = c.left + container.clientLeft
    const originY = c.top + container.clientTop
    return {
      // `bleed` shifts the origin again: a bled canvas starts `bleed` px above
      // and left of that. Taking both here means a caller cannot apply one and
      // forget the other.
      x: r.left - originX - pad + bleed,
      y: r.top - originY - pad + bleed,
      w: r.width + 2 * pad,
      h: r.height + 2 * pad,
    }
  }
</script>

<script lang="ts">
  import { untrack } from 'svelte'
  import { cn } from '@/lib/cn'
  import { DitherEngine, type DitherEngineOptions, type DitherSource } from '@/lib/dither'
  import { onReducedMotion } from '@/lib/motion'
  import { onThemeChange } from '@/lib/theme'

  /**
   * The Svelte face of the engine: an absolutely-positioned canvas that fills
   * its nearest `relative` parent and renders whatever field it is handed.
   *
   * Drop it FIRST inside the container so the real content paints above it —
   * the field is decoration and must never intercept a click (pointer-events
   * are off) or reach a screen reader (aria-hidden).
   *
   * Theme changes are watched (via `onThemeChange`) because the canvas cannot
   * inherit CSS variables the way DOM paint does — a flip would otherwise leave
   * dark-theme dots on a paper-white surface until the next repaint.
   */
  let {
    sources,
    immediate,
    shimmer,
    bleed = 0,
    class: className,
    ...opts
  }: DitherEngineOptions & {
    sources: DitherSource[]
    /** Skip the tween — for fields driven per-frame by the caller (progress). */
    immediate?: boolean
    /**
     * Grow the canvas this many px BEYOND its container on every side.
     *
     * A halo has to render outside the control it surrounds, and the obvious
     * way — padding on a wrapper — moves the control. This keeps the wrapper
     * shrink-wrapped and lets the canvas spill, so adding a bloom to a button
     * costs no layout at all.
     *
     * Field coordinates are in the GROWN space: the container's own box starts
     * at (bleed, bleed). `rectIn` takes the bleed and does that for you.
     *
     * MUST BE >= the widest `spread` in `sources`, with a little margin. The
     * canvas is the field's only extent: a halo that reaches further than the
     * canvas does is not a soft halo that fades out, it is a halo sliced square
     * at the canvas edge — a hard-edged rectangle floating around the control.
     * `bleedFor()` computes it from the sources so the two cannot drift apart.
     *
     * An ancestor with `overflow-hidden` clips the spill. That degrades to a
     * cropped halo, never to a broken layout.
     */
    bleed?: number
    class?: string
  } = $props()

  let canvas = $state<HTMLCanvasElement | null>(null)
  let engine: DitherEngine | null = null

  $effect(() => {
    const el = canvas
    const parent = el?.parentElement
    if (!el || !parent) return

    // Engine options are construction-time only — callers never change pitch or
    // cover mode live, and rebuilding the engine would defeat the tweens. Read
    // untracked so this effect depends on the canvas alone and never restarts.
    // (Shimmer is the one exception: presence-dependent, so it has a live
    // setter and its own effect below.)
    const e = new DitherEngine(el, untrack(() => ({ ...opts, shimmer })))
    engine = e

    const size = () =>
      e.setSize(parent.clientWidth + 2 * bleed, parent.clientHeight + 2 * bleed, window.devicePixelRatio || 1)
    size()
    const ro = new ResizeObserver(size)
    ro.observe(parent)

    // Theme flips and the reduced-motion preference are PAGE signals, and both
    // subscriptions are shared across every canvas field on the page — a bloom
    // per primary button plus a skeleton per row is a lot of fields, and an
    // observer each is a cost the effect does not need to pay. `onReducedMotion`
    // fires immediately, so the engine starts at the right setting.
    const offTheme = onThemeChange(() => e.refreshColors())
    const offMotion = onReducedMotion((reduced) => e.setReducedMotion(reduced))

    return () => {
      ro.disconnect()
      offTheme()
      offMotion()
      e.destroy()
      engine = null
    }
  })

  // Sources are compared by VALUE, not identity — callers rebuild the array on
  // every state change and only a real field change should reach the engine
  // (each one starts a tween). Serialising is what makes `sources` safe to
  // build inline at the call site, which is how every demo below reads.
  const sourcesKey = $derived(JSON.stringify(sources))

  $effect(() => {
    void sourcesKey
    engine?.setSources(sources, { immediate })
  })

  $effect(() => {
    engine?.setShimmer(shimmer ?? 0)
  })
</script>

<canvas
  bind:this={canvas}
  aria-hidden="true"
  style={bleed ? `inset:${-bleed}px` : undefined}
  class={cn('pointer-events-none absolute', bleed ? '' : 'inset-0 h-full w-full', className)}
></canvas>
