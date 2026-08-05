<script lang="ts">
  import { cn } from '@/lib/cn'
  import { relativeTime } from '@/lib/fleet'
  import type { FocusItem } from '@/lib/inbox-focus.svelte'
  import { priorityClass } from './focus-inbox'

  let { items, remaining }: { items: FocusItem[]; remaining: number } = $props()
</script>

<section aria-labelledby="queue-heading" class="py-8">
  <div class="mb-2 flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
    <h2 id="queue-heading">In queue</h2><span>· {remaining}</span>
  </div>
  {#if items.length === 0}
    <p class="font-sans text-sm text-muted">No other decisions are waiting.</p>
  {:else}
    <!-- No listStagger here: FocusInbox renders this section inside
         <Materialize>, whose content branch owns the region's cascade. -->
    <ol class="divide-y divide-line border-y border-line">
      {#each items as item (item.key)}
        <li class="grid min-h-11 grid-cols-[2.5rem_minmax(0,1fr)_auto] items-center gap-3 py-2">
          <span class={cn('font-mono text-[10px] uppercase tracking-[0.06em]', priorityClass(item.priority))}>{item.priority}</span>
          <span class="min-w-0 truncate font-sans text-[13px] text-muted">{item.question}</span>
          <span class="font-mono text-[9px] uppercase tracking-[0.05em] text-ink-dim">{relativeTime(item.createdAt)}</span>
        </li>
      {/each}
    </ol>
  {/if}
</section>
