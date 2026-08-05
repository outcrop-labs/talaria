<script lang="ts">
  import { navigate } from '@/router'
  import Panel from '@/components/ui/Panel.svelte'
  import SectionHeader from '@/components/ui/SectionHeader.svelte'
  import SkeletonRows from '@/components/ui/SkeletonRows.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import { useContextMenu, copyAppLink } from '@/components/ui/context-menu.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useChannels, type Channel } from '@/lib/channels.svelte'
  import { relativeTime } from '@/lib/fleet'
  import AssistantBriefing from './AssistantBriefing.svelte'
  import ActivityList from './ActivityList.svelte'

  // Comms: what's unread, then the room's pulse.
  const menu = useContextMenu()
  // "All caught up." over a 500 — on the landing surface, about messages.
  const list = listQuery(useChannels(), { title: 'Could not load your channels', variant: 'compact' })
  const unread = $derived(list.rows.filter((c) => (c.unreadCount ?? 0) > 0))
  const label = (c: Channel) => (c.kind === 'dm' ? (c.peer?.name ?? c.peer?.email ?? 'DM') : `#${c.name}`)
</script>

<div class="space-y-6">
  <AssistantBriefing scope="comms" />
  <Panel>
    <SectionHeader title="Unread" action={unread.length > 0 ? String(unread.length).padStart(2, '0') : undefined} />
    {#if list.notice}
      <QueryError {...list.notice} />
    {/if}
    {#if list.pending}
      <SkeletonRows rows={4} />
    {:else if list.failed}
      <!-- the notice above already covers it -->
    {:else if unread.length === 0}
      <EmptyState variant="inline" title="All caught up." />
    {:else}
      <ul class="divide-y divide-line">
        {#each unread as c (c.id)}
          <li>
            <button
              type="button"
              onclick={() => void navigate('/comms', { search: { c: c.id } })}
              oncontextmenu={(e) =>
                menu.openMenu(e, [
                  { label: 'Open', onSelect: () => void navigate('/comms', { search: { c: c.id } }) },
                  { label: 'Copy link', onSelect: () => copyAppLink(`/comms?c=${c.id}`) },
                ])}
              class="flex w-full items-center gap-3 py-2.5 text-left transition-colors hover:bg-card2"
            >
              <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{label(c)}</span>
              <span class="shrink-0 font-mono text-[10px] font-medium tracking-[0.05em] text-accent">{c.unreadCount}</span>
              <span class="shrink-0 font-mono text-[11px] text-muted">{relativeTime(c.updatedAt)}</span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </Panel>
  <ActivityList kinds={['channel']} title="Channel activity" collapsible />
  <ContextMenu {menu} />
</div>
