<script lang="ts" module>
  import { cellsKey, rasterise, type Cell } from '@/lib/waiting/braille'
  import type { WaitingAnimation } from '@/lib/waiting/animations'

  /** Frozen frame for reduced motion — the fullest one, sampled once per animation. */
  const STILL_CACHE = new WeakMap<WaitingAnimation, Cell[]>()

  export function stillFrame(a: WaitingAnimation): Cell[] {
    const cached = STILL_CACHE.get(a)
    if (cached) return cached
    let best = rasterise(a.cols, 0, a.field, a.cellAlpha)
    let bestDots = -1
    for (let i = 0; i < 32; i++) {
      const cells = rasterise(a.cols, i / 32, a.field, a.cellAlpha)
      // Popcount of the dot masks — the frame with the most lit dots reads as
      // the most "present" one, which is what a frozen indicator needs to be.
      let dots = 0
      for (const c of cells) {
        let m = c.ch.charCodeAt(0) - 0x2800
        while (m) {
          dots += m & 1
          m >>= 1
        }
      }
      if (dots > bestDots) {
        bestDots = dots
        best = cells
      }
    }
    STILL_CACHE.set(a, best)
    return best
  }
</script>

<script lang="ts">
  import { cn } from '@/lib/cn'
  import { subscribeToClock } from '@/lib/motion'
  import { frac } from '@/lib/waiting/field'
  import { usePrefersReducedMotion } from '@/lib/waiting/reduced-motion.svelte'

  /**
   * One waiting indicator.
   *
   * The animation is driven by writing textContent straight onto the cell spans
   * rather than through component state. A cockpit with a chat turn, three tool
   * rows and a background monitor alive at once would otherwise be five
   * component re-renders per animation frame, for an update whose entire
   * payload is a handful of characters. The frame is also diffed before
   * anything is written: these read best at 12–20fps, so most of the 60 ticks
   * a second are no-ops that never touch the DOM.
   */
  let {
    animation,
    size = 14,
    speed = 1,
    forceReduced,
    class: className,
  }: {
    animation: WaitingAnimation
    /** Font size in px. The glyphs scale with it — there is no second asset. */
    size?: number
    /** Playback multiplier. 1 is the animation's declared period. */
    speed?: number
    /** Force the reduced-motion rendering regardless of the OS setting. */
    forceReduced?: boolean
    class?: string
  } = $props()

  const systemReduced = usePrefersReducedMotion()
  const reduced = $derived(forceReduced ?? systemReduced.current)

  let host = $state<HTMLSpanElement | null>(null)

  const initial = $derived(
    reduced ? stillFrame(animation) : rasterise(animation.cols, 0, animation.field, animation.cellAlpha),
  )

  $effect(() => {
    if (reduced || !host) return
    // Read the reactive inputs up front so the effect re-subscribes when they
    // change; everything below runs off the captured values.
    const a = animation
    const mult = speed
    const cells = Array.from(host.children) as HTMLElement[]
    let lastKey = ''

    return subscribeToClock((elapsed) => {
      const p = frac((elapsed * mult) / a.period)
      const next = rasterise(a.cols, p, a.field, a.cellAlpha)
      const key = cellsKey(next)
      if (key === lastKey) return
      lastKey = key
      for (let i = 0; i < next.length; i++) {
        const el = cells[i]
        const cell = next[i]
        if (!el || !cell) continue
        if (el.textContent !== cell.ch) el.textContent = cell.ch
        const opacity = cell.alpha === 1 ? '' : cell.alpha.toFixed(2)
        if (el.style.opacity !== opacity) el.style.opacity = opacity
      }
    })
  })
</script>

<span
  bind:this={host}
  aria-hidden="true"
  class={cn(
    'inline-flex select-none font-mono leading-none tabular-nums',
    // Mercury's sanctioned reduced-motion swap (spec §9): freeze the frame and
    // fall back to the two-state opacity pulse everything else uses.
    reduced && 'gd-pulse',
    className,
  )}
  style:font-size="{size}px"
>
  <!-- Keyed by animation id so switching animations REMOUNTS the spans. Without
       that, Svelte reconciles against the phase-0 text it thinks is there while
       the effect has been writing frames behind its back, and it can skip an
       update it believes is a no-op. -->
  {#each initial as cell, i (`${animation.id}-${i}`)}
    <!-- Each cell is pinned to one monospace column. IBM Plex Mono may not
         cover U+2800–U+28FF; when the glyph comes from a fallback font with a
         different advance width, an unpinned cell makes the whole indicator
         jitter as dots change. -->
    <span
      class="inline-block text-center"
      style:width="1ch"
      style:opacity={cell.alpha === 1 ? undefined : cell.alpha}
    >
      {cell.ch}
    </span>
  {/each}
</span>
