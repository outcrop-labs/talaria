<script lang="ts">
  import { cn } from '@/lib/cn'

  /** Count readout (unread, pending) — one look everywhere. Mercury keeps
   *  counts in the mono chrome voice (spec §5); gold = attention, not a fill. */
  let { count, class: className }: { count?: number; class?: string } = $props()
</script>

{#if count}
  <span class={cn('ml-auto shrink-0 font-mono text-[10px] font-medium tracking-[0.05em] text-accent', className)}>
    {count > 99 ? '99+' : count}
  </span>
{/if}
