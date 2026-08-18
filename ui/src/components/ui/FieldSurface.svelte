<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { onReducedMotion } from '@/lib/motion'
  import { onThemeChange } from '@/lib/theme'
  import { parseColor, type DitherSource } from '@/lib/dither'
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

  /**
   * OPT-IN FILLS, BY DELEGATION.
   *
   * Any element under this surface marked `data-dither-fill` gets a dithered
   * fill when the pointer or keyboard focus reaches it; `data-dither-fill="on"`
   * keeps it up permanently, which is how a selected row differs from a hovered
   * one — the surface has arrived rather than being half there.
   *
   * Delegated rather than per element on purpose. The CSS utility this replaces
   * was a masked background image on a pseudo-element, and every layer of that
   * is work the browser does on the main thread — a tile resampled per element,
   * a mask composited per element, all of it redone on scroll. Doing it here
   * costs ONE listener for the whole surface and one more entry in a uniform
   * array, and the call site changes from a class to an attribute.
   */
  let hovered = $state<HTMLElement | null>(null)
  let pinned = $state<HTMLElement[]>([])

  const FILL: DitherSource[] = [{ id: 'fill', kind: 'uniform', strength: 0.5, tone: 'neutral' }]

  /** Each entry's box, in SURFACE coordinates, with its sources. */
  function collect(): Field[] {
    const h = host
    if (!h) return []
    const base = h.getBoundingClientRect()
    const out: Field[] = []
    // Delegated fills first, so a row's own registered field (a halo, say)
    // composites on top of its fill rather than replacing it.
    for (const el of new Set([...pinned, hovered].filter(Boolean) as HTMLElement[])) {
      if (!el.isConnected) continue
      const r = el.getBoundingClientRect()
      out.push({ x: r.left - base.left, y: r.top - base.top, w: r.width, h: r.height, sources: FILL })
    }
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

    const markOf = (t: EventTarget | null) =>
      t instanceof Element ? (t.closest('[data-dither-fill]') as HTMLElement | null) : null
    const onOver = (e: Event) => {
      const el = markOf(e.target)
      if (el !== hovered) {
        hovered = el
        r.setFields(collect())
      }
    }
    const onOut = (e: Event) => {
      // `pointerout` fires when moving between children of the same marked
      // element; only a move that leaves the marked element itself counts.
      const to = markOf((e as PointerEvent).relatedTarget)
      if (to !== hovered) {
        hovered = to
        r.setFields(collect())
      }
    }
    h.addEventListener('pointerover', onOver)
    h.addEventListener('pointerout', onOut)
    h.addEventListener('focusin', onOver)
    h.addEventListener('focusout', onOut)

    // Pinned fills are a DOM query, refreshed when the attribute changes —
    // selection is expressed in the markup, so the surface reads it there
    // rather than asking every row to tell it.
    const rescan = () => {
      pinned = [...h.querySelectorAll<HTMLElement>('[data-dither-fill="on"]')]
      r.setFields(collect())
    }
    rescan()
    const mo = new MutationObserver(rescan)
    mo.observe(h, { subtree: true, attributes: true, attributeFilter: ['data-dither-fill'], childList: true })

    const offTheme = onThemeChange(() => r.setTones(readTones()))

    // Reduced motion stops the clump morph and leaves the texture exactly as
    // organic — it is the MOVEMENT that is unwelcome, not the grain, and
    // flattening the field would remove information rather than motion.
    const offMotion = onReducedMotion((reduced) => r.setOptions({ drift: reduced ? 0 : (opts.drift ?? 0.08) }))

    // NOTHING VISIBLE, NOTHING DRAWN. The field animates continuously once
    // anything is on it, so a surface scrolled out of view or in a background
    // tab would otherwise burn a frame budget nobody can see. Both are cheap
    // to observe and both are common — a modal's surface sits behind the
    // shell's, and neither stops existing when the tab loses focus.
    const io = new IntersectionObserver(([e]) => r.setPaused(!e?.isIntersecting), { threshold: 0 })
    io.observe(h)
    const onVisibility = () => r.setPaused(document.hidden)
    document.addEventListener('visibilitychange', onVisibility)

    r.setFields(collect())

    return () => {
      ro.disconnect()
      io.disconnect()
      mo.disconnect()
      h.removeEventListener('pointerover', onOver)
      h.removeEventListener('pointerout', onOut)
      h.removeEventListener('focusin', onOver)
      h.removeEventListener('focusout', onOut)
      document.removeEventListener('visibilitychange', onVisibility)
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
