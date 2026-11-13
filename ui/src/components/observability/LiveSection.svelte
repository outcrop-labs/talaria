<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Stat from '@/components/settings/Stat.svelte'
  import StatStrip from '@/components/settings/StatStrip.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import { cn } from '@/lib/cn'
  import { agentLabel, formatTokens } from '@/lib/cost.svelte'
  import { relativeTime } from '@/lib/fleet'
  import type { InferenceLive } from './inference'
  import { table } from './grammar'

  /** The live strip: generating-now, gateway pulse, fleet temperature, and the
   *  last hour of per-agent activity. */
  let { live }: { live: InferenceLive } = $props()

  const generatingTotal = $derived(live.generating.reduce((n, g) => n + g.count, 0))
  const genByAgent = $derived(new Map(live.generating.map((g) => [g.agentModel, g.count])))
  const troubled = $derived([
    ...(live.fleet.warming ? [`${live.fleet.warming} warming`] : []),
    ...(live.fleet.unhealthy ? [`${live.fleet.unhealthy} unhealthy`] : []),
    ...(live.fleet.down ? [`${live.fleet.down} down`] : []),
  ])
  const seconds = (ms: number) => `${(ms / 1000).toFixed(1)}s`
</script>

<StatStrip>
  <Stat
    label="Generating now"
    sub={generatingTotal ? `${live.generating.length} agent${live.generating.length === 1 ? '' : 's'}` : 'All quiet'}
  >
    {#snippet value()}
      {#if generatingTotal}
        <span class="flex items-center gap-2">
          <!-- Live monitor, not an inline action: the background rung, which
               the site table pins for both of these. -->
          {generatingTotal} <WaitingMark site="observability/live-total" size={12} class="text-accent" />
        </span>
      {:else}
        0
      {/if}
    {/snippet}
  </Stat>
  <Stat
    label="Gateway requests"
    value={live.gateway.requests.toLocaleString('en-US')}
    sub={`Last 15 min · ${live.gateway.errors ? `${live.gateway.errors} errors` : 'no errors'}`}
  />
  <Stat
    label="Time to first byte"
    value={live.gateway.p50 != null ? seconds(live.gateway.p50) : '—'}
    sub={live.gateway.p95 != null ? `Median · p95 ${seconds(live.gateway.p95)}` : 'No recent requests'}
  />
  <Stat label="Agents running" value={live.fleet.running} sub={troubled.join(' · ') || 'All healthy'} />
</StatStrip>

{#if live.lastHour.length > 0}
  <SettingsSection title="Active this hour" description="Agents that generated in the last 60 minutes.">
    <SettingsGroup>
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
            {#each live.lastHour as a (a.agentModel)}
              <tr class={table.tr}>
                <td class={cn(table.td, 'font-medium')}>{agentLabel(a.agentModel).label}</td>
                <td class={cn(table.td, 'text-[13px] text-muted')}>
                  {#if genByAgent.has(a.agentModel)}
                    <span class="flex items-center gap-2 text-fg">
                      <WaitingMark site="observability/live-agent" size={12} class="text-accent" />Generating
                    </span>
                  {:else}
                    Idle
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
    </SettingsGroup>
  </SettingsSection>
{/if}
