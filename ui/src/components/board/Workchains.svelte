<script lang="ts">
  // Workchains view — the board's fourth lens (TALA-30; TALA-35 the graph,
  // TALA-34 the wiring editor). ONE chain fills the view at a time: the
  // switcher above the canvas picks which (the first by default;
  // pickFocusedChain's fallback also absorbs the stale focus a delete
  // leaves behind), and the chain renders as the node canvas
  // (WorkchainCanvas) — grabbable ports on every card, so A→B can be
  // authored by dragging. The rename/delete verbs ride the same dropdown
  // when you can edit; pause and zoom stay on the canvas header. Below the
  // canvas sits UNCHAINED — the tickets no chain holds yet, each row
  // offering to join one.
  //
  // The switcher follows the lens convention (list/gantt): it offers the
  // chains whose steps the board's filtered `tasks` still contain —
  // EXCEPT archived steps, which always count: they are chain structure.
  // A chain leaves the switcher only when it has steps and ALL of them are
  // filtered out; an empty chain stays (its picker is the only way to seed
  // it). The canvas itself draws the whole chain, filter or not.
  import { useQueryClient } from '@tanstack/svelte-query'
  import { ChevronDown, ChevronRight, Plus, Workflow } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { toastError } from '@/lib/toast.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import Select from '@/components/ui/Select.svelte'
  import { confirmDelete, prompt } from '@/components/ui/confirm.svelte'
  import type { ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { useAgents } from '@/lib/agents'
  import { statusColorOf, useBoardStatuses } from '@/lib/statuses'
  import { EFFORT_LABEL, type Task } from '@/lib/task-const'
  import type { Board, BoardMember } from '@/lib/boards.svelte'
  import WorkchainCanvas from './WorkchainCanvas.svelte'
  import { addWorkchainStep, createWorkchain, deleteWorkchain, updateWorkchain, useBoardWorkchains } from '@/lib/workchain-client'
  import { chainedTaskIds, pickFocusedChain, type WorkchainStep } from '@/lib/workchain-rules'

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

  const addChain = async () => {
    const name = await prompt({ title: 'New workchain', message: 'Name this chain of tickets.', confirmLabel: 'Create' })
    if (!name?.trim()) return
    await createWorkchain(board.id, name.trim()).then(invalidate).catch(failure('Creating the workchain'))
  }

  // ── What the switcher offers and the Unchained section draw ────────────────
  const filteredIds = $derived(new Set(tasks.map((t) => t.id)))
  const chained = $derived(chainedTaskIds(chains))

  /** A step counts when its ticket passes the filter — or it is archived:
   *  chain structure, kept even with the archived chip off. */
  const stepShows = (s: WorkchainStep): boolean => s.state === 'archived' || filteredIds.has(s.taskId)
  /** A chain with steps leaves the switcher only when the filter removed
   *  every one; an empty chain stays (its picker is the only way to seed it). */
  const chainShows = (w: { steps: WorkchainStep[] }): boolean =>
    w.steps.length === 0 || w.steps.some(stepShows)

  /** What the switcher offers — the chains the filter leaves visible. */
  const visibleChains = $derived(chains.filter(chainShows))
  const unchained = $derived(tasks.filter((t) => !chained.has(t.id)))
  // Unchained collapses once chains exist (it is a backlog, not the show);
  // with no chains it is the whole view, so it starts open.
  let unchainedOpen = $state(false)
  const anyChains = $derived(chains.length > 0)
  $effect(() => {
    unchainedOpen = !anyChains
  })

  // ── Which chain fills the view ────────────────────────────────────────────
  // The switcher is the ONLY writer of focusedId; a delete leaves it stale
  // on purpose — pickFocusedChain's fallback lands on the first survivor,
  // so the view never sits focused on nothing.
  let focusedId = $state<string | null>(null)
  const focused = $derived(pickFocusedChain(chains, focusedId))
  const focusedChain = $derived(chains.find((c) => c.id === focused) ?? null)

  /** The trigger's `2/5` — where the focused chain sits among the offered
   *  ones (hidden when the focused chain is one the filter removed). */
  const switcherPos = $derived(visibleChains.findIndex((w) => w.id === focused) + 1)
  const switcherCount = $derived(visibleChains.length)

  const switcherItems = $derived.by(() => {
    const items: ContextMenuEntry[] = visibleChains.map((w) => ({
      label: w.name,
      checked: w.id === focused,
      onSelect: () => (focusedId = w.id),
    }))
    if (canEdit && focusedChain) {
      items.push(
        'sep',
        { label: 'Rename…', onSelect: () => void promptRenameFocused() },
        { label: 'Delete…', danger: true, onSelect: () => void deleteFocused() },
      )
    }
    return items
  })

  /** The canvas header's pencil (managed mode) hands over a validated name
   *  — trimmed, non-empty, actually changed. */
  const renameFocused = (name: string) => {
    const c = focusedChain
    if (!c) return
    void updateWorkchain(c.id, { name }).then(invalidate).catch(failure('Renaming the workchain'))
  }

  /** The dropdown's Rename… — a prompt seeded with the current name. */
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
    renameFocused(trimmed)
  }

  /** The dropdown's Delete… — the repo's double opt-in (type the name),
   *  then the request. Its tickets stay on the board, unchained. */
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

  const addTo = (chainId: string, taskId: string) =>
    void addWorkchainStep(chainId, taskId).then(invalidate).catch(failure('Adding the step'))
