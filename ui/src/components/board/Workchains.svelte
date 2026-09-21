<script lang="ts">
  // Workchains view — the board's fourth lens (TALA-30). Each chain renders
  // as a horizontal rail of compact step cards (WorkchainRail) with the
  // derived states legible at a glance; below the chains sits UNCHAINED —
  // the tickets no chain holds yet, each row offering to join one.
  //
  // Filtering follows the lens convention (list/gantt): the rails draw what
  // the board's filtered `tasks` contain — a step whose ticket is filtered
  // out is hidden, and chevrons connect what remains — EXCEPT archived
  // steps, which always render: they are chain structure. A rail is hidden
  // entirely only when it has steps and ALL of them are filtered out; an
  // empty chain stays (its picker is the only way to seed it).
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ChevronRight, Plus } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { errorMessage } from '@/lib/fetch-json'
  import { pushToast } from '@/lib/toast.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import Select from '@/components/ui/Select.svelte'
  import { prompt } from '@/components/ui/confirm.svelte'
  import { useAgents } from '@/lib/agents'
  import { statusColorOf, useBoardStatuses } from '@/lib/statuses'
  import { EFFORT_LABEL, type Task } from '@/lib/task-const'
  import type { Board, BoardMember } from '@/lib/boards.svelte'
  import WorkchainRail from './WorkchainRail.svelte'
  import { addWorkchainStep, createWorkchain, useBoardWorkchains } from '@/lib/workchain-client'
  import { chainedTaskIds, type WorkchainStep } from '@/lib/workchain-rules'

  let {
    board,
    tasks,
    allTasks,
    members = [],
    onOpen,
  }: {
    board: Board
    /** The board's filtered tickets — what this lens draws. */
    tasks: Task[]
    /** Unfiltered: the picker's pool. The pickable set drawn from it
     *  excludes archived and already-chained tickets (one chain per task). */
    allTasks: Task[]
    members?: BoardMember[]
    onOpen: (taskId: string) => void
  } = $props()

  const qc = useQueryClient()
  const canEdit = $derived(board.role === 'owner' || board.role === 'editor')
  const chainsList = listQuery(useBoardWorkchains(() => board.id), { title: 'Could not load this board’s workchains', variant: 'full' })
  const chains = $derived(chainsList.rows)
  const statusesQuery = useBoardStatuses(() => board.id)
  const boardStatuses = $derived(statusesQuery.data ?? [])
  const fleetQuery = useAgents()
  const agents = $derived(fleetQuery.data?.agents ?? [])

  const invalidate = () => qc.invalidateQueries({ queryKey: ['board-workchains', board.id] })
  const failure = (what: string) => (e: unknown) =>
    pushToast({ title: `${what} failed`, body: errorMessage(e), tone: 'danger' })

  const addChain = async () => {
    const name = await prompt({ title: 'New workchain', message: 'Name this chain of tickets.', confirmLabel: 'Create' })
    if (!name?.trim()) return
    await createWorkchain(board.id, name.trim()).then(invalidate).catch(failure('Creating the workchain'))
  }

  // ── What each rail and the Unchained section draw ─────────────────────────
  const filteredIds = $derived(new Set(tasks.map((t) => t.id)))
  const chained = $derived(chainedTaskIds(chains))

  /** A step renders when its ticket passes the filter — or it is archived:
   *  chain structure, drawn even with the archived chip off. */
  const stepShows = (s: WorkchainStep): boolean => s.state === 'archived' || filteredIds.has(s.taskId)
  /** A rail with steps hides only when the filter removed every one. */
  const railShows = (w: { steps: WorkchainStep[] }): boolean =>
    w.steps.length === 0 || w.steps.some(stepShows)

  const visibleChains = $derived(chains.filter(railShows))
  const unchained = $derived(tasks.filter((t) => !chained.has(t.id)))
  /** Pickable = unarchived and in no chain (the v1 invariant is board-wide). */
  const pickable = $derived(allTasks.filter((t) => !t.archivedAt && !chained.has(t.id)))

  // Unchained collapses once chains exist (it is a backlog, not the show);
  // with no chains it is the whole view, so it starts open.
  let unchainedOpen = $state(false)
  const anyChains = $derived(chains.length > 0)
  $effect(() => {
    unchainedOpen = !anyChains
  })

  const addTo = (chainId: string, taskId: string) =>
    void addWorkchainStep(chainId, taskId).then(invalidate).catch(failure('Adding the step'))
</script>

