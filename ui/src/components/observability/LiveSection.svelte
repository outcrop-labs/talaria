<script lang="ts">
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import StatCard from '@/components/ui/StatCard.svelte'
  import { formatTokens } from '@/lib/cost.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { listStagger } from '@/lib/motion'
  import type { InferenceLive } from './inference'

  /** The live strip: generating-now, gateway pulse, fleet temperature, and the
   *  last hour of per-agent activity. */
  let { live }: { live: InferenceLive } = $props()

  const generatingTotal = $derived(live.generating.reduce((n, g) => n + g.count, 0))
  const genByAgent = $derived(new Map(live.generating.map((g) => [g.agentModel, g.count])))
  const fleetParts = $derived([
    `${live.fleet.running} up`,
    ...(live.fleet.warming ? [`${live.fleet.warming} warming`] : []),
    ...(live.fleet.unhealthy ? [`${live.fleet.unhealthy} unhealthy`] : []),
    ...(live.fleet.down ? [`${live.fleet.down} down`] : []),
  ])
</script>

<div class="grid grid-cols-4 gap-4">
  <StatCard
    label="Generating now"
    sub={generatingTotal ? `${live.generating.length} agent${live.generating.length === 1 ? '' : 's'}` : 'all quiet'}
  >
    {#snippet value()}
      {#if generatingTotal}
        <span class="flex items-center gap-2">
          <!-- Live monitor, not an inline action — the background rung
               (spec §9), which the site table pins for both of these. -->
          {generatingTotal} <WaitingMark site="observability/live-total" size={12} class="text-accent" />
        </span>
      {:else}
        0
      {/if}
    {/snippet}
  </StatCard>
  <StatCard
    label="Gateway · 15 min"
    value={String(live.gateway.requests)}
    sub={live.gateway.errors ? `${live.gateway.errors} errors` : 'no errors'}
  />
  <StatCard
    label="First byte"
    value={live.gateway.p50 != null ? `${(live.gateway.p50 / 1000).toFixed(1)}s` : '—'}
    sub={live.gateway.p95 != null ? `p95 ${(live.gateway.p95 / 1000).toFixed(1)}s` : 'no recent requests'}
  />
  <StatCard label="Fleet" value={fleetParts[0]!} sub={fleetParts.slice(1).join(' · ') || 'all healthy'} />
</div>
{#if live.lastHour.length > 0}
  <Panel>
    <SectionHeader title="Active this hour" action={String(live.lastHour.length).padStart(2, '0')} />
    <div class="divide-y divide-line" use:listStagger>
      {#each live.lastHour as a (a.agentModel)}
        <div data-dither-fill class="flex items-center gap-3 py-3 text-sm transition-colors">
          <span class="min-w-0 flex-1 truncate font-mono text-xs text-fg">{a.agentModel}</span>
          {#if genByAgent.has(a.agentModel)}<WaitingMark site="observability/live-agent" size={12} class="text-accent" />{/if}
          <span class="shrink-0 font-mono text-[11px] text-muted">{a.generations} gen</span>
          <span class="shrink-0 font-mono text-[11px] text-muted">{formatTokens(a.tokens)} tokens</span>
          <span class="w-16 shrink-0 text-right font-mono text-[11px] text-muted">{relativeTime(a.lastAt)}</span>
        </div>
      {/each}
    </div>
  </Panel>
{/if}
