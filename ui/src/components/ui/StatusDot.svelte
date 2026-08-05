<script lang="ts">
  import { cn } from '@/lib/cn'
  import { DOT_COLOR, type DotStatus } from './chip'

  // The one status dot — 6px round (spec §8), status decides the color
  // (green healthy / gold attention / orange failure), `pulse` for live:
  // MONITOR BREATHE on the ambient budget (spec §9), only while the state
  // is actually active.
  let {
    status,
    pulse,
    title,
    class: className,
  }: { status: DotStatus; pulse?: boolean; title?: string; class?: string } = $props()
</script>

<span
  {title}
  class={cn('inline-block h-1.5 w-1.5 shrink-0 rounded-full', pulse && 'gd-breathe', className)}
  style:background={DOT_COLOR[status]}
></span>
