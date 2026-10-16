<script lang="ts">
  import { navigate } from '@/router'
  import HomeListPanel from '@/components/app/HomeListPanel.svelte'
  import Chip from '@/components/ui/Chip.svelte'
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
  import { MODE_META, useResearchRuns } from '@/lib/research'
  import { RUN_DOT } from './home'

  // Sessions: the work you have open. Work sessions, plans and research runs
  // are ONE surface now (/work), so they are one panel here rather than the
  // two this tab replaced — a person scanning Home wants "what am I in the
  // middle of", which was never a question about which of three views held it.
  // Archived sessions live in the harness's own rail; Home is the live preview.
  const menu = useContextMenu()
  const qc = useQueryClient()
  const refresh = () => void qc.invalidateQueries({ queryKey: ['conversations'] })
  // The original "No boards yet over a 500", verbatim, on Home: a failed read
  // renders "No sessions yet." to someone whose sessions exist.
  const list = listQuery(useConversations('sessions'), {
    title: 'Could not load your sessions',
    variant: 'compact',
  })
  const fleetQuery = useAgents()
  const agentLabel = (id: string) => fleetQuery.data?.agents.find((a) => a.id === id)?.label ?? id
  // WHICH ROWS ARE RESEARCH RUNS. The merged list carries no kind on the wire,
  // and the runs list answers it by naming each run's conversation — so a
  // research-origin row can wear its depth and the run's own state, which is
  // the thing that moves while nobody is looking at it.
  const runsQuery = useResearchRuns()
  const runBySession = $derived(
    new Map((runsQuery.data ?? []).filter((r) => r.conversationId).map((r) => [r.conversationId!, r])),
  )
  const open = (c: Conversation) => void navigate('/work/:sessionId', { params: { sessionId: c.id } })
  const rename = (c: Conversation) => {
    void renameConversation(c.id, c.title, 'session').then((ok) => {
      if (ok) refresh()
    })
  }
  const archive = (c: Conversation) => {
    void archiveConversation(c.id).then((ok) => {
      if (ok) refresh()
    })
  }
  const remove = (c: Conversation) => {
    void deleteConversation(c.id, c.title, 'session').then((ok) => {
      if (ok) refresh()
    })
  }
</script>

<HomeListPanel
  title="Sessions"
  {menu}
  notice={list.notice}
  pending={list.pending}
  failed={list.failed}
  isEmpty={list.rows.length === 0}
  skeletonRows={5}
>
  {#snippet empty()}
    <EmptyState variant="inline" title="No sessions yet. Start one on /work." />
  {/snippet}
  {#each list.rows.slice(0, 10) as c (c.id)}
    {@const run = runBySession.get(c.id)}
    <li>
      <button
        type="button"
        onclick={() => open(c)}
        oncontextmenu={(e) =>
          menu.openMenu(
            e,
            planRowMenu(c, {
              path: `/work/${c.id}`,
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
        <!-- A run's own status outranks the conversation's: "the agent is
             typing" is a smaller fact than "this report is parked on a
             question". `awaiting` shares running's accent without its pulse. -->
        {#if run}
          <StatusDot status={RUN_DOT[run.status] ?? 'idle'} pulse={run.status === 'running'} />
        {:else if c.working}
          <StatusDot status="accent" pulse />
        {/if}
        <span class="min-w-0 flex-1 truncate font-sans text-sm text-fg">{c.title || 'Untitled session'}</span>
        {#if run}<Chip>{MODE_META[run.mode].label}</Chip>{/if}
        {#if run?.status === 'error'}
          <span class="shrink-0 font-sans text-xs text-danger">failed: open to re-run →</span>
        {:else if c.failed}
          <span class="shrink-0 font-sans text-xs text-danger">failed: open to retry →</span>
        {/if}
        {#if c.role === 'collaborator' && c.ownerLabel}
          <span class="shrink-0 font-sans text-[11px] text-muted">shared by {c.ownerLabel}</span>
        {/if}
        <span class="shrink-0 font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
          {agentLabel(c.agentModel)}
        </span>
        <span class="shrink-0 font-mono text-[11px] text-muted">{relativeTime(c.updatedAt)}</span>
      </button>
    </li>
  {/each}
</HomeListPanel>
