<script lang="ts">
  // Workchains view — the board's fourth lens (TALA-30; TALA-35 the graph,
  // TALA-34 the wiring editor). ONE chain fills the view: a toolbar line, and
  // the node canvas takes every pixel under it. Nothing stacks below it and
  // nothing scrolls — a wiring editor is a surface, and a surface you have to
  // scroll past to reach is not one.
  //
  // The toolbar carries the lens's two menus:
  //   · the WORKCHAIN menu (the trigger names the focused chain) — switch to
  //     any chain the filter leaves visible, create a new one, rename, pause,
  //     delete. One place for "create and manage", rather than a switcher here
  //     and half the verbs on the canvas header.
  //   · ADD TICKETS — the board's unchained tickets, searchable, each one a
  //     click away from joining the focused chain. This replaces the UNCHAINED
  //     list that used to stack under the canvas: the same set, as a menu, with
  //     its count on the trigger so work sitting outside every chain still
  //     announces itself.
  //
  // The canvas keeps only what needs canvas state: zoom, fit, and Tidy up, as
  // a cluster floating over its own bottom-left corner.
  //
  // The menu follows the lens convention (list/gantt): it offers the chains
  // whose steps the board's filtered `tasks` still contain — EXCEPT archived
  // steps, which always count: they are chain structure. A chain leaves the
  // menu only when it has steps and ALL of them are filtered out; an empty
  // chain stays (its picker is the only way to seed it). The canvas itself
  // draws the whole chain, filter or not.
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ChevronDown, ListPlus, Workflow } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { toastError } from '@/lib/toast.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Popover from '@/components/ui/Popover.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import { confirmDelete, prompt } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { useAgents } from '@/lib/agents'
  import { useBoardStatuses } from '@/lib/statuses'
  import { EFFORT_LABEL, type Task } from '@/lib/task-const'
  import type { Board, BoardMember } from '@/lib/boards.svelte'
  import WorkchainCanvas from './WorkchainCanvas.svelte'
  import { addWorkchainStep, createWorkchain, deleteWorkchain, updateWorkchain, useBoardWorkchains } from '@/lib/workchain-client'
  import { chainProgress, chainedTaskIds, filterCandidates, pickFocusedChain, type WorkchainStep } from '@/lib/workchain-rules'

  let {
    board,
    tasks,
    members = [],
    onOpen,
  }: {
    board: Board
    /** The board's filtered tickets — what this lens draws. */
    tasks: Task[]
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
    toastError(`${what} failed`, e)

  /** The canvas's imperative handle — the one thing the toolbar needs from it:
   *  after a ticket joins, show the reader where it landed. */
  let canvas = $state<WorkchainCanvas | null>(null)

  const addChain = async () => {
    const name = await prompt({ title: 'New workchain', message: 'Name this chain of tickets.', confirmLabel: 'Create' })
    if (!name?.trim()) return
    await createWorkchain(board.id, name.trim()).then(invalidate).catch(failure('Creating the workchain'))
  }

  // ── What the menu offers and the ticket picker draws ──────────────────────
  const filteredIds = $derived(new Set(tasks.map((t) => t.id)))
  const chained = $derived(chainedTaskIds(chains))

  /** A step counts when its ticket passes the filter — or it is archived:
   *  chain structure, kept even with the archived chip off. */
  const stepShows = (s: WorkchainStep): boolean => s.state === 'archived' || filteredIds.has(s.taskId)
  /** A chain with steps leaves the menu only when the filter removed every
   *  one; an empty chain stays (its picker is the only way to seed it). */
  const chainShows = (w: { steps: WorkchainStep[] }): boolean =>
    w.steps.length === 0 || w.steps.some(stepShows)

  /** What the workchain menu offers — the chains the filter leaves visible. */
  const visibleChains = $derived(chains.filter(chainShows))
  /** The tickets no chain holds: what "Add tickets" offers. */
  const unchained = $derived(tasks.filter((t) => !chained.has(t.id)))

  // ── Which chain fills the view ────────────────────────────────────────────
  // The menu is the ONLY writer of focusedId; a delete leaves it stale on
  // purpose — pickFocusedChain's fallback lands on the first survivor, so the
  // view never sits focused on nothing.
  let focusedId = $state<string | null>(null)
  const focused = $derived(pickFocusedChain(chains, focusedId))
  const focusedChain = $derived(chains.find((c) => c.id === focused) ?? null)

  /** The trigger's `2/5` — where the focused chain sits among the offered
   *  ones (hidden when the focused chain is one the filter removed). */
  const menuPos = $derived(visibleChains.findIndex((w) => w.id === focused) + 1)
  const menuCount = $derived(visibleChains.length)

  /** The one menu: switch, create, manage. Everything a reader might want to
   *  DO to a workchain hangs off the control that names it. */
  const chainMenuItems = $derived.by(() => {
    const items: ContextMenuEntry[] = visibleChains.map((w) => ({
      label: w.name,
      checked: w.id === focused,
      onSelect: () => (focusedId = w.id),
    }))
    if (!canEdit) return items
    items.push('sep', { label: 'New workchain', onSelect: () => void addChain() })
    if (focusedChain) {
      items.push(
        { label: 'Rename workchain', onSelect: () => void promptRenameFocused() },
        {
          label: focusedChain.paused ? 'Resume chain' : 'Pause chain',
          onSelect: () => togglePaused(),
        },
        'sep',
        { label: 'Delete workchain', danger: true, onSelect: () => void deleteFocused() },
      )
    }
    return items
  })

  const togglePaused = () => {
    const c = focusedChain
    if (!c) return
    void updateWorkchain(c.id, { paused: !c.paused })
      .then(invalidate)
      .catch(failure(c.paused ? 'Resuming the chain' : 'Pausing the chain'))
  }

  /** The prompt seeded with the current name. */
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

  /** The repo's double opt-in (type the name), then the request. Its tickets
   *  stay on the board, unchained. */
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
    await deleteWorkchain(c.id).then(invalidate).catch(failure('Deleting the workchain'))
  }

  // ── Add tickets ───────────────────────────────────────────────────────────
  // A type-to-filter list over the board's unchained tickets. A row click
  // appends the ticket to the focused chain and leaves the menu OPEN — seeding
  // a pipeline is a run of picks, not one — and the canvas re-fits so the
  // reader sees where the new card landed rather than hunting for it.
  let addDraft = $state('')
  const addRows = $derived(filterCandidates(unchained, addDraft))

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
</script>

