import { createFileRoute, useNavigate, Outlet } from '@tanstack/react-router'
import { Skeleton, SkeletonCard } from '@/components/ui/skeleton'
import { useMemo, useState } from 'react'
import { CalendarRange, Layers, LayoutGrid, List, Plus } from 'lucide-react'
import { BoardHeader } from '@/components/board/board-header'
import { Kanban } from '@/components/board/kanban'
import { BoardList, type GroupByKey } from '@/components/board/board-list'
import { Gantt } from '@/components/board/gantt'
import { BoardSettingsModal } from '@/components/board/board-settings-modal'
import { FilterBar, filtersActive, type BoardFilters } from '@/components/board/filter-bar'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { FieldPill } from '@/components/ui/field-pill'
import { Chip } from '@/components/ui/chip'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import {
  useBoards,
  useArchivedBoards,
  useBoardTasks,
  useBoardLive,
  useBoardAgents,
  useBoardMembers,
  useBoardViews,
  createBoardView,
  updateBoardView,
  deleteBoardView,
  type BoardViewConfig,
} from '@/lib/boards'
import { useQueryClient } from '@tanstack/react-query'
import { useContextMenu, DropdownMenu } from '@/components/ui/context-menu'
import { confirm, prompt } from '@/components/ui/confirm'
import { useSession } from '@/lib/session'
import type { Task } from '@/lib/task-const'

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

export const Route = createFileRoute('/_app/boards/$boardId')({
  component: BoardPage,
  validateSearch: (s: Record<string, unknown>): BoardSearch => ({
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
  }),
})

const split = (v?: string): string[] => (v ? v.split(',').filter(Boolean) : [])
const joinOrDrop = (arr: string[]): string | undefined => (arr.length ? arr.join(',') : undefined)

const dayEnd = (offsetDays: number) => {
  const d = new Date()
  d.setDate(d.getDate() + offsetDays)
  d.setHours(23, 59, 59, 999)
  return d.getTime()
}

