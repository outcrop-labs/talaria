<script lang="ts">
  import { createQuery } from '@tanstack/svelte-query'
  import { ChevronDown, ChevronRight } from '@lucide/svelte'
  import { navigateHref } from '@/router'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import ActivityRow from '@/components/app/ActivityRow.svelte'
  import { getList } from '@/lib/fetch-json'
  import { cn } from '@/lib/cn'
  import type { FeedEvent } from './home'

  // Shared: an area-scoped slice of the workspace activity feed.
  let {
    kinds,
    title = 'Recent activity',
    collapsible = false,
  }: { kinds: string[]; title?: string; collapsible?: boolean } = $props()

  let expanded = $state(!collapsible)
  const query = createQuery(() => ({
    queryKey: ['home-activity', kinds.join(',')],
    queryFn: (): Promise<FeedEvent[]> => getList<FeedEvent>(`/api/activity?kinds=${kinds.join(',')}`, 'events'),
    refetchInterval: 30_000,
  }))
</script>

<Panel>
  {#if collapsible}
    <button
      type="button"
      onclick={() => (expanded = !expanded)}
      class={cn('group flex min-h-6 items-center gap-2 text-left', expanded && 'mb-3')}
    >
      {#if expanded}<ChevronDown size={12} class="text-muted" />{:else}<ChevronRight size={12} class="text-muted" />{/if}
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim transition-colors group-hover:text-muted">
        {title}
      </span>
    </button>
  {:else}
    <SectionHeader {title} />
  {/if}
  {#if !expanded}
    <!-- collapsed: nothing -->
  {:else if query.isLoading}
    <SkeletonRows rows={6} />
  {:else if query.data === undefined}
    <!-- No data and not loading = the read broke. "Quiet so far." is a claim
         about the feed, and a failed feed cannot make it. -->
    <QueryError variant="inline" error={query.error} title="Could not load activity" onRetry={() => void query.refetch()} />
  {:else if query.data.length === 0}
    <EmptyState variant="inline" title="Quiet so far." />
  {:else}
    <ul>
      {#each query.data.slice(0, 12) as a, i (i)}
        <ActivityRow actor={a.actor} detail={a.detail} at={a.at} context={a.context} onClick={() => a.href && void navigateHref(a.href)} />
      {/each}
    </ul>
  {/if}
</Panel>
