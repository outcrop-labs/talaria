<script lang="ts">
  // Workchains view — the board's fourth lens (TALA-30; TALA-35 the graph,
  // TALA-34 the wiring editor). ONE chain fills the view, and the page is split
  // in two: a pop-out TICKET PANEL on the left (the board's unchained tickets
  // and the board's own filters — WorkchainSidebar) and the chain's canvas
  // filling everything right of it.
  //
  // The toolbar above the canvas is the CHAIN's, and only the chain's. The
  // board's search and facet pills move into the panel while this lens is up
  // (Board.svelte hides its query row and hands the state down), because they
  // filter the ticket list, not the graph — and the row they used to sit in is
  // the only horizontal space the workchain's own verbs have.
  //
  // The workchain menu is the one home for everything that acts on a chain:
  // switch, create, rename, pause, straighten, copy link, delete. The focused
  // chain lives in the URL (`?chain=`), so a chain is a thing you can send
  // someone — the lens convention every other selection here already follows.
  //
  // Which chains the menu offers follows the list/gantt rule: the chains whose
  // steps the board's filtered `tasks` still contain — EXCEPT archived steps,
  // which always count: they are chain structure. A chain leaves the menu only
  // when it has steps and ALL of them are filtered out; an empty chain stays.
  // The canvas itself draws the whole chain, filter or not.
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ChevronDown, PanelLeftOpen, Plus, Workflow } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { pushToast, toastError } from '@/lib/toast.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { confirm, confirmDelete, prompt } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { useAgents } from '@/lib/agents'
  import { copyAppLink } from '@/lib/links'
  import { useBoardStatuses } from '@/lib/statuses'
  import { isClosedStatus } from './field-pills'
  import type { Task } from '@/lib/task-const'
  import type { Board, BoardLabel, BoardMember } from '@/lib/boards.svelte'
  import type { BoardFilters } from './filter-bar'
  import { spliceIntoWire } from '@/lib/wiring-canvas'
  import WorkchainCanvas from './WorkchainCanvas.svelte'
  import WorkchainSidebar from './WorkchainSidebar.svelte'
  import {
    addWorkchainEdge,
    addWorkchainStep,
    createWorkchain,
    deleteWorkchain,
    removeWorkchainEdge,
    updateWorkchain,
    useBoardWorkchains,
  } from '@/lib/workchain-client'
  import {
    buildPositions,
    chainIsLinear,
    chainProgress,
    chainCandidates,
    chainedTaskIds,
    pickFocusedChain,
    stepReadOrder,
    type Workchain,
    type WorkchainStep,
  } from '@/lib/workchain-rules'

  let {
    board,
    tasks,
    members = [],
    onOpen,
    chainId = null,
    onChain,
    filterCtx,
  }: {
    board: Board
    /** The board's filtered tickets — what the ticket panel offers. */
    tasks: Task[]
    members?: BoardMember[]
    onOpen: (taskId: string) => void
    /** The focused chain, from the URL — a chain is shareable. */
    chainId?: string | null
    onChain: (id: string | null) => void
    /** The board's own query state, which this lens hosts in its panel while
     *  it is up. The route still owns the encoding; this is the handoff. */
    filterCtx: {
      q: string
      onQ: (v: string) => void
      filters: BoardFilters
      onFilters: (f: BoardFilters) => void
      agents: Array<{ id: string; label: string }>
      labels: BoardLabel[]
      meId?: string | null
      showArchived: boolean
      onArchived: (v: boolean) => void
    }
  } = $props()

  const qc = useQueryClient()
  const canEdit = $derived(board.role === 'owner' || board.role === 'editor')
  const chainsList = listQuery(useBoardWorkchains(() => board.id), { title: 'Could not load this board’s workchains', variant: 'full' })
  const chains = $derived(chainsList.rows)
  const statusesQuery = useBoardStatuses(() => board.id)
  const boardStatuses = $derived(statusesQuery.data ?? [])
  const fleetQuery = useAgents()
  const fleetAgents = $derived(fleetQuery.data?.agents ?? [])

  const invalidate = () => qc.invalidateQueries({ queryKey: ['board-workchains', board.id] })
  const failure = (what: string) => (e: unknown) =>
    toastError(`${what} failed`, e)

  /** The canvas's imperative handle: after a ticket joins, show the reader
   *  where it landed. */
  let canvas = $state<WorkchainCanvas | null>(null)

  // The panel is the lens's default posture — this view exists to move tickets
  // into a chain, and a hidden drag source is no source at all. Hiding it is
  // per-board and remembered, the same way the lens itself is.
  const panelKey = $derived(`talaria:workchain-panel:${board.id}`)
  let sidebarOpen = $state(true)
  $effect(() => {
    const key = panelKey
    try {
      sidebarOpen = localStorage.getItem(key) !== 'closed'
    } catch {
      sidebarOpen = true
    }
  })
  const setSidebar = (open: boolean) => {
    sidebarOpen = open
    try {
      localStorage.setItem(panelKey, open ? 'open' : 'closed')
    } catch {
      /* private window: the panel just forgets */
    }
  }

  /** Create, and FOCUS it: naming a chain is asking to work on it. */
  const addChain = async () => {
    const name = await prompt({ title: 'New workchain', message: 'Name this chain of tickets.', confirmLabel: 'Create' })
    if (!name?.trim()) return
    await createWorkchain(board.id, name.trim())
      .then(({ workchain }) => {
        onChain(workchain.id)
        invalidate()
      })
      .catch(failure('Creating the workchain'))
  }

  // ── What the menu offers and the panel draws ──────────────────────────────
  const filteredIds = $derived(new Set(tasks.map((t) => t.id)))
  const chained = $derived(chainedTaskIds(chains))

  /** A step counts when its ticket passes the filter — or it is archived:
   *  chain structure, kept even with the archived chip off. */
  const stepShows = (s: WorkchainStep): boolean => s.state === 'archived' || filteredIds.has(s.taskId)
  const chainShows = (w: { steps: WorkchainStep[] }): boolean =>
    w.steps.length === 0 || w.steps.some(stepShows)

  const visibleChains = $derived(chains.filter(chainShows))
  /** The panel's rows: unchained AND unfinished. `isClosedStatus` is the
   *  board's own one closed predicate, so the panel cannot disagree with the
   *  pills about what done means. */
  const isClosed = (status: string) => isClosedStatus(status, boardStatuses)
  const unchained = $derived(chainCandidates(tasks, chained, isClosed))
  const chainedShown = $derived(tasks.filter((t) => chained.has(t.id)).length)
  /** Tickets the panel is holding back because they are finished — the empty
   *  state says so rather than reading as "this board has nothing". */
  const doneHidden = $derived(tasks.filter((t) => !chained.has(t.id) && isClosed(t.status)).length)

  // ── Which chain fills the view ────────────────────────────────────────────
  const focused = $derived(pickFocusedChain(chains, chainId))
  const focusedChain = $derived(chains.find((c) => c.id === focused) ?? null)
  const menuPos = $derived(visibleChains.findIndex((w) => w.id === focused) + 1)

  /** The one menu. Every verb that acts on a chain hangs off the control that
   *  names it — switching included, so there is never a second place to look. */
  const chainMenuItems = $derived.by(() => {
    const items: ContextMenuEntry[] = visibleChains.map((w) => ({
      // The progress read rides the label: switching chains is a decision, and
      // "3/7" is the fact it turns on.
      label: `${w.name}   ${chainProgress(w)}${w.paused ? '  ·  paused' : ''}`,
      checked: w.id === focused,
      onSelect: () => onChain(w.id),
    }))
    if (visibleChains.length === 0) {
      items.push({ label: 'No workchains match the filters', disabled: true })
    }
    items.push('sep', {
      label: 'New workchain',
      disabled: !canEdit,
      onSelect: () => void addChain(),
    })
    const c = focusedChain
    if (!c) return items
    items.push({ label: 'Copy link to this workchain', onSelect: () => copyChainLink(c) })
    if (!canEdit) return items
    items.push(
      { label: 'Rename workchain', onSelect: () => void promptRenameFocused() },
      { label: c.paused ? 'Resume chain' : 'Pause chain', onSelect: () => togglePaused() },
      {
        // The `positions` verb, which the api answers by rewriting the graph to
        // ONE line through the order sent. It was the rail's reorder and became
        // unreachable when the rail left the lens; it is the only way back from
        // a tangle to a pipeline, so it belongs on the chain's menu.
        label: 'Straighten into a single line',
        disabled: c.steps.length < 2 || chainIsLinear(c),
        onSelect: () => void straightenFocused(),
      },
      'sep',
      { label: 'Delete workchain', danger: true, onSelect: () => void deleteFocused() },
    )
    return items
  })

  const copyChainLink = (c: Workchain) => {
    copyAppLink(`/boards/${board.id}?view=workchains&chain=${c.id}`)
    pushToast({ title: 'Link copied', body: `It opens this board on “${c.name}”.`, tone: 'success' })
  }

  const togglePaused = () => {
    const c = focusedChain
    if (!c) return
    void updateWorkchain(c.id, { paused: !c.paused })
      .then(invalidate)
      .catch(failure(c.paused ? 'Resuming the chain' : 'Pausing the chain'))
  }

  const promptRenameFocused = async () => {
    const c = focusedChain
    if (!c) return
    const name = await prompt({
      title: 'Rename workchain',
      message: `Rename “${c.name}”.`,
      confirmLabel: 'Rename',
      defaultValue: c.name,
    })
    const trimmed = name?.trim()
    if (!trimmed || trimmed === c.name) return
    void updateWorkchain(c.id, { name: trimmed }).then(invalidate).catch(failure('Renaming the workchain'))
  }

  /** Collapse the graph to one line through the chain's read order. The api
   *  rewrites every edge, so it is a real loss — the confirm says which. */
  const straightenFocused = async () => {
    const c = focusedChain
    if (!c || c.steps.length < 2) return
    const order = stepReadOrder(c)
    if (
      !(await confirm({
        title: `Straighten “${c.name}”?`,
        message: `Every wire is redrawn as one line through the chain's current order — ${c.edges.length} wire(s) replaced by ${order.length - 1}. The tickets are untouched.`,
        confirmLabel: 'Straighten',
      }))
    )
      return
    void updateWorkchain(c.id, { positions: buildPositions(order) })
      .then(invalidate)
      .catch(failure('Straightening the workchain'))
  }

  const deleteFocused = async () => {
    const c = focusedChain
    if (!c) return
    if (
      !(await confirmDelete({
        what: 'workchain',
        name: c.name,
        detail: 'Its tickets stay on the board, unchained.',
      }))
    )
      return
    await deleteWorkchain(c.id)
      .then(() => {
        onChain(null)
        invalidate()
      })
      .catch(failure('Deleting the workchain'))
  }

  // ── Tickets joining the chain ─────────────────────────────────────────────
  // Two doors, one write. A CLICK in the panel appends (the pipeline reading:
  // the api wires it to the tail). A DRAG onto the canvas lands the card where
  // it was dropped and draws no wire — the gesture already said where, and a
  // volunteered tail edge would be a predecessor nobody asked for.
  const addToFocused = (taskId: string) => {
    const c = focusedChain
    if (!c) return
    void addWorkchainStep(c.id, taskId)
      .then(() => {
        invalidate()
        canvas?.revealAll()
      })
      .catch(failure('Adding the ticket'))
  }

  const dropOnCanvas = (
    taskId: string,
    at: { x: number; y: number },
    onWire?: { fromTaskId: string; toTaskId: string },
  ) => {
    const c = focusedChain
    if (!c) return
    void (async () => {
      try {
        await addWorkchainStep(c.id, taskId, { wire: false })
        await updateWorkchain(c.id, { nodes: [{ taskId, x: Math.round(at.x), y: Math.round(at.y) }] })
        // Dropped ON a wire: thread it through that connection. The api's
        // `after:` wedge is the other shape — it moves EVERY one of the
        // anchor's successors onto the new step, which is right for "insert
        // into a line" and wrong for "insert into this one wire".
        if (onWire) {
          for (const op of spliceIntoWire(onWire, taskId)) {
            if (op.kind === 'add') await addWorkchainEdge(c.id, op.fromTaskId, op.toTaskId)
            else await removeWorkchainEdge(c.id, op.fromTaskId, op.toTaskId)
          }
        }
        invalidate()
      } catch (e) {
        failure('Adding the ticket')(e)
      }
    })()
  }
