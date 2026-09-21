<script lang="ts">
  import { navigate } from '@/router'
  import HomeListPanel from '@/components/app/HomeListPanel.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import { useContextMenu, openCopyItems } from '@/components/ui/context-menu.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useChannels, type Channel } from '@/lib/channels.svelte'
  import { relativeTime } from '@/lib/fleet'
  import ActivityList from './ActivityList.svelte'

  // Comms: what's unread, then the room's pulse.
  const menu = useContextMenu()
  // "All caught up." over a 500 — on the landing surface, about messages.
  const list = listQuery(useChannels(), { title: 'Could not load your channels', variant: 'compact' })
  const unread = $derived(list.rows.filter((c) => (c.unreadCount ?? 0) > 0))
  const label = (c: Channel) => (c.kind === 'dm' ? (c.peer?.name ?? c.peer?.email ?? 'DM') : `#${c.name}`)
</script>

<HomeListPanel
  title="Unread"
  action={unread.length > 0 ? String(unread.length).padStart(2, '0') : undefined}
  menu={menu}
  notice={list.notice}
  pending={list.pending}
  failed={list.failed}
  isEmpty={unread.length === 0}
  skeletonRows={4}
>
  {#snippet empty()}
    <EmptyState variant="inline" title="All caught up." />
  {/snippet}
  {#each unread as c (c.id)}
    <li>
      <button
        type="button"
        onclick={() => void navigate('/comms/channel/:id', { params: { id: c.id } })}
        oncontextmenu={(e) =>
          menu.openMenu(e, openCopyItems(`/comms/channel/${c.id}`, () => void navigate('/comms/channel/:id', { params: { id: c.id } })))}
        class="flex w-full items-center gap-3 py-2.5 text-left transition-colors hover:bg-card2"
      >
        <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{label(c)}</span>
        <span class="shrink-0 font-mono text-[10px] font-medium tracking-[0.05em] text-accent">{c.unreadCount}</span>
        <span class="shrink-0 font-mono text-[11px] text-muted">{relativeTime(c.updatedAt)}</span>
      </button>
    </li>
  {/each}
  {#snippet after()}
    <ActivityList kinds={['channel']} title="Channel activity" collapsible />
  {/snippet}
</HomeListPanel>