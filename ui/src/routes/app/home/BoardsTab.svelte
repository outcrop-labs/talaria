<script lang="ts">
  import { navigate } from '@/router'
  import Materialize from '@/components/ui/Materialize.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu, copyAppLink, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { cn } from '@/lib/cn'
  import ActivityList from './ActivityList.svelte'
  import { QUEUE_META, type HomeQuery, type QueueKey } from './home'

  // Boards: board-scoped briefing, full queues behind a sidebar, audit trail.
  let { home }: { home: HomeQuery } = $props()

  let queue = $state<QueueKey>('triage')
  const menu = useContextMenu()
  const meta = $derived(QUEUE_META[queue])
  const items = $derived(home.data?.queues[queue].items ?? [])
  const queueKeys = Object.keys(QUEUE_META) as QueueKey[]
</script>

<div class="space-y-6">
  <div class="grid gap-6 lg:grid-cols-[15rem_minmax(0,1fr)]">
    <aside class="space-y-1">
      {#each queueKeys as k (k)}
        <button
          type="button"
          onclick={() => (queue = k)}
          class={cn(
            'flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-left font-sans text-sm transition-colors',
            queue === k ? 'bg-raised text-fg' : 'text-muted hover:bg-card2 hover:text-fg',
          )}
        >
          <StatusDot status={QUEUE_META[k].dot} />
          <span class="min-w-0 flex-1 truncate">{QUEUE_META[k].label}</span>
          {#if home.isLoading}
            <Skeleton class="h-3 w-6 rounded-full" />
          {:else}
            <!-- A failed /api/home must not read as "0 waiting for you". -->
            <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">
              {home.data ? home.data.queues[k].count : '—'}
            </span>
          {/if}
        </button>
      {/each}
    </aside>
    <Panel>
      <SectionHeader
        title={meta.label}
        action={home.data ? String(home.data.queues[queue].count).padStart(2, '0') + ' open' : undefined}
        class="mb-1"
      />
      <!-- Same rule as the counts in the rail: with no payload the header
           says nothing rather than "00 open" over a failed read. -->
      <div class="mb-3 font-sans text-xs text-muted">{meta.hint}</div>
      <!-- Skeleton → content as one motion: queue-row-shaped skeletons
           (ticket ref + title + board/time meta) materialize into the rows.
           Materialize direct — this tab keys error/empty off `home` itself. -->
      <Materialize loading={home.isLoading} count={6} class="divide-y divide-line">
        {#snippet skeleton(i)}
          <div aria-hidden="true" class="flex items-center gap-3 py-2.5">
            <Skeleton class="h-3 w-14 shrink-0 rounded" />
            <div class="flex h-5 min-w-0 flex-1 items-center">
              <Skeleton class={`h-3.5 rounded-full ${['w-3/5', 'w-2/5', 'w-1/2', 'w-2/3'][i % 4]}`} />
            </div>
            <Skeleton class="h-3 w-16 shrink-0 rounded-full" />
            <Skeleton class="h-3 w-10 shrink-0 rounded" />
          </div>
        {/snippet}
        {#if !home.data}
          <!-- "All clear." is a statement about the queue. A broken /api/home
               has no idea whether it's clear, so it must not say so. -->
          <QueryError
            variant="compact"
            error={home.error}
            title="Could not load your queues"
            onRetry={() => void home.refetch()}
          />
        {:else if items.length === 0}
          <EmptyState variant="compact" title="All clear." />
        {:else}
          {#each items as w (w.id)}
            <button
              type="button"
              onclick={() => void navigate('/boards/:boardId/:taskId', { params: { boardId: w.boardId, taskId: w.id } })}
              oncontextmenu={(e) =>
                menu.openMenu(e, [
                  { label: 'Open', onSelect: () => void navigate('/boards/:boardId/:taskId', { params: { boardId: w.boardId, taskId: w.id } }) },
                  { label: 'Copy link', onSelect: () => copyAppLink(`/boards/${w.boardId}/${w.id}`) },
                  ...(w.ticketRef
                    ? ([
                        { label: 'Copy ticket ref', onSelect: () => void navigator.clipboard.writeText(w.ticketRef!) },
                      ] as ContextMenuEntry[])
                    : []),
                ])}
              class="flex w-full items-center gap-3 py-2.5 text-left transition-colors hover:bg-card2"
            >
              {#if w.ticketRef}<span class="shrink-0 font-mono text-[11px] text-muted">{w.ticketRef}</span>{/if}
              <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{w.title}</span>
              <span class="shrink-0 font-sans text-xs text-muted">{w.board}</span>
              <span class="shrink-0 font-mono text-[11px] text-muted">{relativeTime(w.updatedAt)}</span>
            </button>
          {/each}
        {/if}
      </Materialize>
    </Panel>
  </div>
  <ActivityList kinds={['ticket']} title="Board activity" collapsible />
  <ContextMenu {menu} />
</div>
