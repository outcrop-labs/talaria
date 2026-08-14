<script lang="ts">
  import { untrack } from 'svelte'
  import { searchParams } from 'sv-router'
  import { useQueryClient } from '@tanstack/svelte-query'
  import { CalendarRange, Layers, LayoutGrid, List, Plus } from '@lucide/svelte'
  import { navigate, route } from '@/router'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import SkeletonCard from '@/components/ui/SkeletonCard.svelte'
  import BoardHeader from '@/components/board/BoardHeader.svelte'
  import Kanban from '@/components/board/Kanban.svelte'
  import BoardList from '@/components/board/BoardList.svelte'
  import type { GroupByKey } from '@/components/board/board-list'
  import Gantt from '@/components/board/Gantt.svelte'
  import BoardSettingsModal from '@/components/board/BoardSettingsModal.svelte'
  import FilterBar from '@/components/board/FilterBar.svelte'
  import { filtersActive, type BoardFilters } from '@/components/board/filter-bar'
  import EmptyState from '@/components/ui/EmptyState.svelte'
  import QueryError from '@/components/ui/QueryError.svelte'
  import { listQuery } from '@/components/ui/query-state'
  import Input from '@/components/ui/Input.svelte'
  import FieldPill from '@/components/ui/FieldPill.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import { staggerIn } from '@/lib/motion'
  import { useAgents } from '@/lib/agents'
  import {
    useBoards,
    useArchivedBoards,
    useBoardTasks,
    useBoardLive,
    useBoardAgents,
    useBoardMembers,
    useBoardViews,
    useBoardLabels,
    createBoardView,
    updateBoardView,
    deleteBoardView,
    type BoardViewConfig,
  } from '@/lib/boards.svelte'
  import ContextMenu from '@/components/ui/ContextMenu.svelte'
  import DropdownMenu from '@/components/ui/DropdownMenu.svelte'
  import { useContextMenu, type ContextMenuEntry } from '@/components/ui/context-menu.svelte'
  import { confirm, prompt } from '@/components/ui/confirm.svelte'
  import { useSession } from '@/lib/session'
  import { useBoardStatuses, type BoardStatus } from '@/lib/statuses'
  import type { Task } from '@/lib/task-const'
  import { isClosedStatus } from '@/components/board/field-pills'

  // All view state lives in the URL (deep-link convention): view, group, q, and
  // the filter facets (comma-multi within a facet: OR inside, AND across).
  interface BoardSearch {
    view?: 'list' | 'gantt'
    /** Active saved view id (styling only — the config params travel too). */
    v?: string
    group?: GroupByKey
    q?: string
    status?: string
    assignee?: string
    priority?: string
    label?: string
    due?: 'overdue' | 'today' | 'week' | 'none'
    archived?: boolean
  }

  const str = (v: unknown): string | undefined => (typeof v === 'string' && v ? v : undefined)
  /** Raw string read of a search param (sv-router parses "1"/"true" — coerce back). */
  const sp = (k: string): string | undefined => {
    const v = searchParams.get(k)
    return v == null || v === '' ? undefined : String(v)
  }

  // The old route's validateSearch, applied to the reactive URL.
  const search = $derived.by((): BoardSearch => {
    const s: Record<string, unknown> = {
      view: sp('view'),
      v: sp('v'),
      group: sp('group'),
      q: sp('q'),
      status: sp('status'),
      assignee: sp('assignee'),
      priority: sp('priority'),
      label: sp('label'),
      due: sp('due'),
      archived: searchParams.get('archived') === true,
    }
    return {
      ...(s.view === 'list' || s.view === 'gantt' ? { view: s.view as 'list' | 'gantt' } : {}),
      ...(str(s.v) ? { v: str(s.v) } : {}),
      ...(['status', 'priority', 'assignee', 'label', 'none'].includes(s.group as string) && s.group !== 'status'
        ? { group: s.group as GroupByKey }
        : {}),
      ...(str(s.q) ? { q: str(s.q) } : {}),
      ...(str(s.status) ? { status: str(s.status) } : {}),
      ...(str(s.assignee) ? { assignee: str(s.assignee) } : {}),
      ...(str(s.priority) ? { priority: str(s.priority) } : {}),
      ...(str(s.label) ? { label: str(s.label) } : {}),
      ...(['overdue', 'today', 'week', 'none'].includes(s.due as string) ? { due: s.due as BoardSearch['due'] } : {}),
      ...(s.archived === true ? { archived: true } : {}),
    }
  })

  const split = (v?: string): string[] => (v ? v.split(',').filter(Boolean) : [])
  const joinOrDrop = (arr: string[]): string | undefined => (arr.length ? arr.join(',') : undefined)

  const dayEnd = (offsetDays: number) => {
    const d = new Date()
    d.setDate(d.getDate() + offsetDays)
    d.setHours(23, 59, 59, 999)
    return d.getTime()
  }

  function matchesDue(t: Task, due: BoardSearch['due'], statuses: BoardStatus[]): boolean {
    if (!due) return true
    const open = !isClosedStatus(t.status, statuses)
    switch (due) {
      case 'none':
        return !t.dueDate
      case 'overdue':
        return !!t.dueDate && Date.parse(t.dueDate) < Date.now() && open
      case 'today':
        return !!t.dueDate && Date.parse(t.dueDate) <= dayEnd(0) && open
      case 'week':
        return !!t.dueDate && Date.parse(t.dueDate) <= dayEnd(7) && open
    }
  }

  // Loose params (not getParams): this component also renders BEHIND the
  // ticket route (Task.svelte draws the board under its overlay), where the
  // pathname is /boards/:boardId/:taskId.
  const boardId = $derived(route.params.boardId ?? '')
  const meQuery = useSession()
  const me = $derived(meQuery.data)
  const boardsQuery = useBoards()
  const boards = $derived(boardsQuery.data ?? [])
  const isLoading = $derived(boardsQuery.isLoading)
  const archivedQuery = useArchivedBoards()
  const archivedBoards = $derived(archivedQuery.data ?? [])
  const archivedLoading = $derived(archivedQuery.isLoading)
  const board = $derived(boards.find((b) => b.id === boardId) ?? archivedBoards.find((b) => b.id === boardId))

  const showArchived = $derived(search.archived ?? false)
  const tasksQuery = useBoardTasks(() => (board ? boardId : null), () => showArchived)
  const allTasks = $derived(tasksQuery.data ?? [])
  const tasksLoading = $derived(tasksQuery.isLoading)
  const agentsQuery = useAgents()
  const fleet = $derived(agentsQuery.data)
  const fleetLoading = $derived(agentsQuery.isLoading)
  const boardCfgQuery = useBoardAgents(() => (board ? boardId : null))
  const boardCfg = $derived(boardCfgQuery.data)
  const cfgLoading = $derived(boardCfgQuery.isLoading)
  // Four supporting reads. Defaulted to `[]` each one furnished the toolbar
  // with a confident absence: no teammates to filter by, no labels on this
  // board, no columns (so the board silently drew the built-in ones), and no
  // saved views. All four discarded their query on the line that made it.
  const membersList = listQuery(useBoardMembers(() => (board ? boardId : null)), { title: 'Could not load who’s on this board', variant: 'inline' })
  const labelsList = listQuery(useBoardLabels(() => (board ? boardId : null)), { title: 'Could not load labels', variant: 'inline' })
  const statusesList = listQuery(useBoardStatuses(() => (board ? boardId : null)), { title: 'Could not load this board’s columns', variant: 'inline' })
  const members = $derived(membersList.rows)
  const registryLabels = $derived(labelsList.rows)
  const boardStatuses = $derived(statusesList.rows)
  // Only agents allowed on this board are assignable/filterable here.
  const boardAgents = $derived(
    boardCfg?.allowAll
      ? fleet?.agents ?? []
      : (fleet?.agents ?? []).filter((a) => boardCfg?.models.includes(a.id)),
  )
  useBoardLive(() => (board ? boardId : null))

  const qc = useQueryClient()
  // Per-user preference (localStorage): render every status/priority group
  // in list mode, empty ones included, as drop lanes.
  let showEmptyGroups = $state(false)
  $effect(() => {
    try {
      showEmptyGroups = localStorage.getItem('talaria:list-empty-groups') === '1'
    } catch {
      /* ignore */
    }
  })
  const toggleEmptyGroups = () => {
    try {
      localStorage.setItem('talaria:list-empty-groups', showEmptyGroups ? '0' : '1')
    } catch {
      /* ignore */
    }
    showEmptyGroups = !showEmptyGroups
  }
  const viewsList = listQuery(useBoardViews(() => (board ? boardId : null)), { title: 'Could not load your saved views', variant: 'inline' })
  const savedViews = $derived(viewsList.rows)
  const menu = useContextMenu()
  let settingsOpen = $state(false)
  const view = $derived(search.view ?? 'board')
  const groupBy: GroupByKey = $derived(search.group ?? 'status')
  const q = $derived(search.q ?? '')
  const filters = $derived.by(
    (): BoardFilters => ({
      statuses: split(search.status),
      assignees: split(search.assignee),
      priorities: split(search.priority),
      labels: split(search.label),
      due: search.due ?? '',
    }),
  )

  // User picks PUSH (back walks them); typing REPLACEs (no history spam).
  const VIEW_KEYS: Array<keyof BoardSearch> = ['view', 'group', 'q', 'status', 'assignee', 'priority', 'label', 'due']
  const setSearch = (patch: Partial<BoardSearch>, replace = false) => {
    const next: Partial<BoardSearch> = { ...search, ...patch }
    // Manually changing any view/filter detaches from the saved view —
    // the tab un-highlights the moment the state stops being that view.
    if (patch.v === undefined && VIEW_KEYS.some((k) => k in patch)) delete next.v
    for (const k of Object.keys(next) as Array<keyof BoardSearch>) {
      if (next[k] === '' || next[k] === false || next[k] === undefined) delete next[k]
    }
    void navigate('/boards/:boardId', { params: { boardId }, search: next as Record<string, string | boolean>, replace })
  }
  const setFilters = (f: BoardFilters) =>
    setSearch({
      status: joinOrDrop(f.statuses),
      assignee: joinOrDrop(f.assignees),
      priority: joinOrDrop(f.priorities),
      label: joinOrDrop(f.labels),
      due: f.due || undefined,
    })

  // ── Saved views: apply / save / manage ─────────────────────────────────
  const currentConfig = (): BoardViewConfig => ({
    ...(view !== 'board' ? { view } : {}),
    ...(groupBy !== 'status' ? { group: groupBy } : {}),
    ...(q ? { q } : {}),
    ...(search.status ? { status: search.status } : {}),
    ...(search.assignee ? { assignee: search.assignee } : {}),
    ...(search.priority ? { priority: search.priority } : {}),
    ...(search.label ? { label: search.label } : {}),
    ...(search.due ? { due: search.due } : {}),
  })
  const invalidateViews = () => qc.invalidateQueries({ queryKey: ['board-views', boardId] })
  const applyView = (sv: { id: string; config: BoardViewConfig }) =>
    void navigate('/boards/:boardId', { params: { boardId }, search: { ...sv.config, v: sv.id } as Record<string, string> })
  const saveCurrentAsView = async () => {
    const name = await prompt({ title: 'Save view', message: 'Name this view (current layout, grouping, and filters are captured).', confirmLabel: 'Save' })
    if (!name?.trim()) return
    const r = (await createBoardView(boardId, name.trim(), currentConfig())) as { view?: { id: string } }
    await invalidateViews()
    if (r.view?.id) setSearch({ v: r.view.id })
  }
  const viewTabContextMenu = (e: MouseEvent, sv: { id: string; name: string; config: BoardViewConfig }) =>
    menu.openMenu(e, [
      {
        label: 'Update to current filters',
        onSelect: () => void updateBoardView(boardId, sv.id, { config: currentConfig() }).then(invalidateViews),
      },
      {
        label: 'Rename',
        onSelect: () =>
          void (async () => {
            const name = await prompt({ title: 'Rename view', message: sv.name, confirmLabel: 'Rename' })
            if (name?.trim()) await updateBoardView(boardId, sv.id, { name: name.trim() }).then(invalidateViews)
          })(),
      },
      'sep',
      {
        label: 'Delete view',
        danger: true,
        onSelect: () =>
          void (async () => {
            if (!(await confirm({ title: `Delete view "${sv.name}"?`, message: 'The filters it saved are lost; tickets are untouched.', danger: true }))) return
            await deleteBoardView(boardId, sv.id)
            await invalidateViews()
            if (search.v === sv.id) setSearch({ v: undefined })
          })(),
      },
    ])

  // The label registry powers the facet (colored); names still filter tags.
  const boardLabels = $derived(registryLabels)

  const tasks = $derived.by(() => {
    const query = q.trim().toLowerCase()
    return allTasks.filter(
      (t) =>
        (!query ||
          t.title.toLowerCase().includes(query) ||
          (t.ticketRef ?? '').toLowerCase().includes(query) ||
          (t.description ?? '').toLowerCase().includes(query) ||
          t.tags.some((tag) => tag.toLowerCase().includes(query))) &&
        (!filters.statuses.length || filters.statuses.includes(t.status)) &&
        (!filters.assignees.length ||
          filters.assignees.some((a) => (a === '__none' ? t.assignees.length === 0 : t.assignees.includes(a)))) &&
        (!filters.priorities.length || filters.priorities.includes(t.priority)) &&
        (!filters.labels.length || filters.labels.some((l) => t.tags.includes(l))) &&
        matchesDue(t, filters.due || undefined, boardStatuses),
    )
  })

  // ORDER MATTERS below (see the template). "Board not found — it may have been
  // deleted" is the sentence that convinced an owner his company's boards were
  // gone; it may only be reached when the board list genuinely LOADED and this
  // board wasn't in it. A failed list, or failed tasks, says so instead and
  // offers a retry. The archived list matters too: this board may be an
  // archived one, and a failed archived read would otherwise land on the same
  // false "not found".
  const boardListQuery = $derived(boardsQuery.isError ? boardsQuery : archivedQuery)

  // ── The layout you picked, per board ────────────────────────────────────
  //
  // Kanban was not a choice anyone had made — it is what an absent `?view=`
  // decodes to, and every arrival decoded it afresh. PER BOARD, not per person,
  // because the preference is about the board's shape: a roadmap wants gantt,
  // a triage board wants the list. Saved views (`?v=`) stay what they are — a
  // named lens you apply deliberately — so one of those is never overridden.
  const viewPrefKey = (id: string) => `talaria:board-view:${id}`
  const setView = (next: 'board' | 'list' | 'gantt') => {
    try {
      localStorage.setItem(viewPrefKey(boardId), next)
    } catch {
      /* private mode: the URL still carries this session's choice */
    }
    setSearch({ view: next === 'board' ? undefined : next })
  }
  // Restore once per board. Keyed on the id rather than a boolean so that
  // walking between boards restores each one, while changing a filter — which
  // re-runs this effect through `search` — does not drag the view back.
  let restoredFor: string | null = null
  $effect(() => {
    const id = boardId
    if (restoredFor === id) return
    restoredFor = id
    // An explicit `?view=` or a saved view already answers the question, and a
    // pasted link must survive: only a bare board URL is ours to fill in.
    if (untrack(() => search.view || search.v)) return
    let saved: string | null = null
    try {
      saved = localStorage.getItem(viewPrefKey(id))
    } catch {
      /* ignore */
    }
    if (saved === 'list' || saved === 'gantt') setSearch({ view: saved }, true)
  })

  const toggleCls = (active: boolean) =>
    cn(
      'grid h-7 w-7 place-items-center rounded-md transition-colors',
      focusGold,
      active ? 'bg-raised text-fg' : 'text-muted hover:bg-hover hover:text-fg',
    )
  // Opening a ticket keeps the CURRENT view/filter state — the overlay sits
  // on top of whatever view you were in, and closing returns you to it.
  const openTicket = (taskId: string) =>
    void navigate('/boards/:boardId/:taskId', { params: { boardId, taskId }, search: searchParams.toString() })
  const canEdit = $derived(board?.role === 'owner' || board?.role === 'editor')
