<script lang="ts">
  import { ChevronDown, ChevronRight } from '@lucide/svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { cn } from '@/lib/cn'
  import { fade, fly, listStagger, PANEL, QUICK } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'
  import { useMarkNotificationsRead, useNotifications, type Notification } from '@/lib/notifications'
  import { navigateHref } from '@/router'

  // The notifications half of the Inbox surface: mentions (and more kinds as
  // they land), newest first, mark-read on open. Lives at the top of `/` now —
  // the old standalone /inbox page redirects here.
  const query = useNotifications()
  const markRead = useMarkNotificationsRead()

  // LIVE, NOT JUST POLLED. `addNotification` publishes the row's id to this
  // person's firehose the moment it lands; the 30s poll in useNotifications
  // stays as the floor for a dropped SSE connection or a sleeping laptop.
  // Same shape as useBriefLive: the event carries no content — it invalidates,
  // and the ordinary route re-reads with the ordinary read path.
  const qc = useQueryClient()
  $effect(() => {
    const es = new EventSource('/api/me/events')
    es.onmessage = (event: MessageEvent<string>) => {
      try {
        if ((JSON.parse(event.data) as { type?: string }).type !== 'notification') return
      } catch {
        return
      }
      void qc.invalidateQueries({ queryKey: ['notifications'] })
    }
    return () => es.close()
  })
  const items = $derived(query.data?.notifications ?? [])
  const unread = $derived(query.data?.unread ?? 0)
  // Collapsed by default: the briefing above is the working surface — this is
  // the raw feed, one click away, with the unread count doing the talking.
  let expanded = $state(false)

  const open = (n: Notification) => {
    void markRead([n.id])
    // n.href is a server-built path, not a compile-time route literal.
    if (n.href) void navigateHref(n.href)
  }
</script>

<!-- A rejected read leaves `data` undefined FOR EVER, and `!data` was the only
     thing standing in for it — so a 500 on /api/notifications shimmered a
     skeleton at the top of the Inbox permanently, with no error text and no
     way to retry. Broke, loading, and resolved-empty are three answers. -->
{#if query.isError && query.data === undefined}
  <Panel>
    <!-- Inline density on purpose: collapsed, this panel is one row tall,
         and a full-height error block at the top of Home would shove the
         briefing down further than the bug ever did. -->
    <QueryError
      variant="inline"
      title="Could not load notifications"
      error={query.error}
      onRetry={() => void query.refetch()}
    />
  </Panel>
{:else if !query.data}
  <!-- In flight → hold a modest space at the top of the page (this panel leads
       the column, so popping in late shoves EVERYTHING down). Resolved empty →
       nothing: a quiet inbox takes no space. The sketch mirrors the real
       anatomy — collapsed header line, then two notification-row silhouettes
       (unread treatment: dot + title + time in a raised bordered row). No
       Materialize here: loaded, the rows sit behind the collapsed header, so
       there is no visible list for the skeletons to materialize into. -->
  <Panel aria-hidden="true">
    <div class="flex min-h-6 items-center gap-2">
      <!-- Fixed geometry, so nothing about it is uncertain — a flat rail, not a skeleton (UI-CONVENTIONS, Loading). -->
      <div class="h-3 w-3 rounded bg-line"></div>
      <Skeleton class="h-2.5 w-24 rounded-full" />
    </div>
    <div class="mt-2 space-y-1">
      {#each [0, 1] as i (i)}
        <div class="rounded-md border border-line bg-raised px-3 py-2.5">
          <div class="flex h-5 items-center gap-2">
            <div class="h-1.5 w-1.5 shrink-0 rounded-full bg-line"></div>
            <Skeleton class={`h-3 rounded-full ${i ? 'w-2/5' : 'w-3/5'}`} />
            <span class="ml-auto"><Skeleton class="h-2.5 w-10 rounded-full" /></span>
          </div>
        </div>
      {/each}
    </div>
  </Panel>
{:else if items.length > 0}
  <Panel>
    <div class={expanded ? 'mb-2 flex min-h-6 items-center gap-3' : 'flex min-h-6 items-center gap-3'}>
      <button type="button" onclick={() => (expanded = !expanded)} class="group flex min-w-0 items-center gap-2 text-left">
        {#if expanded}
          <ChevronDown size={12} class="shrink-0 text-muted" />
        {:else}
          <ChevronRight size={12} class="shrink-0 text-muted" />
        {/if}
        <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim transition-colors group-hover:text-muted">
          Notifications
        </span>
        {#if unread > 0}
          <span class="font-mono text-[10px] font-medium tracking-[0.05em] text-accent">{unread}</span>
        {/if}
        {#if unread === 0}
          <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">all read</span>
        {/if}
      </button>
      {#if expanded && unread > 0}
        <Button variant="outline" size="sm" class="ml-auto" onclick={() => void markRead()}>
          Mark all read
        </Button>
      {/if}
    </div>
    <!-- Polled every 30s: a failed refresh keeps the last good feed on screen
         (stale beats blank) but must not pass an old unread count off as
         current. -->
    {#if query.isError}
      <QueryError
        variant="inline"
        class="mt-2"
        title="Notifications may be out of date"
        error={query.error}
        onRetry={() => void query.refetch()}
      />
    {/if}
    {#if expanded}
      <ul in:fly={PANEL} out:fade={QUICK} class="max-h-80 space-y-1 overflow-y-auto" use:listStagger>
        {#each items as n (n.id)}
          <li in:fade={{ duration: 150 }} out:fade={QUICK}>
            <button
              type="button"
              onclick={() => open(n)}
              class={cn(
                'w-full rounded-md border border-transparent px-3 py-2.5 text-left transition-colors hover:bg-card2',
                !n.readAt && 'border-line bg-raised',
              )}
            >
              <div class="flex items-baseline gap-2">
                {#if !n.readAt}<span class="h-1.5 w-1.5 shrink-0 rounded-full bg-accent"></span>{/if}
                <span class={cn('min-w-0 flex-1 truncate font-sans text-sm', n.readAt ? 'text-muted' : 'font-medium text-fg')}>
                  {n.title}
                </span>
                <span class="shrink-0 font-mono text-[11px] text-muted">{relativeTime(n.createdAt)}</span>
              </div>
              {#if n.body}<div class="mt-0.5 truncate pl-3.5 font-sans text-xs text-muted">{n.body}</div>{/if}
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </Panel>
{/if}
