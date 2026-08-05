<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import Panel from '@/components/ui/Panel.svelte'

  // The §8 stat block: big sans numeral in readout cream + 10px mono uppercase
  // ink-dim label + optional mono sub line. Reuse for fleet/cost/activity
  // summaries.
  let {
    label,
    value,
    sub,
    class: className,
  }: { label: string; value: string | number | Snippet; sub?: string; class?: string } = $props()
</script>

<Panel class={cn('p-5', className)}>
  <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</div>
  <div class="mt-1.5 font-sans text-2xl font-semibold text-fg">
    {#if typeof value === 'function'}{@render value()}{:else}{value}{/if}
  </div>
  {#if sub}<div class="mt-1 font-mono text-[11px] text-muted">{sub}</div>{/if}
</Panel>
