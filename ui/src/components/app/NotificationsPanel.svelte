<script lang="ts">
  import { ChevronDown, ChevronRight } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Panel from '@/components/ui/Panel.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import { cn } from '@/lib/cn'
  import { fly, PANEL } from '@/lib/motion'
  import { relativeTime } from '@/lib/fleet'
  import { useMarkNotificationsRead, useNotifications, type Notification } from '@/lib/notifications'
  import { navigateHref } from '@/router'

  // The notifications half of the Inbox surface: mentions (and more kinds as
  // they land), newest first, mark-read on open. Lives at the top of `/` now —
  // the old standalone /inbox page redirects here.
  const query = useNotifications()
  const markRead = useMarkNotificationsRead()
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
       nothing: a quiet inbox takes no space. -->
  <Panel>
    <Skeleton class="mb-3 h-3 w-28 rounded-full" />
    <SkeletonRows rows={2} />
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
      <ul in:fly={PANEL} class="max-h-80 space-y-1 overflow-y-auto">
        {#each items as n (n.id)}
          <li>
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
