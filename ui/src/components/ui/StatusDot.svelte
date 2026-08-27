<script lang="ts">
  import { cn } from '@/lib/cn'
  import { DOT_COLOR, type DotStatus } from './chip'

  // The one status dot — 6px round (spec §8), status decides the color
  // (green healthy / gold attention / orange failure), `pulse` for live:
  // MONITOR BREATHE on the ambient budget (spec §9), only while the state
  // is actually active.
  //
  // `color` is for dots whose color is DATA, not health — a board status
  // hue, a priority, a chart series. Those are not states of the fleet, so
  // they have no business in DotStatus, but before this prop they had no way
  // to say their color here either, and every one of them hand-rolled this
  // span. `status` stays the vocabulary for the five health tones; `color`
  // replaces the table when both are given.
  let {
    status,
    color,
    pulse,
    title,
    class: className,
  }: { status?: DotStatus; color?: string; pulse?: boolean; title?: string; class?: string } = $props()

  const background = $derived(color ?? (status != null ? DOT_COLOR[status] : undefined))
</script>

<span
  {title}
  class={cn('inline-block h-1.5 w-1.5 shrink-0 rounded-full', pulse && 'gd-breathe', className)}
  style:background={background}
></span>
