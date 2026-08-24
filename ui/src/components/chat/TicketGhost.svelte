<script lang="ts" module>
  export interface GhostSpec {
    /** Tailwind width class for the title pill. */
    title: string
    /** Width classes for the description lines. */
    lines: string[]
    /** Width classes for the blocked-by chips. */
    chips: string[]
  }
</script>

<script lang="ts">
  // ONE GHOST TICKET, DRAWN ENTIRELY IN DITHER — no solid bars, no field
  // floating over graphics. The DOM inside is geometry donors: invisible
  // elements carrying only the sizes, radii and positions of the card's
  // paths (box, checkbox, title pill, priority chips, description lines,
  // blocked-by chips). Each is measured into a rect source and the canvas
  // paints the path itself as dither cells — the dots ARE the ticket.
  //
  // The living half is the mask. Two crests travel through the card, but a
  // wave alone paints wherever it pleases — which is how an earlier cut
  // shipped as a shimmer grid OVER the graphics. The mask clips painting to
  // the union of the path rects, so a crest only exists where a path is:
  // cells cross the dither threshold along the bars and the lit pattern
  // slides through the geometry. Paths stay readable in the troughs (their
  // own density carries them); crests brighten and reshuffle them.
  import DitherLayer, { rectIn } from '@/components/ui/DitherLayer.svelte'
  import type { DitherSource, MaskRect } from '@/lib/dither'

  let { spec, seed = 0 }: { spec: GhostSpec; seed?: number } = $props()

  let root = $state<HTMLDivElement | null>(null)
  let sources = $state<DitherSource[]>([])
  let mask = $state<MaskRect[] | null>(null)

  $effect(() => {
    if (!root) return
    const build = () => {
      const paths: DitherSource[] = []
      const clip: MaskRect[] = []
      const box = root!.querySelector<HTMLElement>('[data-path="box"]')
      if (box) {
        const r = rectIn(root!, box)
        paths.push({
          id: 'box',
          kind: 'rect',
          x: r.x,
          y: r.y,
          w: r.w,
          h: r.h,
          radius: parseFloat(getComputedStyle(box).borderTopLeftRadius) || 0,
          spread: 0,
          // inner 0 + rim: the box is drawn as its BORDER — a band of cells
          // hugging the boundary, nothing in the interior.
          strength: 0.55,
          inner: 0,
          rim: 7,
        })
        // The border's share of the clip: four strips forming a ring, so the
        // travelling crests cannot wash the box's interior either.
        const t = 8
        clip.push(
          { x: r.x, y: r.y, w: r.w, h: t },
          { x: r.x, y: r.y + r.h - t, w: r.w, h: t },
          { x: r.x, y: r.y, w: t, h: r.h },
          { x: r.x + r.w - t, y: r.y, w: t, h: r.h },
        )
      }
      root!.querySelectorAll<HTMLElement>('[data-path]:not([data-path="box"])').forEach((el, i) => {
        const r = rectIn(root!, el)
        paths.push({
          id: `path-${i}`,
          kind: 'rect',
          x: r.x,
          y: r.y,
          w: r.w,
          h: r.h,
          radius: parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0,
          spread: 0,
          strength: 0.6,
        })
        clip.push({ x: r.x, y: r.y, w: r.w, h: r.h })
      })
      sources = [
        ...paths,
        // Seed-varied so the six ghosts don't shimmer in lockstep.
        { id: 'flow', kind: 'wave', axis: 'y', wavelength: 120 + seed * 9, speed: 22 + seed * 3, strength: 0.45 },
        { id: 'drift', kind: 'wave', axis: 'x', wavelength: 64 + seed * 5, speed: -10 - seed * 2, strength: 0.25 },
      ]
      mask = clip
    }
    build()
    const ro = new ResizeObserver(build)
    ro.observe(root)
    return () => ro.disconnect()
  })
</script>

<div bind:this={root} class="relative h-32">
  {#if sources.length > 0}
    <DitherLayer {sources} {mask} alphaFloor={0.26} maxAlpha={0.8} shimmer={0.08} />
  {/if}
  <!-- Geometry donors: invisible (layout intact, nothing painted). Their
       border-radius is read as well as their box, so pills mask as pills and
       the card's rounded corners wrap correctly. -->
  <div class="invisible relative flex h-full flex-col gap-3 p-3.5">
    <div data-path="box" class="absolute inset-0 rounded-xl"></div>
    <div class="relative flex items-center gap-2.5">
      <div data-path class="h-3.5 w-3.5 shrink-0 rounded-[3px]"></div>
      <div data-path class={`h-2.5 rounded-full ${spec.title}`}></div>
      <span class="flex-1"></span>
      <div data-path class="h-3 w-8 rounded-full"></div>
      <div data-path class="h-3 w-8 rounded-full"></div>
    </div>
    <div class="space-y-2">
      {#each spec.lines as w, j (j)}
        <div data-path class={`h-2 rounded-full ${w}`}></div>
      {/each}
    </div>
    <div class="mt-auto flex items-center gap-1.5">
      {#each spec.chips as w, j (j)}
        <div data-path class={`h-3 rounded-full ${w}`}></div>
      {/each}
    </div>
  </div>
</div>
