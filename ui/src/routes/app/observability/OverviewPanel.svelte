<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { ChevronRight } from '@lucide/svelte'
  import { navigateHref, p } from '@/router'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Stat from '@/components/settings/Stat.svelte'
  import StatStrip from '@/components/settings/StatStrip.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { getList } from '@/lib/fetch-json'
  import { formatCost, formatTokens, useCost } from '@/lib/cost.svelte'
  import { relativeTime } from '@/lib/fleet'
  import AlertRow from '@/components/observability/AlertRow.svelte'
  import { useAlerts } from '@/components/observability/alerts'
  import { useHost } from '@/components/observability/host'
  import { useInference } from '@/components/observability/inference'
  import { sectionLink } from '@/components/observability/grammar'
  import type { ObsTab } from './observability'

  // The cross-section: one number per detail tab, then what is wrong, then
  // what changed. Every block is a doorway into the tab that owns it.
  const tabHref = (tab: ObsTab) => p('/observability/:tab', { params: { tab } })

  // "All clear" is a CLAIM about the fleet. It may only come from a 200 that
  // really carried no problems. A failed /api/alerts has to say so.
  const alertsQuery = useAlerts()
  const alerts = $derived(alertsQuery.data)
  const problems = $derived(
    (alerts ?? [])
      .filter((a) => a.severity !== 'info')
      .sort((a, b) => (a.severity === b.severity ? 0 : a.severity === 'critical' ? -1 : 1)),
  )
  const SHOWN = 5

  const inferenceQuery = useInference()
  const live = $derived(inferenceQuery.data?.live)
  const generating = $derived(live?.generating.reduce((n, g) => n + g.count, 0) ?? 0)
  const fleetLine = $derived(
    live
      ? [
          `${live.fleet.running} agents up`,
          ...(live.fleet.warming ? [`${live.fleet.warming} warming`] : []),
          ...(live.fleet.unhealthy ? [`${live.fleet.unhealthy} unhealthy`] : []),
          ...(live.fleet.down ? [`${live.fleet.down} down`] : []),
        ].join(' · ')
      : undefined,
  )
  const gatewayLine = $derived(
    live
      ? `Last 15 min · ${live.gateway.errors ? `${live.gateway.errors} errors` : 'no errors'}${
          live.gateway.p50 != null ? ` · p50 ${(live.gateway.p50 / 1000).toFixed(1)}s` : ''
        }`
      : undefined,
  )

  const costQuery = useCost()
  const cost = $derived(costQuery.data?.totals)

  const hostQuery = useHost()
  const host = $derived(hostQuery.data)
  const hostLine = $derived.by(() => {
    if (!host) return hostQuery.isError ? 'Could not read' : undefined
    if (!host.available) return 'Not visible from this process'
    const disk = host.mounts[0]
    const memory = host.memory ? `Memory ${Math.round(host.memory.percent)}%` : null
    const diskPart = disk ? `Fullest disk ${Math.round(disk.percent)}%` : null
    return [memory, diskPart].filter(Boolean).join(' · ') || undefined
  })

  const auditQuery = createQuery(() => ({
    queryKey: ['home-activity', ''],
    queryFn: () =>
      getList<{ at: string; actor: string; detail: string; context: string; href: string }>('/api/activity', 'events'),
    refetchInterval: 30_000,
  }))
  const audit = $derived(auditQuery.data)
</script>

