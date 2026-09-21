<script lang="ts">
  import { navigate } from '@/router'
  import HomeListPanel from '@/components/app/HomeListPanel.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { useContextMenu, openCopyItems } from '@/components/ui/context-menu.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { useConversations } from '@/lib/conversations.svelte'
  import { useAgents } from '@/lib/agents'
  import { relativeTime } from '@/lib/fleet'

  // Plans: your live plans (and ones shared with you).
  const menu = useContextMenu()
  // The original "No boards yet over a 500", verbatim, on Home: a failed read
  // renders "No plans yet. Start one on /plan." to an owner whose plans exist.
  const list = listQuery(useConversations('plan'), { title: 'Could not load your plans', variant: 'compact' })
  const fleetQuery = useAgents()
  const agentLabel = (id: string) => fleetQuery.data?.agents.find((a) => a.id === id)?.label ?? id
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
          menu.openMenu(e, openCopyItems(`/plan/${c.id}`, () => void navigate('/plan/:planId', { params: { planId: c.id } })))}
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