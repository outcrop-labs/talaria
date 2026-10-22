<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { navigateHref } from '@/router'
  import ActivityRow from '@/components/app/ActivityRow.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { getList } from '@/lib/fetch-json'

  // What changed lately: the audit trail's newest rows, the same read (and
  // cache key) the Observability overview and Home use.
  const auditQuery = createQuery(() => ({
    queryKey: ['home-activity', ''],
    queryFn: () =>
      getList<{ at: string; actor: string; detail: string; context: string; href: string }>('/api/activity', 'events'),
    refetchInterval: 30_000,
  }))
  const audit = $derived(auditQuery.data)
</script>

<Panel class="px-4 py-3">
  <div class="mb-2 flex min-h-6 items-center">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Recent changes</span>
  </div>
  {#if auditQuery.isLoading}
    <SkeletonRows rows={5} />
  {:else if audit === undefined}
    <!-- "Quiet so far." is a fact about the log, not about a failed request. -->
    <QueryError
      variant="inline"
      error={auditQuery.error}
      title="Could not load recent changes"
      onRetry={() => void auditQuery.refetch()}
    />
  {:else if audit.length === 0}
    <p class="font-sans text-xs text-muted">Quiet so far.</p>
  {:else}
    <ul>
      {#each audit.slice(0, 7) as a, i (i)}
        <ActivityRow actor={a.actor} detail={a.detail} at={a.at} onClick={() => a.href && void navigateHref(a.href)} />
      {/each}
    </ul>
  {/if}
</Panel>
