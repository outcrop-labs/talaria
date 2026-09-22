<script lang="ts">
  import DitherLayer from './DitherLayer.svelte'
  import { cn } from '@/lib/cn'
  import type { DitherEngineOptions, DitherSource } from '@/lib/dither'

  // The RING form of the work field: the same DitherLayer engine, its field
  // masked down to a hairline band around the box. The overlay wash says
  // "work is happening here" across the whole surface; the border is the
  // TRACE — the edge that follows the card, visible even where the wash is
  // too faint to read. It is additive by design: the overlay stays, this
  // rides with it at the same z, under the content's controls.
  //
  // The mask is the classic two-layer exclude: one gradient covering the
  // whole box, one covering only the content box (inset by `ring` of
  // padding), the latter subtracted from the former. Both layers follow the
  // wrapper's border-radius, so the ring turns the same corners as the
  // container it traces — pass the container's radius class in `radius`.
  let {
    sources,
    radius = 'rounded-lg',
    ring = 2,
    class: className,
    ...opts
  }: {
    sources: DitherSource[]
    /** The traced container's own radius class, so the ring matches its corners. */
    radius?: string
    /** Ring thickness in px (the mask's padding). */
    ring?: number
    class?: string
  } & DitherEngineOptions = $props()
</script>

<div
  aria-hidden="true"
  class={cn('pointer-events-none absolute inset-0 z-[5]', radius, className)}
  style:padding="{ring}px"
  style:mask="linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0)"
  style:-webkit-mask="linear-gradient(#000 0 0) content-box, linear-gradient(#000 0 0)"
  style:mask-composite="exclude"
  style:-webkit-mask-composite="xor"
>
  <DitherLayer {sources} {...opts} />
</div>
