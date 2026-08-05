<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { navigateHref } from '@/router'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getJson, getList } from '@/lib/fetch-json'
  import { formatTokens } from '@/lib/cost.svelte'
  import ActivityRow from '@/components/app/ActivityRow.svelte'
  import type { ObsTab } from './observability'

  // ── Overview: the cross-section — worst news first ──────────────────────────
  interface OverviewAlert {
    severity: 'critical' | 'warning' | 'info'
    title: string
    href: string
  }
  const SEV_COLOR: Record<OverviewAlert['severity'], string> = {
    critical: 'var(--theme-danger)',
    warning: 'var(--theme-warning)',
    info: 'var(--theme-line)',
  }

  let { onOpen }: { onOpen: (t: ObsTab) => void } = $props()

  // "all clear" is a CLAIM about the fleet. It may only come from a 200 that
  // really carried no problems — a failed /api/alerts has to say so.
  const alertsQuery = createQuery(() => ({
    queryKey: ['alerts'],
    queryFn: (): Promise<OverviewAlert[]> => getList<OverviewAlert>('/api/alerts', 'alerts'),
    refetchInterval: 60_000,
  }))
  const alerts = $derived(alertsQuery.data)
  const alertsLoading = $derived(alertsQuery.isLoading)
  const inferenceQuery = createQuery(() => ({
    queryKey: ['inference'],
    queryFn: () =>
      getJson<{
        live: {
          generating: Array<{ agentModel: string; count: number }>
          gateway: { requests: number; errors: number; p50: number | null }
          fleet: { running: number; warming: number; unhealthy: number; down: number }
        }
      }>('/api/inference'),
    refetchInterval: 15_000,
  }))
  const costQuery = createQuery(() => ({
    queryKey: ['cost'],
    queryFn: () =>
      getJson<{ totals: { today: { prompt: number; completion: number; cost: number }; month: { cost: number } } }>('/api/cost'),
    refetchInterval: 60_000,
  }))
  const cost = $derived(costQuery.data)
  const auditQuery = createQuery(() => ({
    queryKey: ['home-activity', ''],
    queryFn: () =>
      getList<{ at: string; actor: string; detail: string; context: string; href: string }>('/api/activity', 'events'),
    refetchInterval: 30_000,
  }))
  const audit = $derived(auditQuery.data)
  const auditLoading = $derived(auditQuery.isLoading)

  const live = $derived(inferenceQuery.data?.live)
  const generating = $derived(live?.generating.reduce((n, g) => n + g.count, 0) ?? 0)
  const fleetParts = $derived(
    live
      ? [
          `${live.fleet.running} up`,
          ...(live.fleet.warming ? [`${live.fleet.warming} warming`] : []),
          ...(live.fleet.unhealthy ? [`${live.fleet.unhealthy} unhealthy`] : []),
          ...(live.fleet.down ? [`${live.fleet.down} down`] : []),
        ].join(' · ')
      : null,
  )
  const problems = $derived((alerts ?? []).filter((a) => a.severity !== 'info'))
</script>

<!-- data-obs-sections: the hook Observability's data-stagger-items selector
     targets — this pane's three blocks cascade on the page entrance. -->