{#snippet pending()}<Skeleton class="my-1.5 h-5 w-16 rounded-full" />{/snippet}

<!-- data-obs-sections: the hook Observability's stagger selector targets, so
     this pane's blocks cascade on the page entrance. -->
<div class="space-y-10" data-obs-sections>
  <StatStrip>
    <Stat label="Generating now" href={tabHref('compute')} sub={fleetLine}>
      {#snippet value()}
        {#if live}{generating}{:else if inferenceQuery.isError}—{:else}{@render pending()}{/if}
      {/snippet}
    </Stat>
    <Stat label="Gateway requests" href={tabHref('compute')} sub={gatewayLine}>
      {#snippet value()}
        {#if live}{live.gateway.requests.toLocaleString('en-US')}{:else if inferenceQuery.isError}—{:else}{@render pending()}{/if}
      {/snippet}
    </Stat>
    <Stat
      label="Spend today"
      href={tabHref('cost')}
      sub={cost
        ? `${formatTokens(cost.today.prompt + cost.today.completion)} tokens · ${formatCost(cost.month.cost ?? 0)} this month`
        : undefined}
    >
      {#snippet value()}
        {#if cost}{formatCost(cost.today.cost ?? 0)}{:else if costQuery.isError}—{:else}{@render pending()}{/if}
      {/snippet}
    </Stat>
    <Stat label="Host CPU" href={tabHref('host')} sub={hostLine}>
      {#snippet value()}
        {#if host?.cpu}{Math.round(host.cpu.percent)}%
        {:else if (host && !host.available) || hostQuery.isError}—
        {:else}{@render pending()}{/if}
      {/snippet}
    </Stat>
  </StatStrip>

  <SettingsSection title="Needs attention">
    {#snippet action()}
      {#if alertsQuery.isLoading}
        <Skeleton class="h-3 w-8 rounded-full" />
      {:else if alerts === undefined}
        <!-- Unknown, not clear. The card below carries the reason. -->
        <span class="font-sans text-[13px] leading-5 text-muted">Unknown</span>
      {:else if problems.length > 0}
        <Chip tone="warn">{problems.length} open</Chip>
      {:else}
        <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted"><StatusDot status="ok" />All clear</span>
      {/if}
      <a href={tabHref('alerts')} class={sectionLink}>All alerts<ChevronRight size={14} /></a>
    {/snippet}
    <SettingsGroup>
      {#if alertsQuery.isLoading}
        <div class="px-5 py-4"><SkeletonRows rows={2} /></div>
      {:else if alerts === undefined}
        <div class="px-5 py-4">
          <QueryError
            variant="inline"
            error={alertsQuery.error}
            title="Could not load alerts"
            onRetry={() => void alertsQuery.refetch()}
          />
        </div>
      {:else if problems.length > 0}
        {#each problems.slice(0, SHOWN) as a, i (i)}
          <AlertRow alert={a} />
        {/each}
        {#if problems.length > SHOWN}
          <a href={tabHref('alerts')} class="block px-5 py-3 font-sans text-[13px] leading-5 text-muted transition-colors hover:text-fg">
            {problems.length - SHOWN} more in Alerts
          </a>
        {/if}
      {:else}
        <p class="px-5 py-4 font-sans text-sm leading-5 text-muted">Nothing is asking for you right now.</p>
      {/if}
    </SettingsGroup>
  </SettingsSection>

  <SettingsSection title="Recent activity">
    {#snippet action()}
      <a href={tabHref('audit')} class={sectionLink}>Full audit log<ChevronRight size={14} /></a>
    {/snippet}
    <SettingsGroup>
      {#if auditQuery.isLoading}
        <div class="px-5 py-4"><SkeletonRows rows={5} /></div>
      {:else if audit === undefined}
        <!-- "Quiet so far." is a fact about the log, not about a failed request. -->
        <div class="px-5 py-4">
          <QueryError
            variant="inline"
            error={auditQuery.error}
            title="Could not load recent activity"
            onRetry={() => void auditQuery.refetch()}
          />
        </div>
      {:else if audit.length === 0}
        <p class="px-5 py-4 font-sans text-sm leading-5 text-muted">Quiet so far.</p>
      {:else}
        {#each audit.slice(0, 6) as a, i (i)}
          <button
            type="button"
            onclick={() => a.href && void navigateHref(a.href)}
            class="flex h-11 w-full items-center gap-6 px-5 text-left font-sans text-sm leading-5 transition-colors hover:bg-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
          >
            <span class="min-w-0 flex-1 truncate text-muted">
              <span class="font-medium text-fg">{a.actor}</span>
              {a.detail}
            </span>
            <span class="shrink-0 text-[13px] tabular-nums text-muted">{relativeTime(a.at)}</span>
          </button>
        {/each}
      {/if}
    </SettingsGroup>
  </SettingsSection>
</div>
