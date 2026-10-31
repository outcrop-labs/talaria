<script lang="ts">
  // The launcher's ambient field: painted once, static (lib/dither-field.ts
  // says why the live engine is not used here). Repaints only when its box or
  // the device pixel ratio changes, and only once that change has settled: a
  // paint is a full rebuild (tens of milliseconds), so a window drag that
  // repainted on every step kept the main thread busy for the whole drag. In
  // between, the last picture is stretched to the box (the canvas is sized in
  // CSS), which for a field this faint is invisible.
  import type { DitherSource } from '@dither'
  import { paintField, type FieldOptions } from '../lib/dither-field'

  let { sources, ...opts }: FieldOptions & { sources: DitherSource[] } = $props()

  let canvas = $state<HTMLCanvasElement | null>(null)

  $effect(() => {
    const el = canvas
    const parent = el?.parentElement
    if (!el || !parent) return
    const field = sources
    const options = { ...opts }

    // Quiet time after the last size change before a repaint.
    const SETTLE_MS = 200
    let painted = ''
    let frame = 0
    let timer: ReturnType<typeof setTimeout> | undefined
    const paint = () => {
      frame = 0
      const w = parent.clientWidth
      const h = parent.clientHeight
      const dpr = window.devicePixelRatio || 1
      const key = `${w}x${h}@${dpr}`
      if (w === 0 || h === 0 || key === painted) return
      painted = key
      const ms = paintField(el, field, w, h, dpr, options)
      el.dataset.paintMs = ms.toFixed(1)
      el.dataset.paints = String(Number(el.dataset.paints ?? 0) + 1)
    }
    const schedule = () => {
      clearTimeout(timer)
      timer = setTimeout(() => {
        if (!frame) frame = requestAnimationFrame(paint)
      }, SETTLE_MS)
    }
    // The first paint waits one frame, so the launcher's content paints first:
    // the field is faint enough that arriving a frame later is invisible, and
    // its ~80 ms first build (cold JIT) no longer sits before first paint.
    frame = requestAnimationFrame(paint)
    const ro = new ResizeObserver(schedule)
    ro.observe(parent)

    // Moving to a monitor with another scale changes the ratio, not the box.
    let mq: MediaQueryList | null = null
    const watchRatio = () => {
      mq?.removeEventListener('change', onRatio)
      mq = matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`)
      mq.addEventListener('change', onRatio)
    }
    const onRatio = () => {
      watchRatio()
      schedule()
    }
    watchRatio()

    return () => {
      ro.disconnect()
      mq?.removeEventListener('change', onRatio)
      clearTimeout(timer)
      cancelAnimationFrame(frame)
    }
  })
</script>

<canvas
  bind:this={canvas}
  aria-hidden="true"
  data-field="ambient"
  class="pointer-events-none absolute inset-0 h-full w-full"
></canvas>
