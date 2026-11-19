<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { ChevronRight } from '@lucide/svelte'
  import { p } from '@/router'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Stat from '@/components/settings/Stat.svelte'
  import StatStrip from '@/components/settings/StatStrip.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { getList } from '@/lib/fetch-json'
  import { formatCost, useCost } from '@/lib/cost.svelte'
  import { actorLabel } from '@/components/observability/actor'
  import { relativeTime } from '@/lib/fleet'
  import ActiveAgents from '@/components/observability/ActiveAgents.svelte'
  import { useAlerts } from '@/components/observability/alerts'
  import DailyBars from '@/components/observability/DailyBars.svelte'
  import { useHost } from '@/components/observability/host'
  import { fleetTrouble, useInference } from '@/components/observability/inference'
  import { fact, factLine, factLink, sectionLink } from '@/components/observability/grammar'
  import type { ObsTab } from './observability'

  // Is the system healthy, and what is it costing: four numbers, each a
  // doorway into the tab that owns it, the gateway and the latest activity as
  // one line under them, then the two pictures only this page puts side by
  // side. The alert list and the activity feed live on the Manage overview.
  const tabHref = (tab: ObsTab) => p('/observability/:tab', { params: { tab } })

  // "All clear" is a CLAIM about the fleet. It may only come from a 200 that
  // really carried no problems. A failed /api/alerts has to say so.
  const alertsQuery = useAlerts()
  const alerts = $derived(alertsQuery.data)
  const critical = $derived((alerts ?? []).filter((a) => a.severity === 'critical').length)
  const warnings = $derived((alerts ?? []).filter((a) => a.severity === 'warning').length)
  const alertsLine = $derived.by(() => {
    if (!alerts) return alertsQuery.isError ? 'Could not check' : undefined
    const parts = [critical ? `${critical} critical` : null, warnings ? `${warnings} warning${warnings === 1 ? '' : 's'}` : null]
    return parts.filter(Boolean).join(' · ') || 'All clear'
  })

  const inferenceQuery = useInference()
  const live = $derived(inferenceQuery.data?.live)

  const costQuery = useCost()
  const cost = $derived(costQuery.data?.totals)
  const perDay = $derived(costQuery.data?.perDay ?? [])

  const hostQuery = useHost()
  const host = $derived(hostQuery.data)
  const hostLine = $derived.by(() => {
    if (!host) return hostQuery.isError ? 'Could not read' : undefined
    if (!host.available) return 'Not visible from this process'
    const disk = host.mounts[0]
    const memory = host.memory ? `Memory ${Math.round(host.memory.percent)}%` : null
    const diskPart = disk ? `fullest disk ${Math.round(disk.percent)}%` : null
    const line = [memory, diskPart].filter(Boolean).join(' · ')
    return line ? line.charAt(0).toUpperCase() + line.slice(1) : undefined
  })

  // The same read, and cache key, as the Manage overview's feed.
  const auditQuery = createQuery(() => ({
    queryKey: ['home-activity', ''],
    queryFn: () =>
      getList<{ at: string; actor: string; detail: string; context: string; href: string }>('/api/activity', 'events'),
    refetchInterval: 30_000,
  }))
  const latest = $derived(auditQuery.data?.[0])
  const latestBy = $derived(latest ? [actorLabel(latest.actor), latest.context].filter(Boolean).join(' in ') : '')
</script>

{#snippet pending()}<Skeleton class="my-1.5 h-5 w-16 rounded-full" />{/snippet}

<!-- data-obs-sections: the hook Observability's stagger selector targets, so
     this pane's blocks cascade on the page entrance. -->
<div class="space-y-8" data-obs-sections>
  <div class="space-y-3">
    <StatStrip>
      <Stat label="Open alerts" href={tabHref('alerts')} sub={alertsLine}>
        {#snippet value()}
          {#if alerts}
            <span class="flex items-center gap-2.5">
              {critical + warnings}
              <StatusDot status={critical ? 'danger' : warnings ? 'warn' : 'ok'} class="h-2 w-2" />
            </span>
          {:else if alertsQuery.isError}—{:else}{@render pending()}{/if}
        {/snippet}
      </Stat>
      <Stat
        label="Agents running"
        href={tabHref('compute')}
        sub={live ? fleetTrouble(live.fleet).join(' · ') || 'All healthy' : inferenceQuery.isError ? 'Could not read' : undefined}
      >
        {#snippet value()}
          {#if live}{live.fleet.running}{:else if inferenceQuery.isError}—{:else}{@render pending()}{/if}
        {/snippet}
      </Stat>
      <Stat label="Host CPU" href={tabHref('host')} sub={hostLine}>
        {#snippet value()}
          {#if host?.cpu}{Math.round(host.cpu.percent)}%
          {:else if (host && !host.available) || hostQuery.isError}—
          {:else}{@render pending()}{/if}
        {/snippet}
      </Stat>
      <Stat
        label="Spend today"
        href={tabHref('cost')}
        sub={cost ? `${formatCost(cost.month.cost ?? 0)} this month` : costQuery.isError ? 'Could not read' : undefined}
      >
        {#snippet value()}
          {#if cost}{formatCost(cost.today.cost ?? 0)}{:else if costQuery.isError}—{:else}{@render pending()}{/if}
        {/snippet}
      </Stat>
    </StatStrip>
    <p class={factLine}>
      <span class={fact}>
        {#if live}
          <a href={tabHref('compute')} class={factLink}><span class="tabular-nums text-fg">{live.gateway.requests.toLocaleString('en-US')}</span> gateway requests in the last 15 min</a>{#if live.gateway.errors}<span class="text-fg">, {live.gateway.errors.toLocaleString('en-US')} errors</span>{/if}
        {:else if inferenceQuery.isError}
          <a href={tabHref('compute')} class={factLink}>Gateway unavailable</a>
        {:else}
          <Skeleton class="inline-block h-3 w-40 rounded-full align-middle" />
        {/if}
      </span>
      <span class={fact}>
        {#if latest}
          <a href={tabHref('audit')} class={factLink}>Last activity {relativeTime(latest.at)}</a>, {latestBy}
        {:else if auditQuery.isError}
          <a href={tabHref('audit')} class={factLink}>Activity unavailable</a>
        {:else if auditQuery.data}
          <a href={tabHref('audit')} class={factLink}>No activity yet</a>
        {:else}
          <Skeleton class="inline-block h-3 w-40 rounded-full align-middle" />
        {/if}
      </span>
    </p>
  </div>

  {#if perDay.length > 1}
    <DailyBars days={perDay} href={tabHref('cost')} />
  {/if}

  {#if live}
    <SettingsSection title="Busiest agents this hour">
      {#snippet action()}
        <a href={tabHref('compute')} class={sectionLink}>All active agents<ChevronRight size={14} /></a>
      {/snippet}
      <SettingsGroup>
        {#if live.lastHour.length > 0}
          <ActiveAgents {live} limit={3} />
        {:else}
          <p class="px-5 py-4 font-sans text-sm leading-5 text-muted">No agent has generated in the last hour.</p>
        {/if}
      </SettingsGroup>
    </SettingsSection>
  {/if}
</div>
