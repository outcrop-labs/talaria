import { createFileRoute, useNavigate, Outlet } from '@tanstack/react-router'
import { Skeleton, SkeletonCard } from '@/components/ui/skeleton'
import { useMemo } from 'react'
import { LayoutGrid, List } from 'lucide-react'
import { BoardHeader } from '@/components/board/board-header'
import { Kanban } from '@/components/board/kanban'
import { BoardList } from '@/components/board/board-list'
import { BoardSettingsModal } from '@/components/board/board-settings-modal'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { Checkbox } from '@/components/ui/checkbox'
import { Chip } from '@/components/ui/chip'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import { useBoards, useArchivedBoards, useBoardTasks, useBoardLive, useBoardAgents, useBoardMembers } from '@/lib/boards'
import { useSession } from '@/lib/session'
import { userAssignee } from '@/lib/assignees'
import { PRIORITIES } from '@/lib/task-const'
import { useState } from 'react'

// Filters live in the URL (deep-link convention): /boards/x?view=list&q=…
interface BoardSearch {
  view?: 'list'
  q?: string
  assignee?: string
  priority?: string
  label?: string
  overdue?: boolean
  archived?: boolean
  settings?: boolean
}

export const Route = createFileRoute('/_app/boards/$boardId')({
  component: BoardPage,
  validateSearch: (s: Record<string, unknown>): BoardSearch => ({
    ...(s.view === 'list' ? { view: 'list' as const } : {}),
    ...(typeof s.q === 'string' && s.q ? { q: s.q } : {}),
    ...(typeof s.assignee === 'string' && s.assignee ? { assignee: s.assignee } : {}),
    ...(typeof s.priority === 'string' && s.priority ? { priority: s.priority } : {}),
    ...(typeof s.label === 'string' && s.label ? { label: s.label } : {}),
    ...(s.overdue === true ? { overdue: true } : {}),
    ...(s.archived === true ? { archived: true } : {}),
  }),
})

const isOverdue = (t: { dueDate: string | null; status: string }) =>
  !!t.dueDate && new Date(t.dueDate).getTime() < Date.now() && !['done', 'cancelled'].includes(t.status)

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

  const [settingsOpen, setSettingsOpen] = useState(false)
  const view = search.view ?? 'board'
  const q = search.q ?? ''
  const assignee = search.assignee ?? ''
  const priority = search.priority ?? ''
  const label = search.label ?? ''
  const overdue = search.overdue ?? false

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

  // Labels present on this board (for the filter select).
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
        (!assignee || t.assignees.includes(assignee)) &&
        (!priority || t.priority === priority) &&
        (!label || t.tags.includes(label)) &&
        (!overdue || isOverdue(t)),
    )
  }, [allTasks, q, assignee, priority, label, overdue])

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
  const meAssignee = me ? userAssignee(me.id) : ''

  return (
    <div className="flex h-full min-w-0 flex-col">
      <BoardHeader board={board} onSettings={() => setSettingsOpen(true)} />

      {/* Toolbar: view toggle + filters (all URL-backed) */}
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
        </div>
        <Input value={q} onChange={(e) => setSearch({ q: e.target.value }, true)} placeholder="Search" size="sm" className="w-44" />
        {fleetLoading || cfgLoading ? (
          <Skeleton className="h-9 w-32" />
        ) : (
          <Select value={assignee} onChange={(e) => setSearch({ assignee: e.target.value })} size="sm">
            <option value="">Any assignee</option>
            {me && <option value={meAssignee}>Me</option>}
            {members
              .filter((m) => m.userId !== me?.id)
              .map((m) => (
                <option key={m.userId} value={userAssignee(m.userId)}>
                  {m.name ?? m.email}
                </option>
              ))}
            {boardAgents.map((a) => (
              <option key={a.id} value={a.id}>
                {a.label}
              </option>
            ))}
          </Select>
        )}
        <Select value={priority} onChange={(e) => setSearch({ priority: e.target.value })} size="sm">
          <option value="">Any priority</option>
          {PRIORITIES.map((p) => (
            <option key={p} value={p}>
              {p}
            </option>
          ))}
        </Select>
        {boardLabels.length > 0 && (
          <Select value={label} onChange={(e) => setSearch({ label: e.target.value })} size="sm">
            <option value="">Any label</option>
            {boardLabels.map((l) => (
              <option key={l} value={l}>
                {l}
              </option>
            ))}
          </Select>
        )}
        <Chip onSelect={() => setSearch({ overdue: !overdue })} selected={overdue} title="Only tickets past their due date">
          overdue
        </Chip>
        <Checkbox checked={showArchived} onChange={(v) => setSearch({ archived: v })} label="Show archived" />
        <span className="ml-auto text-xs text-muted">
          {tasks.length} of {allTasks.length}
        </span>
      </div>

      <div className="min-h-0 min-w-0 flex-1">
        {view === 'board' ? (
          <Kanban board={board} tasks={tasks} onOpen={openTicket} members={members} />
        ) : (
          <BoardList
            tasks={tasks}
            onOpen={openTicket}
            boardId={boardId}
            members={members}
            canEdit={board.role === 'owner' || board.role === 'editor'}
          />
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
