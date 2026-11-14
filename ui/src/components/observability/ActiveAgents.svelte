<script lang="ts">
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { cn } from '@/lib/cn'
  import { agentLabel, formatTokens } from '@/lib/cost.svelte'
  import { relativeTime } from '@/lib/fleet'
  import type { InferenceLive } from './inference'
  import { table } from './grammar'

  /** The agents that generated in the last hour. With `limit`, only the
   *  busiest few, for the Overview's summary. */
  let { live, limit }: { live: InferenceLive; limit?: number } = $props()

  const generating = $derived(new Set(live.generating.map((g) => g.agentModel)))
  const rows = $derived(
    limit === undefined ? live.lastHour : [...live.lastHour].sort((a, b) => b.generations - a.generations).slice(0, limit),
  )
</script>

<div class="overflow-x-auto">
  <table class={table.root}>
    <thead>
      <tr>
        <th class={table.th}>Agent</th>
        <th class={table.th}>Status</th>
        <th class={cn(table.th, table.num)}>Generations</th>
        <th class={cn(table.th, table.num)}>Tokens</th>
        <th class={cn(table.th, table.num)}>Last active</th>
      </tr>
    </thead>
    <tbody>
      {#each rows as a (a.agentModel)}
        <tr class={table.tr}>
          <td class={cn(table.td, 'font-medium')}>{agentLabel(a.agentModel).label}</td>
          <td class={cn(table.td, 'text-[13px] text-muted')}>
            {#if generating.has(a.agentModel)}
              <span class="flex items-center gap-2 text-fg">
                <WaitingMark site="observability/live-agent" size={12} class="text-accent" />Generating
              </span>
            {:else}
              <span class="flex items-center gap-2"><StatusDot status="idle" />Idle</span>
            {/if}
          </td>
          <td class={cn(table.td, table.num)}>{a.generations.toLocaleString('en-US')}</td>
          <td class={cn(table.td, table.num)}>{formatTokens(a.tokens)}</td>
          <td class={cn(table.td, table.num, 'text-muted')}>{relativeTime(a.lastAt)}</td>
        </tr>
      {/each}
    </tbody>
  </table>
</div>
