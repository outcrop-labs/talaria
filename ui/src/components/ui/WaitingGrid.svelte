<script lang="ts" module>
  import { clamp01 } from '@/lib/waiting/field'
  import type { GridAnimation } from '@/lib/waiting/grid-animations'

  /** Opacity written to the DOM, quantised. */
  const STEPS = 24
  const quantise = (v: number): number => Math.round(clamp01(v) * STEPS) / STEPS

  function rasteriseGrid(a: GridAnimation, p: number): number[] {
    const out: number[] = new Array(a.n * a.n)
    for (let y = 0; y < a.n; y++) {
      for (let x = 0; x < a.n; x++) out[y * a.n + x] = quantise(a.field(x, y, p, a.n, a.n))
    }
    return out
  }

  /** Brightest frame — what a frozen indicator should show under reduced motion. */
  const STILL_CACHE = new WeakMap<GridAnimation, number[]>()
  function stillGrid(a: GridAnimation): number[] {
    const cached = STILL_CACHE.get(a)
    if (cached) return cached
    let best = rasteriseGrid(a, 0)
    let bestSum = -1
    for (let i = 0; i < 32; i++) {
      const g = rasteriseGrid(a, i / 32)
      const sum = g.reduce((s, v) => s + v, 0)
      if (sum > bestSum) {
        bestSum = sum
        best = g
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
   * The dot-grid renderer: a field kept as opacity rather than thresholded.
   *
   * Where braille gets one bit per dot, this gets a full alpha ramp — which is
   * the only reason the gradient and dissolve animations exist at all. The cost
   * is that it is n² elements instead of a few characters, and it no longer
   * lives inside a line of text.
   *
   * Colour still rides on `currentColor`, so these stay theme-aware and inherit
   * from whatever context they are dropped into, exactly like the braille set.
   */
  let {
    animation,
    size = 22,
    speed = 1,
    forceReduced,
    class: className,
  }: {
    animation: GridAnimation
    /** Overall square size in px. Dot diameter and gap derive from it. */
    size?: number
    speed?: number
    forceReduced?: boolean
    class?: string
  } = $props()

  const systemReduced = usePrefersReducedMotion()
  const reduced = $derived(forceReduced ?? systemReduced.current)

  let host = $state<HTMLSpanElement | null>(null)

  const n = $derived(animation.n)
  // 62% of the box is dot, the rest is gap — tight enough to read as one
  // display, open enough that neighbouring dots stay countable.
  const dot = $derived((size * 0.62) / n)
  const gap = $derived(n > 1 ? (size * 0.38) / (n - 1) : 0)

  const initial = $derived(reduced ? stillGrid(animation) : rasteriseGrid(animation, 0))

  $effect(() => {
    if (reduced || !host) return
    const a = animation
    const mult = speed
    const cols = a.n
    const dots = Array.from(host.children) as HTMLElement[]
    // Last value per dot, so a frame only writes the dots that actually moved.
    // At 25 dots and 60fps an unconditional write is 1,500 style mutations a
    // second per indicator; quantised + diffed it is a small fraction of that.
    const last = new Array<number>(dots.length).fill(-1)

    return subscribeToClock((elapsed) => {
      const p = frac((elapsed * mult) / a.period)
      for (let i = 0; i < dots.length; i++) {
        const y = Math.floor(i / cols)
        const v = quantise(a.field(i - y * cols, y, p, cols, cols))
        if (v === last[i]) continue
        last[i] = v
        dots[i]!.style.opacity = String(v)
      }
    })
  })
</script>

<span
  bind:this={host}
  aria-hidden="true"
  class={cn(
    'inline-grid shrink-0 align-middle',
    // Mercury's sanctioned reduced-motion swap (spec §9): freeze the frame and
    // fall back to the two-state opacity pulse everything else uses.
    reduced && 'gd-pulse',
    className,
  )}
  style:grid-template-columns="repeat({n}, {dot}px)"
  style:gap="{gap}px"
  style:width="{size}px"
  style:height="{size}px"
  style:place-content="center"
>
  <!-- Keyed by animation id so switching animations remounts the dots and
       Svelte never reconciles against opacities the effect wrote behind it. -->
  {#each initial as v, i (`${animation.id}-${i}`)}
    <!-- background-color inline rather than Tailwind's `bg-current`.
         `bg-current` is generated correctly in a real build. What it does NOT
         survive is a long-running dev server: Tailwind's scan of this file
         happened before the file existed, so a class name introduced afterwards
         never reaches the stylesheet. The class then applies to an element with
         no rule behind it and every dot paints transparent — the grid renders
         perfectly and shows nothing, with no error. One line of plain CSS is
         immune to that and keeps the currentColor inheritance the accent tint
         depends on. -->
    <span
      class="rounded-full"
      style:width="{dot}px"
      style:height="{dot}px"
      style:opacity={v}
      style:background-color="currentColor"
    ></span>
  {/each}
</span>
