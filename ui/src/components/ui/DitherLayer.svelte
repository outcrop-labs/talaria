<script lang="ts" module>
  export interface RectShape {
    x: number
    y: number
    w: number
    h: number
  }

  /** An element's box in its container's coordinate space — what a RectSource
   *  wants. `pad` grows the rect so the halo starts outside the control's edge. */
  export function rectIn(container: HTMLElement, el: HTMLElement, pad = 0): RectShape {
    const c = container.getBoundingClientRect()
    const r = el.getBoundingClientRect()
    return { x: r.left - c.left - pad, y: r.top - c.top - pad, w: r.width + 2 * pad, h: r.height + 2 * pad }
  }
</script>

<script lang="ts">
  import { untrack } from 'svelte'
  import { cn } from '@/lib/cn'
  import { DitherEngine, type DitherEngineOptions, type DitherSource } from '@/lib/dither'

  /**
   * The Svelte face of the engine: an absolutely-positioned canvas that fills
   * its nearest `relative` parent and renders whatever field it is handed.
   *
   * Drop it FIRST inside the container so the real content paints above it —
   * the field is decoration and must never intercept a click (pointer-events
   * are off) or reach a screen reader (aria-hidden).
   *
   * Theme changes are observed on <html data-theme> because the canvas cannot
   * inherit CSS variables the way DOM paint does — a flip would otherwise leave
   * dark-theme dots on a paper-white surface until the next repaint.
   */
  let {
    sources,
    immediate,
    shimmer,
    class: className,
    ...opts
  }: DitherEngineOptions & {
    sources: DitherSource[]
    /** Skip the tween — for fields driven per-frame by the caller (progress). */
    immediate?: boolean
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

    const size = () => e.setSize(parent.clientWidth, parent.clientHeight, window.devicePixelRatio || 1)
    size()
    const ro = new ResizeObserver(size)
    ro.observe(parent)

    const mo = new MutationObserver(() => e.refreshColors())
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'class'] })

    const mq = window.matchMedia('(prefers-reduced-motion: reduce)')
    e.setReducedMotion(mq.matches)
    const onMq = () => e.setReducedMotion(mq.matches)
    mq.addEventListener('change', onMq)

    return () => {
      ro.disconnect()
      mo.disconnect()
      mq.removeEventListener('change', onMq)
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
  class={cn('pointer-events-none absolute inset-0 h-full w-full', className)}
></canvas>