function matchesDue(t: Task, due: BoardSearch['due']): boolean {
  if (!due) return true
  const open = !['done', 'cancelled'].includes(t.status)
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

function BoardPage() {
  const { boardId } = Route.useParams()
  const navigate = useNavigate()
  const search = Route.useSearch()
  const { data: me } = useSession()
  const { data: boards = [], isLoading } = useBoards()
  const { data: archivedBoards = [] } = useArchivedBoards()
  const board = boards.find((b) => b.id === boardId) ?? archivedBoards.find((b) => b.id === boardId)

  const showArchived = search.archived ?? false
  const { data: allTasks = [], isLoading: tasksLoading } = useBoardTasks(board ? boardId : null, showArchived)
  const { data: fleet, isLoading: fleetLoading } = useAgents()
  const { data: boardCfg, isLoading: cfgLoading } = useBoardAgents(board ? boardId : null)
  const { data: members = [] } = useBoardMembers(board ? boardId : null)
  // Only agents allowed on this board are assignable/filterable here.
  const boardAgents = boardCfg?.allowAll
    ? fleet?.agents ?? []
    : (fleet?.agents ?? []).filter((a) => boardCfg?.models.includes(a.id))
  useBoardLive(board ? boardId : null)

  const qc = useQueryClient()
  const { data: savedViews = [] } = useBoardViews(board ? boardId : null)
  const { openMenu, menu: viewTabMenu } = useContextMenu()
  const [settingsOpen, setSettingsOpen] = useState(false)
  const view = search.view ?? 'board'
  const groupBy: GroupByKey = search.group ?? 'status'
  const q = search.q ?? ''
  const filters: BoardFilters = {
    statuses: split(search.status),
    assignees: split(search.assignee),
    priorities: split(search.priority),
    labels: split(search.label),
    due: search.due ?? '',
  }

  // User picks PUSH (back walks them); typing REPLACEs (no history spam).
  const setSearch = (patch: Partial<BoardSearch>, replace = false) =>
    void navigate({
      to: '/boards/$boardId',
      params: { boardId },
      search: (prev: BoardSearch) => {
        const next = { ...prev, ...patch }
        for (const k of Object.keys(next) as Array<keyof BoardSearch>) {
          if (next[k] === '' || next[k] === false || next[k] === undefined) delete next[k]
        }
        return next
      },
      replace,
    })
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
    void navigate({ to: '/boards/$boardId', params: { boardId }, search: { ...sv.config, v: sv.id } as BoardSearch })
  const saveCurrentAsView = async () => {
    const name = await prompt({ title: 'Save view', message: 'Name this view (current layout, grouping, and filters are captured).', confirmLabel: 'Save' })
    if (!name?.trim()) return
    const r = (await createBoardView(boardId, name.trim(), currentConfig())) as { view?: { id: string } }
    await invalidateViews()
    if (r.view?.id) setSearch({ v: r.view.id })
  }
  const viewTabContextMenu = (e: React.MouseEvent, sv: { id: string; name: string; config: BoardViewConfig }) =>
    openMenu(e, [
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

  // Labels present on this board (for the filter facet).
  const boardLabels = useMemo(() => [...new Set(allTasks.flatMap((t) => t.tags))].sort(), [allTasks])

  const tasks = useMemo(() => {
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
        matchesDue(t, filters.due || undefined),
    )
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [allTasks, q, search.status, search.assignee, search.priority, search.label, search.due])

  // One continuous skeleton across the serial fetch (boards → tasks): the board
  // must not paint with empty columns while its tasks are still in flight.
  // `!showArchived` keeps the gate to the cold-load path — flipping the archived
  // toggle refetches under the toolbar, not under a full-page skeleton.
  if (isLoading || (board && !showArchived && tasksLoading))
    return (
      <div className="grid h-full grid-cols-2 gap-3 overflow-hidden p-6 sm:grid-cols-4">
        {[0, 1, 2, 3].map((c) => (
          <div key={c} className="space-y-3">
            <Skeleton className="h-3 w-20 rounded-full" delay={c * 0.1} />
            {[0, 1, 2].map((r) => (
              <SkeletonCard key={r} delay={c * 0.1 + r * 0.15} />
            ))}
          </div>
        ))}
      </div>
    )
  if (!board) return <EmptyState icon="⧉" title="Board not found" hint="It may have been deleted, or you don’t have access." />

  const toggleCls = (active: boolean) =>
    cn('grid h-7 w-7 place-items-center rounded-md transition-colors', active ? 'bg-card text-fg' : 'text-muted hover:text-fg')
  const openTicket = (taskId: string) => void navigate({ to: '/boards/$boardId/$taskId', params: { boardId, taskId } })
  const canEdit = board.role === 'owner' || board.role === 'editor'

  return (
    <div className="flex h-full min-w-0 flex-col">
      <BoardHeader board={board} onSettings={() => setSettingsOpen(true)} />

      {/* Row 1 — the VIEW: mode toggle, saved views, save. */}
      <div className="flex flex-wrap items-center gap-2 border-b border-line-subtle px-5 py-2">
        <div className="flex rounded-lg border border-line p-0.5">
          <button
            className={toggleCls(view === 'board')}
            onClick={() => setSearch({ view: undefined })}
            title="Board view"
            aria-label="Board view"
          >
            <LayoutGrid size={15} />
          </button>
          <button
            className={toggleCls(view === 'list')}
            onClick={() => setSearch({ view: 'list' })}
            title="List view"
            aria-label="List view"
          >
            <List size={15} />
          </button>
          <button
            className={toggleCls(view === 'gantt')}
            onClick={() => setSearch({ view: 'gantt' })}
            title="Gantt view"
            aria-label="Gantt view"
          >
            <CalendarRange size={15} />
          </button>
        </div>
        {savedViews.length > 0 && <div className="h-5 w-px bg-line-subtle" />}
        {savedViews.map((sv) => {
          // The tab wears its view type: board grid, list, or gantt.
          const TypeIcon = sv.config.view === 'gantt' ? CalendarRange : sv.config.view === 'list' ? List : LayoutGrid
          return (
            <button
              key={sv.id}
              onClick={() => applyView(sv)}
              onContextMenu={(e) => viewTabContextMenu(e, sv)}
              className={cn(
                'flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs transition-colors',
                search.v === sv.id
                  ? 'border-[var(--theme-accent-border)] bg-card text-fg'
                  : 'border-transparent text-muted hover:bg-card hover:text-fg',
              )}
            >
              <TypeIcon size={12} className={cn('shrink-0', search.v === sv.id ? 'text-accent' : 'opacity-70')} />
              {sv.name}
            </button>
          )
        })}
        {canEdit && (
          <button
            onClick={() => void saveCurrentAsView()}
            title="Save the current layout, grouping, and filters as a named view"
            className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-muted transition-colors hover:text-accent"
          >
            <Plus size={12} /> Save view
          </button>
        )}
        <span className="ml-auto text-xs text-muted">
          {filtersActive(filters) || q ? `${tasks.length} of ${allTasks.length}` : `${allTasks.length} tickets`}
        </span>
        {viewTabMenu}
      </div>

      {/* Row 2 — the QUERY: search + filters left; display options right. */}
      <div className="flex flex-wrap items-center gap-2 border-b border-line-subtle px-5 py-1.5">
        <Input value={q} onChange={(e) => setSearch({ q: e.target.value }, true)} placeholder="Search" size="sm" className="w-40" />
        {fleetLoading || cfgLoading ? (
          <Skeleton className="h-9 w-64" />
        ) : (
          <FilterBar value={filters} onChange={setFilters} members={members} agents={boardAgents} labels={boardLabels} meId={me?.id} />
        )}
        <span className="ml-auto flex items-center gap-1.5">
          {view === 'list' && (
            <DropdownMenu
              align="right"
              trigger={(open) => (
                <FieldPill icon={<Layers size={12} />} active={open || groupBy !== 'status'} className="h-9 rounded-lg px-2 text-xs" title="Group by">
                  {groupBy === 'none' ? 'No grouping' : `Group: ${groupBy}`}
                </FieldPill>
              )}
              items={(['status', 'priority', 'assignee', 'label', 'none'] as GroupByKey[]).map((g) => ({
                label: g === 'none' ? 'No grouping' : g,
                checked: groupBy === g,
                onSelect: () => setSearch({ group: g }),
              }))}
            />
          )}
          <Chip onSelect={() => setSearch({ archived: !showArchived })} selected={showArchived} title="Include archived tickets">
            archived
          </Chip>
        </span>
      </div>

      <div className="relative min-h-0 min-w-0 flex-1">
        {view === 'board' ? (
          <Kanban board={board} tasks={tasks} onOpen={openTicket} members={members} />
        ) : view === 'gantt' ? (
          <Gantt board={board} tasks={tasks} onOpen={openTicket} />
        ) : (
          <BoardList tasks={tasks} onOpen={openTicket} boardId={boardId} members={members} canEdit={canEdit} groupBy={groupBy} />
        )}
      </div>

      <BoardSettingsModal
        board={board}
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        onArchived={() => void navigate({ to: '/boards' })}
        onDeleted={() => void navigate({ to: '/boards' })}
      />

      {/* Ticket detail renders here as a nested, directly-linkable route
          (/boards/:boardId/:taskId) — a fixed overlay above the board. */}
      <Outlet />
    </div>
  )
}