<div class="flex h-full flex-col">
  <div class="min-h-0 flex-1 space-y-6 overflow-y-auto p-4">
    {#if chainsList.notice}<QueryError {...chainsList.notice} class="mb-4" />{/if}

    {#if chainsList.pending}
      <!-- First load: rail-shaped skeletons — a header line and a row of
           compact cards — not the kanban columns or the list's table. -->
      {#each [0, 1] as r (r)}
        <div>
          <div class="flex items-center gap-2">
            <Skeleton class="h-4 w-32 rounded-full" />
            <Skeleton class="h-3 w-8 rounded-full" />
          </div>
          <div class="mt-2 flex gap-1.5">
            {#each [0, 1, 2, 3] as i (i)}
              <Skeleton class="h-20 w-52 shrink-0 rounded-lg" />
            {/each}
          </div>
        </div>
      {/each}
    {:else if chainsList.failed}
      <!-- The notice (rendered above) owns the whole region; an empty rail
           stack under a failed read would read as "and also no chains". -->
    {:else if chains.length === 0}
      {#if canEdit}
        <EmptyState
          icon="⭆"
          title="No workchains yet"
          hint="Chain tickets into a pipeline — steps hand off in order, and the chain shows where work is stuck."
        >
          {#snippet action()}
            <button
              type="button"
              class="flex items-center gap-1 rounded-md px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg"
              onclick={() => void addChain()}
            >
              <Plus size={12} /> Create a workchain
            </button>
          {/snippet}
        </EmptyState>
      {:else}
        <EmptyState
          icon="⭆"
          title="No workchains yet"
          hint="This board has no chains. An editor can create the first one."
        />
      {/if}
    {:else}
      {#each visibleChains as w (w.id)}
        <WorkchainRail workchain={w} {boardStatuses} {agents} {members} {pickable} {onOpen} onAdded={invalidate} />
      {/each}
      {#if canEdit}
        <button
          type="button"
          onclick={() => void addChain()}
          class="flex items-center gap-1 rounded-md px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg"
        >
          <Plus size={12} /> New workchain
        </button>
      {/if}
    {/if}

    {#if !chainsList.pending && !chainsList.failed && (unchained.length > 0 || !anyChains)}
      <!-- UNCHAINED — the tickets no chain holds. Collapsed by default once
           chains exist (a backlog, not the show); open when the board has no
           chains, because then it is the whole story. -->
      <div>
        <button
          type="button"
          onclick={() => (unchainedOpen = !unchainedOpen)}
          class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-muted transition-colors hover:text-fg"
        >
          <ChevronRight size={11} class={cn('shrink-0 transition-transform duration-150', unchainedOpen && 'rotate-90')} />
          Unchained
          <span class="tracking-[0.05em]">{unchained.length}</span>
        </button>
        {#if unchainedOpen}
          {#if unchained.length === 0}
            <p class="mt-2 pl-4 font-sans text-xs text-muted">Every ticket is chained.</p>
          {:else}
            <div class="mt-2 flex flex-col gap-1 pl-4">
              {#each unchained as t (t.id)}
                <div class="group flex items-center gap-2 rounded-md px-2 py-1 transition-colors hover:bg-raised">
                  <StatusDot color={statusColorOf(t.status, boardStatuses)} class="mt-0.5" />
                  {#if t.ticketRef}
                    <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{t.ticketRef}</span>
                  {/if}
                  <!-- svelte-ignore a11y_no_static_element_interactions -->
                  <span
                    role="button"
                    tabindex={0}
                    onclick={() => onOpen(t.id)}
                    onkeydown={(e) => {
                      if (e.key === 'Enter' && e.target === e.currentTarget) onOpen(t.id)
                    }}
                    class="min-w-0 flex-1 cursor-pointer truncate font-sans text-sm text-fg"
                  >
                    {t.title}
                  </span>
                  {#if t.effort}
                    <span class="shrink-0 rounded border border-line-subtle px-1 font-mono text-[9px] uppercase tracking-[0.05em] text-muted">
                      {EFFORT_LABEL[t.effort]}
                    </span>
                  {/if}
                  {#if canEdit && chains.length > 0}
                    {@const opts = chains}
                    {#if opts.length > 0}
                      <Select
                        size="sm"
                        class="h-6 shrink-0 rounded-md border border-line bg-panel px-1 font-mono text-[10px] text-muted opacity-0 transition-opacity focus:opacity-100 group-hover:opacity-100"
                        aria-label={`Add ${t.ticketRef ?? t.title} to a workchain`}
                        onchange={(e) => {
                          const v = e.currentTarget.value
                          e.currentTarget.value = ''
                          if (v) addTo(v, t.id)
                        }}
                      >
                        <option value="">+ chain…</option>
                        {#each opts as w (w.id)}
                          <option value={w.id}>{w.name}</option>
                        {/each}
                      </Select>
                    {/if}
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        {/if}
      </div>
    {/if}
  </div>
</div>
