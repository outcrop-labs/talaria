<script lang="ts">
  // PORTED from ui/src/components/ui/DitherLayer.svelte, trimmed to the
  // launcher's needs: same engine, same canvas contract, with the app's
  // theme/motion helpers replaced by launcher-local ones (the launcher is
  // always dark; reduced-motion is watched directly). Keep in step with ui/.
  import { untrack } from 'svelte'
  import {
    DitherEngine,
    type DitherEngineOptions,
    type DitherSource,
    type MaskRect,
  } from '../lib/dither'

  let {
    sources,
    immediate,
    shimmer,
    class: className,
    mask,
    ...opts
  }: DitherEngineOptions & {
    sources: DitherSource[]
    immediate?: boolean
    class?: string
    mask?: MaskRect[] | null
  } = $props()

  let canvas = $state<HTMLCanvasElement | null>(null)
  let engine: DitherEngine | null = null

  const onReducedMotion = (fn: (reduced: boolean) => void) => {
    const mq = matchMedia('(prefers-reduced-motion: reduce)')
    const sync = () => fn(mq.matches)
    sync()
    mq.addEventListener('change', sync)
    return () => mq.removeEventListener('change', sync)
  }

  $effect(() => {
    const el = canvas
    const parent = el?.parentElement
    if (!el || !parent) return

    const e = new DitherEngine(el, untrack(() => ({ ...opts, shimmer })))
    engine = e

    const size = () =>
      e.setSize(parent.clientWidth, parent.clientHeight, window.devicePixelRatio || 1)
    size()
    const ro = new ResizeObserver(size)
    ro.observe(parent)

    const offMotion = onReducedMotion((reduced) => e.setReducedMotion(reduced))

    return () => {
      ro.disconnect()
      offMotion()
      e.destroy()
      engine = null
    }
  })

  $effect(() => {
    void mask
    engine?.setMask(mask ?? null)
  })

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
  class={`pointer-events-none absolute inset-0 h-full w-full ${className ?? ''}`}
></canvas>
