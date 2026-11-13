<script lang="ts">
  import SettingsGroup from '@/components/settings/SettingsGroup.svelte'
  import SettingsSection from '@/components/settings/SettingsSection.svelte'
  import AlertRow from '@/components/observability/AlertRow.svelte'
  import { useAlerts } from '@/components/observability/alerts'
  import Chip from '@/components/ui/Chip.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'

  // WORST NEWS FIRST. The alerts the instance is raising about itself —
  // agents down or unhealthy, unroutable brains, MCP and retrieval planes,
  // the email outbox, unpriced usage — each a row that goes where it can be
  // fixed. `/api/alerts` filters to what this viewer may act on. This page
  // owns the list: Observability shows it as a count and a link.
  //
  // "All clear" is a CLAIM about the instance. It only comes from a 200 that
  // really carried no problems; a failed read says so instead.
  const alertsQuery = useAlerts()
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
        <AlertRow alert={a} />
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