</script>

<div class="flex h-full flex-col">
  <div class="min-h-0 flex-1 space-y-6 overflow-y-auto p-4">
    {#if chainsList.notice}<QueryError {...chainsList.notice} class="mb-4" />{/if}

    {#if chainsList.pending}
      <!-- First load: the shape of the coming view — a switcher line, then
           ONE canvas block taking the rest — not the kanban columns or the
           list's table. -->
      <div class="flex h-full flex-col">
        <Skeleton class="h-8 w-56 shrink-0 rounded-md" />
        <Skeleton class="mt-2 min-h-0 w-full flex-1 rounded-lg" />
      </div>
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
      <!-- ONE chain fills the view at a time: the switcher above the canvas
           picks which (the first by default), the canvas takes the rest of
           the height, and UNCHAINED scrolls in below it. One at a time
           keeps the wiring editor a surface, not a scroll. -->
      <div class="flex h-full min-h-0 flex-1 flex-col">
        <div class="flex shrink-0 items-center gap-2">
          <DropdownMenu align="left" items={switcherItems}>
            {#snippet trigger(open)}
              <button
                type="button"
                class={cn(
                  'flex h-8 min-w-0 max-w-72 items-center gap-2 rounded-md border border-line bg-panel px-2 font-sans text-sm text-fg transition-colors hover:border-line-strong',
                  open && 'border-line-strong',
                )}
              >
                <Workflow size={14} class="shrink-0 text-muted" />
                <span class="min-w-0 truncate">{focusedChain?.name}</span>
                {#if switcherPos > 0}
                  <span class="shrink-0 font-mono text-[10px] tracking-[0.05em] text-muted">{switcherPos}/{switcherCount}</span>
                {/if}
                <ChevronDown size={14} class={cn('shrink-0 text-muted transition-transform', open && 'rotate-180')} />
              </button>
            {/snippet}
          </DropdownMenu>
          {#if canEdit}
            <button
              type="button"
              onclick={() => void addChain()}
              class="flex items-center gap-1 rounded-md px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg"
            >
              <Plus size={12} /> New workchain
            </button>
          {/if}
        </div>
        {#if focusedChain}
          <!-- TALA-34: the chain renders as the node canvas — the wiring
               editor needs grabbable ports on any chain you want to extend,
               including a straight one (a rail has no ports, so A→B could
               never be authored by dragging). The rail leaves the lens; the
               cheap cousin (moveStepOrder reorder) lives on in TaskDetail. -->
          <div class="min-h-0 flex-1">
            <WorkchainCanvas
              workchain={focusedChain}
              boardId={board.id}
              {boardStatuses}
              {agents}
              {members}
              {onOpen}
              onChanged={invalidate}
              managed={canEdit}
              onRename={renameFocused}
              onDelete={() => void deleteFocused()}
            />
          </div>
        {/if}
      </div>
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
