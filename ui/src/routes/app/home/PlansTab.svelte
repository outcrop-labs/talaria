<script lang="ts">
  import { navigate } from '@/router'
  import HomeListPanel from '@/components/app/HomeListPanel.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { useContextMenu } from '@/components/ui/context-menu.svelte'
  import { planRowMenu } from '@/components/record-menus'
  import { listQuery } from '@/components/ui/query-state'
  import { useQueryClient } from '@tanstack/svelte-query'
  import {
    archiveConversation,
    deleteConversation,
    renameConversation,
    useConversations,
    type Conversation,
  } from '@/lib/conversations.svelte'
  import { useAgents } from '@/lib/agents'
  import { relativeTime } from '@/lib/fleet'

  // Plans: your live plans (and ones shared with you). Archived plans live
  // on /plan, in the rail's Archived section — Home is the live preview.
  const menu = useContextMenu()
  const qc = useQueryClient()
  const refresh = () => void qc.invalidateQueries({ queryKey: ['conversations'] })
  // The original "No boards yet over a 500", verbatim, on Home: a failed read
  // renders "No plans yet. Start one on /plan." to an owner whose plans exist.
  const list = listQuery(useConversations('plan'), { title: 'Could not load your plans', variant: 'compact' })
  const fleetQuery = useAgents()
  const agentLabel = (id: string) => fleetQuery.data?.agents.find((a) => a.id === id)?.label ?? id
  const open = (c: Conversation) => void navigate('/plan/:planId', { params: { planId: c.id } })
  const rename = (c: Conversation) => {
    void renameConversation(c.id, c.title).then((ok) => {
      if (ok) refresh()
    })
  }
  const archive = (c: Conversation) => {
    void archiveConversation(c.id).then((ok) => {
      if (ok) refresh()
    })
  }
  const remove = (c: Conversation) => {
    void deleteConversation(c.id, c.title).then((ok) => {
      if (ok) refresh()
    })
  }
</script>

<HomeListPanel
  title="Plans"
  menu={menu}
  notice={list.notice}
  pending={list.pending}
  failed={list.failed}
  isEmpty={list.rows.length === 0}
  skeletonRows={5}
>
  {#snippet empty()}
    <EmptyState variant="inline" title="No plans yet. Start one on /plan." />
  {/snippet}
  {#each list.rows.slice(0, 10) as c (c.id)}
    <li>
      <button
        type="button"
        onclick={() => void navigate('/plan/:planId', { params: { planId: c.id } })}
        oncontextmenu={(e) =>
          menu.openMenu(
            e,
            planRowMenu(c, {
              path: `/plan/${c.id}`,
              open: () => open(c),
              archived: false,
              onRename: () => rename(c),
              onArchive: () => archive(c),
              onRestore: () => {},
              onDelete: () => remove(c),
            }),
          )}
        class="flex w-full items-center gap-3 py-2.5 text-left transition-colors hover:bg-card2"
      >
        {#if c.working}<StatusDot status="accent" pulse />{/if}
        <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{c.title || 'Untitled plan'}</span>
        {#if c.failed}<span class="shrink-0 font-sans text-xs text-danger">failed: open to retry →</span>{/if}
        {#if c.role === 'collaborator' && c.ownerLabel}
          <span class="shrink-0 font-sans text-[11px] text-muted">shared by {c.ownerLabel}</span>
        {/if}
        <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">{agentLabel(c.agentModel)}</span>
        <span class="shrink-0 font-mono text-[11px] text-muted">{relativeTime(c.updatedAt)}</span>
      </button>
    </li>
  {/each}
</HomeListPanel>
