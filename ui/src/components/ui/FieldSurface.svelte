<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { onReducedMotion } from '@/lib/motion'
  import { onThemeChange } from '@/lib/theme'
  import { parseColor } from '@/lib/dither'
  import { FieldRenderer, TONE_ORDER, fieldGLSupported, type Field, type FieldGLOptions } from '@/lib/field-gl'
  import { provideFieldSurface, type FieldEntry } from '@/lib/field-registry.svelte'

  /**
   * ONE DITHER CANVAS FOR EVERYTHING BENEATH IT.
   *
   * Wrap a surface — the app shell, a modal — and every control inside it that
   * asks for a field gets one, drawn in a single pass. The canvas sits BEHIND
   * the surface's content, which is where all of these fields belong: a row's
   * fill under its own label, a button's halo under everything.
   *
   * Nesting is supported and meaningful: a modal declares its own surface, so
   * its fields are drawn on its own canvas at its own stacking level rather
   * than by the shell underneath it.
   */
  let {
    class: className,
    children,
    ...opts
  }: FieldGLOptions & { class?: string; children: Snippet } = $props()

  let host = $state<HTMLDivElement | null>(null)
  let canvas = $state<HTMLCanvasElement | null>(null)
  let renderer: FieldRenderer | null = null
  const entries = new Set<FieldEntry>()

  provideFieldSurface({
    register(entry) {
      entries.add(entry)
      renderer?.setFields(collect())
      return () => {
        entries.delete(entry)
        renderer?.setFields(collect())
      }
    },
    invalidate() {
      renderer?.setFields(collect())
    },
  })

  /** Each entry's box, in SURFACE coordinates, with its sources. */
  function collect(): Field[] {
    const h = host
    if (!h) return []
    const base = h.getBoundingClientRect()
    const out: Field[] = []
    for (const e of entries) {
      // A control that has left the document contributes nothing; dropping it
      // here is what keeps a long-lived surface from accumulating dead entries.
      if (!e.el.isConnected) continue
      const r = e.el.getBoundingClientRect()
      const sources = e.sources()
      if (sources.length === 0) continue
      out.push({ x: r.left - base.left, y: r.top - base.top, w: r.width, h: r.height, sources })
    }
    return out
  }

  const readTones = () => {
    const style = getComputedStyle(document.documentElement)
    const VAR = {
      neutral: '--theme-text',
      accent: '--theme-accent',
      success: '--theme-success',
      danger: '--theme-danger',
      surface: '--theme-panel',
    } as const
    return TONE_ORDER.map((t) => parseColor(style.getPropertyValue(VAR[t]) || '#808080'))
  }

  $effect(() => {
    const c = canvas
    const h = host
    if (!c || !h || !fieldGLSupported()) return

    let r: FieldRenderer
    try {
      r = new FieldRenderer(c, opts)
    } catch (err) {
      // A surface with no field is a complete UI, so this must not take the
      // page down — but it must not be silent either. A swallowed shader error
      // is indistinguishable from the feature never having been wired up, and
      // that costs an afternoon of looking in the wrong place.
      console.error('[dither] field surface disabled:', err)
      return
    }
    renderer = r
    r.setTones(readTones())

    const size = () => r.setSize(h.clientWidth, h.clientHeight, window.devicePixelRatio || 1)
    size()
    const ro = new ResizeObserver(() => {
      size()
      r.setFields(collect())
    })
    ro.observe(h)
    // Boxes move for reasons a ResizeObserver cannot see — a row scrolling
    // inside a pane, a rail collapsing. Re-collecting on scroll keeps the
    // fields on their controls.
    const onScroll = () => r.setFields(collect())
    h.addEventListener('scroll', onScroll, { capture: true, passive: true })

    const offTheme = onThemeChange(() => r.setTones(readTones()))
    const offMotion = onReducedMotion((reduced) => r.setOptions({ organic: reduced ? opts.organic ?? 0.45 : opts.organic ?? 0.45 }))

    r.setFields(collect())

    return () => {
      ro.disconnect()
      h.removeEventListener('scroll', onScroll, { capture: true })
      offTheme()
      offMotion()
      r.destroy()
      renderer = null
    }
  })
</script>

<div bind:this={host} class={cn('relative', className)}>
  <canvas
    bind:this={canvas}
    aria-hidden="true"
    class="pointer-events-none absolute inset-0 h-full w-full"
  ></canvas>
  <!-- Content above the field, in its own stacking context so nothing below
       can be lifted over it by an unrelated z-index. -->
  <div class="relative isolate h-full">{@render children()}</div>
</div>
