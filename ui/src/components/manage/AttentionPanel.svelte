<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { navigateHref } from '@/router'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import type { DotStatus } from '@/components/ui/chip'
  import { getList } from '@/lib/fetch-json'

  // WORST NEWS FIRST. The alerts the instance is raising about itself —
  // agents down or unhealthy, unroutable brains, MCP and retrieval planes,
  // the email outbox, unpriced usage — each a row that goes where it can be
  // fixed. `/api/alerts` filters to what this viewer may act on.
  //
  // "All clear" is a CLAIM about the instance. It only comes from a 200 that
  // really carried no problems; a failed read says so instead.
  interface Alert {
    severity: 'critical' | 'warning' | 'info'
    title: string
    detail?: string
    href?: string
  }
  const SEV: Record<Alert['severity'], DotStatus> = { critical: 'danger', warning: 'warn', info: 'idle' }

  // The same key the Observability overview reads, so the two share a cache.
  const alertsQuery = createQuery(() => ({
    queryKey: ['alerts'],
    queryFn: (): Promise<Alert[]> => getList<Alert>('/api/alerts', 'alerts'),
    refetchInterval: 60_000,
  }))
  const alerts = $derived(alertsQuery.data)
  const problems = $derived(
    (alerts ?? [])
      .filter((a) => a.severity !== 'info')
      .sort((a, b) => (a.severity === b.severity ? 0 : a.severity === 'critical' ? -1 : 1)),
  )
  const SHOWN = 5
</script>

<Panel class="px-4 py-3">
  <div class="mb-2 flex min-h-6 items-center gap-2">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Needs attention</span>
    {#if alertsQuery.isLoading}
      <Skeleton class="h-3 w-8 rounded-full" />
    {:else if alerts === undefined}
      <span class="text-xs text-muted">unknown</span>
    {:else if problems.length > 0}
      <span class="rounded border border-warning/40 px-1.5 font-mono text-[10px] tracking-[0.05em] text-warning">{problems.length}</span>
    {:else}
      <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-success">● all clear</span>
    {/if}
  </div>
  {#if alertsQuery.isLoading}
    <SkeletonRows rows={2} />
  {:else if alerts === undefined}
    <QueryError
      variant="inline"
      error={alertsQuery.error}
      title="Could not load alerts"
      onRetry={() => void alertsQuery.refetch()}
    />
  {:else if problems.length > 0}
    <ul class="divide-y divide-line-subtle">
      {#each problems.slice(0, SHOWN) as a, i (i)}
        <li>
          <button
            type="button"
            disabled={!a.href}
            onclick={() => a.href && void navigateHref(a.href)}
            class="group -mx-1.5 flex w-[calc(100%+0.75rem)] items-center gap-2.5 rounded px-1.5 py-2 text-left transition-colors duration-[120ms] enabled:dither-fill"
          >
            <StatusDot status={SEV[a.severity]} class="h-[7px] w-[7px] shrink-0" />
            <span class="min-w-0 flex-1">
              <span class="block truncate font-sans text-sm text-fg">{a.title}</span>
              {#if a.detail}<span class="block truncate font-sans text-xs text-muted">{a.detail}</span>{/if}
            </span>
            {#if a.href}
              <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors group-hover:text-accent">Fix →</span>
            {/if}
          </button>
        </li>
      {/each}
    </ul>
    {#if problems.length > SHOWN}
      <p class="mt-1 font-sans text-xs text-muted">
        and {problems.length - SHOWN} more under Observability → Alerts.
      </p>
    {/if}
  {:else}
    <p class="font-sans text-sm text-muted">Nothing is asking for you right now.</p>
  {/if}
</Panel>