<div class="flex h-full min-h-0 flex-col">
  {#if chainsList.notice}<div class="px-4 pt-4"><QueryError {...chainsList.notice} /></div>{/if}

  {#if chainsList.pending}
    <!-- First load: the shape of the coming view — a toolbar line, then ONE
         canvas block taking the rest — not the kanban columns or the list's
         table. -->
    <div class="flex min-h-0 flex-1 flex-col p-4">
      <Skeleton class="h-8 w-56 shrink-0 rounded-md" />
      <Skeleton class="mt-2 min-h-0 w-full flex-1 rounded-lg" />
    </div>
  {:else if chainsList.failed}
    <!-- The notice (rendered above) owns the whole region; an empty canvas
         under a failed read would read as "and also no chains". -->
  {:else if chains.length === 0}
    <!-- `full` owns the pane edge to edge, vignette included — so it gets the
         pane, not a padded box inside it (docs/UI-CONVENTIONS.md). -->
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
    <!-- The toolbar: the chain's menu on the left, the ticket menu on the
         right, and nothing else competing for the line. -->
    <div class="flex shrink-0 flex-wrap items-center gap-2">
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
              <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{menuPos}/{menuCount}</span>
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
      {#if canEdit && focusedChain}
        <div class="ml-auto">
        <Popover align="right">
          {#snippet trigger(open)}
            <button
              type="button"
              title="Add a ticket from this board to the chain"
              class={cn(
                'flex items-center gap-1 rounded-md px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
                open ? 'text-fg' : 'text-muted hover:text-fg',
              )}
              onclick={() => (addDraft = '')}
            >
              <ListPlus size={12} /> Add tickets
              {#if unchained.length > 0}
                <span class="tracking-[0.05em] text-muted">{unchained.length}</span>
              {/if}
            </button>
          {/snippet}
          {#snippet content()}
            <!-- The old UNCHAINED list, as a menu. It stays open on a pick:
                 seeding a chain is a run of them, and each added row leaves
                 the list on the re-read. -->
            <div class="flex w-72 flex-col gap-1">
              <Input autofocus bind:value={addDraft} placeholder="Search tickets" size="sm" />
              {#if unchained.length === 0}
                <p class="px-1 py-2 font-sans text-xs text-muted">Every ticket on this board is chained.</p>
              {:else if addRows.length === 0}
                <p class="px-1 py-2 font-sans text-xs text-muted">No tickets match.</p>
              {:else}
                <div class="flex max-h-72 flex-col gap-0.5 overflow-y-auto">
                  {#each addRows as t (t.id)}
                    <button
                      type="button"
                      onclick={() => addToFocused(t.id)}
                      class="flex min-w-0 items-center gap-2 rounded-md px-1.5 py-1 text-left transition-colors hover:bg-raised"
                    >
                      {#if t.ticketRef}
                        <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{t.ticketRef}</span>
                      {/if}
                      <span class="min-w-0 flex-1 truncate font-sans text-xs text-fg">{t.title}</span>
                      {#if t.effort}
                        <span class="shrink-0 rounded border border-line-subtle px-1 font-mono text-[9px] uppercase tracking-[0.05em] text-muted">
                          {EFFORT_LABEL[t.effort]}
                        </span>
                      {/if}
                    </button>
                  {/each}
                </div>
              {/if}
            </div>
          {/snippet}
        </Popover>
        </div>
      {/if}
    </div>
    {#if focusedChain}
      <!-- TALA-34: the chain renders as the node canvas — the wiring editor
           needs grabbable ports on any chain you want to extend, including a
           straight one (a rail has no ports, so A→B could never be authored
           by dragging). It takes every pixel under the toolbar. -->
      <div class="mt-2 min-h-0 flex-1">
        <WorkchainCanvas
          bind:this={canvas}
          workchain={focusedChain}
          boardId={board.id}
          {boardStatuses}
          {agents}
          {members}
          {onOpen}
          onChanged={invalidate}
          {canEdit}
        />
      </div>
    {/if}
    </div>
  {/if}
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