</script>

<div class="flex h-full min-h-0">
  {#if sidebarOpen}
    <WorkchainSidebar
      {unchained}
      chainedCount={chainedShown}
      {doneHidden}
      {boardStatuses}
      {members}
      agents={filterCtx.agents}
      labels={filterCtx.labels}
      meId={filterCtx.meId}
      q={filterCtx.q}
      onQ={filterCtx.onQ}
      filters={filterCtx.filters}
      onFilters={filterCtx.onFilters}
      showArchived={filterCtx.showArchived}
      onArchived={filterCtx.onArchived}
      canEdit={canEdit && focusedChain !== null}
      onAdd={addToFocused}
      {onOpen}
      onClose={() => setSidebar(false)}
    />
  {/if}

  <div class="flex min-h-0 min-w-0 flex-1 flex-col">
    {#if chainsList.notice}<div class="px-4 pt-4"><QueryError {...chainsList.notice} /></div>{/if}

    {#if chainsList.pending}
      <div class="flex min-h-0 flex-1 flex-col p-4">
        <Skeleton class="h-8 w-56 shrink-0 rounded-md" />
        <Skeleton class="mt-2 min-h-0 w-full flex-1 rounded-lg" />
      </div>
    {:else if chainsList.failed}
      <!-- The notice above owns the region; an empty canvas under a failed
           read would say "and also no chains". -->
    {:else if chains.length === 0}
      <EmptyState
        class="min-h-0 flex-1"
        icon="⭆"
        title="No workchains yet"
        hint={canEdit
          ? 'Chain tickets into a pipeline — steps hand off in order, and the chain shows where work is stuck.'
          : 'This board has no chains. An editor can create the first one.'}
        action={canEdit ? createAction : undefined}
      />
    {:else}
      <div class="flex min-h-0 flex-1 flex-col p-4">
        <!-- The chain's toolbar. Nothing on this line belongs to the board. -->
        <div class="flex shrink-0 flex-wrap items-center gap-2">
          {#if !sidebarOpen}
            <button
              type="button"
              title="Show the ticket panel"
              onclick={() => setSidebar(true)}
              class="flex h-8 items-center gap-1.5 rounded-md border border-line bg-panel px-2 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:border-line-strong hover:text-fg"
            >
              <PanelLeftOpen size={14} />
              Tickets
              {#if unchained.length > 0}<span class="tracking-[0.05em]">{unchained.length}</span>{/if}
            </button>
          {/if}
          <DropdownMenu align="left" items={chainMenuItems}>
            {#snippet trigger(open)}
              <button
                type="button"
                title="Switch, create or manage workchains"
                class={cn(
                  'flex h-8 min-w-0 max-w-72 items-center gap-2 rounded-md border border-line bg-panel px-2 font-sans text-sm text-fg transition-colors hover:border-line-strong',
                  open && 'border-line-strong',
                )}
              >
                <Workflow size={14} class="shrink-0 text-muted" />
                <span class="min-w-0 truncate">{focusedChain?.name}</span>
                {#if menuPos > 0}
                  <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{menuPos}/{visibleChains.length}</span>
                {/if}
                <ChevronDown size={14} class={cn('shrink-0 text-muted transition-transform', open && 'rotate-180')} />
              </button>
            {/snippet}
          </DropdownMenu>
          {#if focusedChain}
            <span class="font-mono text-[10px] tracking-[0.05em] text-muted">{chainProgress(focusedChain)}</span>
            {#if focusedChain.paused}
              <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-warning">paused</span>
            {/if}
          {/if}
          {#if canEdit}
            <button
              type="button"
              onclick={() => void addChain()}
              class="ml-auto flex items-center gap-1 rounded-md px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg"
            >
              <Plus size={12} /> New workchain
            </button>
          {/if}
        </div>
        {#if focusedChain}
          <div class="mt-2 min-h-0 flex-1">
            <WorkchainCanvas
              bind:this={canvas}
              workchain={focusedChain}
              boardId={board.id}
              {boardStatuses}
              agents={fleetAgents}
              {members}
              {onOpen}
              onChanged={invalidate}
              {canEdit}
              onDropTicket={dropOnCanvas}
              onOpenSidebar={() => setSidebar(true)}
            />
          </div>
        {/if}
      </div>
    {/if}
  </div>
</div>

{#snippet createAction()}
  <button
    type="button"
    class="flex items-center gap-1 rounded-md px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg"
    onclick={() => void addChain()}
  >
    <Workflow size={12} /> Create a workchain
  </button>
{/snippet}
