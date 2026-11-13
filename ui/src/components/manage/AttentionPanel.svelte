<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { navigateHref } from '@/router'
  import { ChevronRight } from '@lucide/svelte'
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import Chip from '@/components/ui/Chip.svelte'
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

<SettingsSection title="Needs attention">
  {#snippet action()}
    {#if alertsQuery.isLoading}
      <Skeleton class="h-3 w-8 rounded-full" />
    {:else if alerts === undefined}
      <span class="font-sans text-[13px] leading-5 text-muted">Unknown</span>
    {:else if problems.length > 0}
      <Chip tone="warn">{problems.length} open</Chip>
    {:else}
      <span class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted"><StatusDot status="ok" />All clear</span>
    {/if}
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
        <SettingsRow
          label={a.title}
          hint={a.detail}
          disabled={!a.href}
          onclick={() => a.href && void navigateHref(a.href)}
        >
          {#snippet lead()}<StatusDot status={SEV[a.severity]} class="h-2 w-2" />{/snippet}
          {#if a.href}
            <span class="flex items-center gap-1 font-sans text-[13px] font-medium leading-5 text-muted">Fix<ChevronRight size={14} /></span>
          {/if}
        </SettingsRow>
      {/each}
      {#if problems.length > SHOWN}
        <a href="/observability/alerts" class="block px-5 py-3 font-sans text-[13px] leading-5 text-muted transition-colors hover:text-fg">
          {problems.length - SHOWN} more in Observability
        </a>
      {/if}
    {:else}
      <p class="px-5 py-4 font-sans text-sm leading-5 text-muted">Nothing is asking for you right now.</p>
    {/if}
  </SettingsGroup>
</SettingsSection>
