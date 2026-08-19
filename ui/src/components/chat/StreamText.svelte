<script lang="ts">
  import Markdown from '@/components/ui/Markdown.svelte'
  import DitherLayer, { type RectShape } from '@/components/ui/DitherLayer.svelte'
  import { cn } from '@/lib/cn'
  import { CHURN_MS, revealedText, settling, trackArrivals, unsettledCount } from '@/lib/stream-reveal'
  import { onReducedMotion, subscribeToClock } from '@/lib/motion'
  import type { DitherSource } from '@/lib/dither'

  /**
   * Streamed assistant text: characters land unresolved and decode, and the
   * field they condense out of follows ONLY the run that is still in motion.
   *
   * THE BALANCE. The settled prefix is plain text — full opacity, zero field,
   * always readable. The unresolved tail is code being written: a run of
   * letters and digits that re-roll on staggered per-column clocks (see
   * standIn — rain, not a strobe), wrapped in `.stream-scramble*` so the
   * body of the tail sits at 0.12 opacity while the newest few characters
   * lead at 0.38, the way the front of a matrix column does. The dither is
   * masked to exactly those spans (one rect per line fragment, re-measured
   * after every repaint), so the field blooms around the scramble and
   * nowhere else — and when the turn settles, the field tweens out on the
   * engine's standard budget and the glyphs are already gone.
   *
   * THE SCRAMBLE IS lib/stream-reveal (pure, tested); this component owns the
   * two things a pure function cannot — the clock and the field. The clock is
   * the page's shared one, bucketed to the churn rate: a character re-rolling
   * at 60Hz is a strobe (see CHURN_MS), and nothing in the settled prefix
   * changes between churn ticks, so a faster clock would repaint the same
   * message.
   *
   * THE FADE AND THE MASK ARE POST-RENDER. Markdown flattens text — the
   * pipeline drops raw HTML — so a stand-in glyph cannot be styled or masked
   * from the source. The unresolved region is a suffix (arrivals are
   * monotonic), which puts it in the last text node of the rendered markdown;
   * a Range wraps just that slice, and the mask effect then measures the
   * wrapped spans' line fragments into canvas-local rects. If the tail
   * straddles a node boundary (a break, a fence inside it), this tick treats
   * only what the last node holds and re-wraps next tick — the effect is
   * quiet enough to survive that. Every repaint replaces the markdown's DOM,
   * which also clears the old wraps; nothing leaks.
   *
   * HISTORY NEVER ANIMATES. `prev` starts at the mounted content, so a message
   * that was already there when it mounted has no arrivals; and a message that
   * is rewritten wholesale (a resume replay, a thread swap) fails the prefix
   * test in trackArrivals and gets none.
   *
   * Reduced motion: the scramble is dropped (the text simply streams in) and
   * the field sources go with it — the field's whole subject is the
   * statistical state, and under reduced motion there is none.
   */
  let {
    content,
    live,
    class: className,
  }: {
    content: string
    /** This turn is being written right now. History does not animate. */
    live: boolean
    class?: string
  } = $props()

  let arrivals = $state<number[]>([])
  // svelte-ignore state_referenced_locally
  // Capturing the INITIAL value is the point: a message that was already
  // there when it mounted must not animate its history.
  let prev = $state(content)
  let now = $state(0)
  let reduced = $state(false)
  let contentEl = $state<HTMLDivElement | null>(null)
  let mask = $state<RectShape[]>([])

  $effect(() => onReducedMotion((r) => (reduced = r)))

  // Record arrival times for characters appended since the last growth. Only
  // growth is an arrival — see the header.
  $effect(() => {
    if (content === prev) return
    arrivals = trackArrivals(prev, content, arrivals, performance.now())
    prev = content
  })

  // The clock, while this turn is live. `now` is read by the deriveds below,
  // so a bucket change is a repaint and a same-bucket frame is not.
  $effect(() => {
    if (!live || reduced) return
    let bucket = -1
    return subscribeToClock(() => {
      const t = performance.now()
      const b = Math.floor(t / CHURN_MS)
      if (b === bucket) return
      bucket = b
      now = t
    })
  })

  const active = $derived(live && !reduced && settling(content, arrivals, now))

  const display = $derived(active ? revealedText(content, arrivals, now) : content)

  // THE FIELD, while the tail is in motion. A uniform body at real strength
  // gives the small text something to condense out of; one slow crest drifting
  // through it keeps the field alive without reporting progress (waves
  // promise an end, a crest does not); shimmer is the house "still, but not
  // dead". Empty when settled — the engine tweens the sources out over its
  // standard budget, so a pause in the stream dissolves the field and the
  // next token brings it back.
  const sources = $derived<DitherSource[]>(
    active
      ? [
          { id: 'stream-veil', kind: 'uniform', strength: 0.22 },
          { id: 'stream-drift', kind: 'wave', axis: 'x', wavelength: 150, speed: 16, strength: 0.08 },
        ]
      : [],
  )

  // THE GLYPHS ARE THE TAIL'S NOISE — see header. Runs after the markdown
  // repaint, which is why `display` is read first. The tail's own line
  // fragments are what the field is measured over below, so the wrap happens
  // BEFORE that effect.
  $effect(() => {
    void display
    if (!live || reduced || !contentEl) return
    const n = unsettledCount(content, arrivals, now)
    if (n === 0) return
    let text: Text | null = null
    const walker = document.createTreeWalker(contentEl, NodeFilter.SHOW_TEXT)
    while (walker.nextNode()) text = walker.currentNode as Text
    if (!text || !text.data.length) return
    const len = text.data.length
    // Clamp to the node: when the run straddles a newline, the earlier part
    // lives in an earlier text node and only the node's own share wraps here
    // (the rest stays at full opacity until it settles — under 220ms).
    const k = Math.min(n, len)
    const start = len - k
    // The newest few characters lead the rain at the brighter opacity; the
    // run arriving behind them is the near-invisible body.
    const head = Math.max(1, Math.round(k * 0.3))
    const wrap = (from: number, to: number, cls: string) => {
      if (from >= to) return
      const range = document.createRange()
      range.setStart(text, from)
      range.setEnd(text, to)
      const span = document.createElement('span')
      span.className = cls
      range.surroundContents(span)
    }
    // Head first, so the body range ends exactly where the new span begins.
    wrap(len - head, len, 'stream-scramble-head')
    wrap(start, len - head, 'stream-scramble')
  })

  // THE FIELD FOLLOWS THE TAIL — see header. One rect per line fragment of
  // every scrambling span, in the content block's box (the canvas fills it).
  // Runs after the wrap effect above; `[]` parks the engine on a clean canvas.
  $effect(() => {
    void display
    const root = contentEl
    if (!root) {
      mask = []
      return
    }
    const spans = root.querySelectorAll<HTMLElement>('.stream-scramble, .stream-scramble-head')
    if (!spans.length) {
      mask = []
      return
    }
    const base = root.getBoundingClientRect()
    const out: RectShape[] = []
    spans.forEach((el) =>
      Array.from(el.getClientRects()).forEach((r) =>
        out.push({ x: r.left - base.left, y: r.top - base.top, w: r.width, h: r.height }),
      ),
    )
    mask = out
  })
</script>

<div class={cn('relative', className)}>
  <DitherLayer {sources} {mask} alphaFloor={0.06} maxAlpha={0.4} shimmer={0.15} />
  <!-- `relative` so the content paints above the canvas: both are positioned,
       and DOM order is what decides. The same pattern as Generating.svelte. -->
  <div class="relative" bind:this={contentEl}>
    <Markdown children={display} />
  </div>
</div>