</script>

{#snippet groupByIcon()}<Layers size={12} />{/snippet}

<!-- One continuous skeleton across the serial fetch (boards → tasks): the board
     must not paint with empty columns while its tasks are still in flight.
     `!showArchived` keeps the gate to the cold-load path — flipping the archived
     toggle refetches under the toolbar, not under a full-page skeleton.

     The ARCHIVED list is part of the same gate. The two lists race, and a board
     that lives only in the archived one would otherwise flash "Board not found
     — It may have been deleted" for as long as that request takes: the exact
     false-deletion sentence this whole path exists to prevent. Neither list has
     spoken until both have. -->
{#if isLoading || archivedLoading || (board && !showArchived && tasksLoading)}
  <div class="grid h-full grid-cols-2 gap-3 overflow-hidden p-6 sm:grid-cols-4">
    {#each [0, 1, 2, 3] as c (c)}
      <div class="space-y-3">
        <Skeleton class="h-3 w-20 rounded-full" delay={c * 0.1} />
        {#each [0, 1, 2] as r (r)}
          <SkeletonCard delay={c * 0.1 + r * 0.15} />
        {/each}
      </div>
    {/each}
  </div>
{:else if !board && boardListQuery.isError && boardListQuery.data === undefined}
  <QueryError
    error={boardListQuery.error}
    title="Could not load this board"
    onRetry={() => {
      void boardsQuery.refetch()
      void archivedQuery.refetch()
    }}
  />
{:else if board && tasksQuery.isError && tasksQuery.data === undefined}
  <QueryError
    error={tasksQuery.error}
    title={`Could not load ${board.name}`}
    onRetry={() => void tasksQuery.refetch()}
  />
{:else if !board}
  <EmptyState icon="⧉" title="Board not found" hint="It may have been deleted, or you don’t have access." />
{:else}
  <!-- Page content entrance AND post-skeleton reveal in one: this branch
       mounts when board+tasks land, so header → view row → filter row → the
       task canvas rise in sequence (ANIMATIONS.md). The canvas is ONE child —
       its columns/rows are query data and never stagger. Never on the
       skeleton branch. -->
  <div use:staggerIn class="flex h-full min-w-0 flex-col">
    <BoardHeader {board} onSettings={() => (settingsOpen = true)} />

    <!-- Row 1 — the VIEW: mode toggle, saved views, save. -->
    <div class="flex flex-wrap items-center gap-2 border-b border-line-subtle px-5 py-2">
      <div class="flex rounded-md border border-line p-0.5">
        <button
          class={toggleCls(view === 'board')}
          onclick={() => setView('board')}
          title="Board view"
          aria-label="Board view"
        >
          <LayoutGrid size={15} />
        </button>
        <button
          class={toggleCls(view === 'list')}
          onclick={() => setView('list')}
          title="List view"
          aria-label="List view"
        >
          <List size={15} />
        </button>
        <button
          class={toggleCls(view === 'gantt')}
          onclick={() => setView('gantt')}
          title="Gantt view"
          aria-label="Gantt view"
        >
          <CalendarRange size={15} />
        </button>
      </div>
      {#if viewsList.notice}<QueryError {...viewsList.notice} />{/if}
      {#if savedViews.length > 0}<div class="h-5 w-px bg-line-subtle"></div>{/if}
      {#each savedViews as sv (sv.id)}
        <!-- The tab wears its view type: board grid, list, or gantt. -->
        {@const TypeIcon = sv.config.view === 'gantt' ? CalendarRange : sv.config.view === 'list' ? List : LayoutGrid}
        <button
          onclick={() => applyView(sv)}
          oncontextmenu={(e) => viewTabContextMenu(e, sv)}
          class={cn(
            'flex items-center gap-1.5 rounded-md border px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
            focusGold,
            search.v === sv.id
              ? 'border-[var(--theme-accent-border)] bg-raised text-fg'
              : 'border-transparent text-muted hover:bg-hover hover:text-fg',
          )}
        >
          <TypeIcon size={12} class={cn('shrink-0', search.v === sv.id ? 'text-accent' : 'opacity-70')} />
          {sv.name}
        </button>
      {/each}
      {#if canEdit}
        <button
          onclick={() => void saveCurrentAsView()}
          title="Save the current layout, grouping, and filters as a named view"
          class={cn(
            'flex items-center gap-1 rounded-md px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg',
            focusGold,
          )}
        >
          <Plus size={12} /> Save view
        </button>
      {/if}
      <span class="ml-auto font-mono text-[10px] uppercase tracking-[0.05em] text-muted">
        {filtersActive(filters) || q ? `${tasks.length} of ${allTasks.length}` : `${allTasks.length} tickets`}
      </span>
      <ContextMenu {menu} />
    </div>

    <!-- Row 2 — the QUERY: search + filters left; display options right. -->
    <div class="flex flex-wrap items-center gap-2 border-b border-line-subtle px-5 py-1.5">
      <Input value={q} oninput={(e) => setSearch({ q: e.currentTarget.value }, true)} placeholder="Search" size="sm" class="w-40" />
      {#if fleetLoading || cfgLoading}
        <Skeleton class="h-9 w-64" />
      {:else}
        <FilterBar value={filters} onChange={setFilters} {members} agents={boardAgents} labels={boardLabels} statuses={boardStatuses} meId={me?.id} />
      {/if}
      <!-- A filter bar with a facet missing looks like a board that has no
           such thing. These say which read is the one that is missing. -->
      {#if membersList.notice || labelsList.notice || statusesList.notice}
        <div class="flex flex-wrap items-center gap-3">
          {#if membersList.notice}<QueryError {...membersList.notice} />{/if}
          {#if labelsList.notice}<QueryError {...labelsList.notice} />{/if}
          {#if statusesList.notice}<QueryError {...statusesList.notice} />{/if}
        </div>
      {/if}
      <span class="ml-auto flex items-center gap-1.5">
        {#if view === 'list'}
          <DropdownMenu
            align="right"
            items={[
              ...(['status', 'priority', 'assignee', 'label', 'none'] as GroupByKey[]).map((g) => ({
                label: g === 'none' ? 'No grouping' : g,
                checked: groupBy === g,
                onSelect: () => setSearch({ group: g }),
              })),
              ...(groupBy === 'status' || groupBy === 'priority'
                ? ([
                    'sep',
                    { label: 'Show empty groups', checked: showEmptyGroups, keepOpen: true, onSelect: toggleEmptyGroups },
                  ] as ContextMenuEntry[])
                : []),
            ]}
          >
            {#snippet trigger(open)}
              <FieldPill icon={groupByIcon} active={open || groupBy !== 'status'} class="h-9 rounded-md px-2" title="Group by">
                {groupBy === 'none' ? 'No grouping' : `Group: ${groupBy}`}
              </FieldPill>
            {/snippet}
          </DropdownMenu>
        {/if}
        <Chip onSelect={() => setSearch({ archived: !showArchived })} selected={showArchived} title="Include archived tickets">
          archived
        </Chip>
      </span>
    </div>

    <div class="relative min-h-0 min-w-0 flex-1">
      {#if view === 'board'}
        <Kanban {board} {tasks} onOpen={openTicket} {members} />
      {:else if view === 'gantt'}
        <Gantt {board} {tasks} onOpen={openTicket} />
      {:else}
        <BoardList {tasks} onOpen={openTicket} {boardId} {members} {canEdit} {groupBy} {showEmptyGroups} />
      {/if}
    </div>

    <BoardSettingsModal
      {board}
      open={settingsOpen}
      onClose={() => (settingsOpen = false)}
      onArchived={() => void navigate('/boards')}
      onDeleted={() => void navigate('/boards')}
    />

    <!-- Ticket detail is the sibling, directly-linkable route
         (/boards/:boardId/:taskId — Task.svelte): it renders this board
         itself, with the fixed overlay above it. -->
  </div>
{/if}