<div class="space-y-6" data-obs-sections>
  <!-- Worst news first: alerts strip -->
  <Panel>
    <button type="button" onclick={() => onOpen('alerts')} class="group -mx-1.5 mb-3 flex min-h-6 w-[calc(100%+0.75rem)] items-center gap-2 rounded px-1.5 text-left transition-colors duration-[120ms] hover:bg-hover">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Alerts</span>
      {#if alertsLoading}
        <Skeleton class="h-3 w-8 rounded-full" />
      {:else if alerts === undefined}
        <!-- Unknown, not clear — the strip below carries the reason. -->
        <span class="text-xs text-muted">unknown</span>
      {:else if problems.length > 0}
        <span class="rounded border border-warning/40 px-1.5 font-mono text-[10px] tracking-[0.05em] text-warning">{problems.length}</span>
      {:else}
        <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-success">● all clear</span>
      {/if}
      <span class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors group-hover:text-accent">Open →</span>
    </button>
    {#if alertsLoading}
      <SkeletonRows rows={2} />
    {:else if alerts === undefined}
      <QueryError
        variant="inline"
        error={alertsQuery.error}
        title="Could not load alerts"
        onRetry={() => void alertsQuery.refetch()}
      />
    {:else if problems.length > 0}
      <ul class="space-y-1.5">
        {#each problems.slice(0, 3) as a, i (i)}
          <li class="flex items-center gap-2.5 text-sm">
            <span class="h-[7px] w-[7px] shrink-0 rounded-full" style:background={SEV_COLOR[a.severity]}></span>
            <span class="min-w-0 flex-1 truncate font-sans text-fg">{a.title}</span>
          </li>
        {/each}
      </ul>
    {/if}
  </Panel>

  <div class="grid gap-4 sm:grid-cols-3">
    <!-- §8 stat blocks: big sans numeral + 10px mono uppercase label. -->
    <Panel class="p-5">
      <button type="button" onclick={() => onOpen('compute')} class="w-full text-left">
        <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Generating now</div>
        {#if live}
          <div class="mt-1.5 font-sans text-2xl font-semibold text-fg">{generating}</div>
          <div class="mt-1 truncate font-mono text-[11px] text-muted">{fleetParts}</div>
        {:else}
          <Skeleton class="mt-2 h-5 w-16 rounded-full" />
        {/if}
      </button>
    </Panel>
    <Panel class="p-5">
      <button type="button" onclick={() => onOpen('compute')} class="w-full text-left">
        <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Gateway · 15 min</div>
        {#if live}
          <div class="mt-1.5 font-sans text-2xl font-semibold text-fg">{live.gateway.requests}</div>
          <div class="mt-1 font-mono text-[11px] text-muted">
            {live.gateway.errors ? `${live.gateway.errors} errors` : 'no errors'}{live.gateway.p50 != null ? ` · p50 ${(live.gateway.p50 / 1000).toFixed(1)}s` : ''}
          </div>
        {:else}
          <Skeleton class="mt-2 h-5 w-16 rounded-full" />
        {/if}
      </button>
    </Panel>
    <Panel class="p-5">
      <button type="button" onclick={() => onOpen('cost')} class="w-full text-left">
        <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Spend today</div>
        {#if cost}
          <div class="mt-1.5 font-sans text-2xl font-semibold text-fg">${cost.totals.today.cost.toFixed(2)}</div>
          <div class="mt-1 font-mono text-[11px] text-muted">
            {formatTokens(cost.totals.today.prompt + cost.totals.today.completion)} tokens · ${cost.totals.month.cost.toFixed(2)} this month
          </div>
        {:else}
          <Skeleton class="mt-2 h-5 w-16 rounded-full" />
        {/if}
      </button>
    </Panel>
  </div>

  <Panel>
    <button type="button" onclick={() => onOpen('audit')} class="group -mx-1.5 mb-3 flex min-h-6 w-[calc(100%+0.75rem)] items-center gap-2 rounded px-1.5 text-left transition-colors duration-[120ms] hover:bg-hover">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Audit trail</span>
      <span class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors group-hover:text-accent">Open →</span>
    </button>
    {#if auditLoading}
      <SkeletonRows rows={5} />
    {:else if audit === undefined}
      <!-- "Quiet so far." is a fact about the audit log, not about a failed
           request — a broken /api/activity says broken. -->
      <QueryError
        variant="inline"
        error={auditQuery.error}
        title="Could not load the audit trail"
        onRetry={() => void auditQuery.refetch()}
      />
    {:else if audit.length === 0}
      <div class="text-xs text-muted">Quiet so far.</div>
    {:else}
      <ul>
        {#each audit.slice(0, 6) as a, i (i)}
          <ActivityRow actor={a.actor} detail={a.detail} at={a.at} onClick={() => a.href && void navigateHref(a.href)} />
        {/each}
      </ul>
    {/if}
  </Panel>
</div>
