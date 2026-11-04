<script lang="ts">
  import { cn } from '@/lib/cn'
  import { fullTime, gutterTime, isoTime, turnTime } from './chat-view'

  // A message's send time as a real `<time datetime>`, in the viewer's zone.
  // The hover title carries the whole stamp (weekday, date, time); the text is
  // the header's time beside the name, or the gutter's shorter one on a
  // continuation line (lib/message-groups). Every row renders one, so each
  // message's time reaches assistive tech even where a block shows it once.
  let {
    at,
    timeZone,
    variant = 'header',
    class: className,
  }: {
    at?: string
    timeZone?: string | null
    variant?: 'header' | 'gutter'
    class?: string
  } = $props()

  const label = $derived(variant === 'gutter' ? gutterTime(at, timeZone) : turnTime(at, timeZone))
</script>

{#if label}
  <time
    datetime={isoTime(at)}
    title={fullTime(at, timeZone)}
    class={cn('font-mono tracking-[0.05em] text-muted', variant === 'gutter' ? 'text-[9px]' : 'text-[10px]', className)}
  >
    {label}
  </time>
{/if}
