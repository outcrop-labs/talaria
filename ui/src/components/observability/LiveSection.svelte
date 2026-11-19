<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Stat from '@/components/settings/Stat.svelte'
  import StatStrip from '@/components/settings/StatStrip.svelte'
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import ActiveAgents from './ActiveAgents.svelte'
  import { fleetTrouble, type InferenceLive } from './inference'

  /** The live strip: generating-now, gateway pulse, fleet temperature, and the
   *  last hour of per-agent activity. */
  let { live }: { live: InferenceLive } = $props()

  const generatingTotal = $derived(live.generating.reduce((n, g) => n + g.count, 0))
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
    sub={`${live.gateway.errors ? `${live.gateway.errors} errors` : 'No errors'} · last 15 min`}
  />
  <Stat
    label="Time to first byte"
    value={live.gateway.p50 != null ? seconds(live.gateway.p50) : '—'}
    sub={live.gateway.p95 != null ? `Median · p95 ${seconds(live.gateway.p95)}` : 'No recent requests'}
  />
  <Stat label="Agents running" value={live.fleet.running} sub={fleetTrouble(live.fleet).join(' · ') || 'All healthy'} />
</StatStrip>

{#if live.lastHour.length > 0}
  <SettingsSection title="Active this hour">
    <SettingsGroup>
      <ActiveAgents {live} />
    </SettingsGroup>
  </SettingsSection>
{/if}
