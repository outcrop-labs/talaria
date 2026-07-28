import { createFileRoute, useNavigate, Outlet } from '@tanstack/react-router'
import { Skeleton, SkeletonCard } from '@/components/ui/skeleton'
import { useMemo, useState } from 'react'
import { LayoutGrid, List } from 'lucide-react'
import { BoardHeader } from '@/components/board/board-header'
import { Kanban } from '@/components/board/kanban'
import { BoardList } from '@/components/board/board-list'
import { BoardSettingsModal } from '@/components/board/board-settings-modal'
import { EmptyState } from '@/components/ui/empty-state'
import { Input } from '@/components/ui/input'
import { Select } from '@/components/ui/select'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import { useBoards, useArchivedBoards, useBoardTasks, useBoardLive, useBoardAgents } from '@/lib/boards'
import { PRIORITIES } from '@/lib/task-const'

export const Route = createFileRoute('/_app/boards/$boardId')({
  component: BoardPage,
})

function BoardPage() {
  const { boardId } = Route.useParams()
  const navigate = useNavigate()
  const { data: boards = [], isLoading } = useBoards()
  const { data: archivedBoards = [] } = useArchivedBoards()
  const board = boards.find((b) => b.id === boardId) ?? archivedBoards.find((b) => b.id === boardId)

  const [showArchived, setShowArchived] = useState(false)
  const { data: allTasks = [] } = useBoardTasks(board ? boardId : null, showArchived)
  const { data: fleet } = useAgents()
  const { data: boardCfg } = useBoardAgents(board ? boardId : null)
  // Only agents allowed on this board are assignable/filterable here.
  const boardAgents = boardCfg?.allowAll
    ? fleet?.agents ?? []
    : (fleet?.agents ?? []).filter((a) => boardCfg?.models.includes(a.id))
  useBoardLive(board ? boardId : null)

  const [settingsOpen, setSettingsOpen] = useState(false)
  const [view, setView] = useState<'board' | 'list'>('board')
  const [q, setQ] = useState('')
  const [assignee, setAssignee] = useState('')
  const [priority, setPriority] = useState('')

  const tasks = useMemo(() => {
    const query = q.trim().toLowerCase()
    return allTasks.filter(
      (t) =>
        (!query || t.title.toLowerCase().includes(query) || (t.ticketRef ?? '').toLowerCase().includes(query)) &&
        (!assignee || t.assignees.includes(assignee)) &&
        (!priority || t.priority === priority),
    )
  }, [allTasks, q, assignee, priority])

  if (isLoading)
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

  return (
    <div className="flex h-full min-w-0 flex-col">
      <BoardHeader board={board} onSettings={() => setSettingsOpen(true)} />

      {/* Toolbar: view toggle + filters */}
      <div className="flex flex-wrap items-center gap-2 border-b border-line-subtle px-5 py-2">
        <div className="flex rounded-lg border border-line p-0.5">
          <button className={toggleCls(view === 'board')} onClick={() => setView('board')} title="Board view" aria-label="Board view">
            <LayoutGrid size={15} />
          </button>
          <button className={toggleCls(view === 'list')} onClick={() => setView('list')} title="List view" aria-label="List view">
            <List size={15} />
          </button>
        </div>
        <Input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search" size="sm" className="w-44" />
        <Select value={assignee} onChange={(e) => setAssignee(e.target.value)} size="sm">
          <option value="">Any assignee</option>
          {boardAgents.map((a) => (
            <option key={a.id} value={a.id}>{a.label}</option>
          ))}
        </Select>
        <Select value={priority} onChange={(e) => setPriority(e.target.value)} size="sm">
          <option value="">Any priority</option>
          {PRIORITIES.map((p) => (
            <option key={p} value={p}>{p}</option>
          ))}
        </Select>
        <label className="flex cursor-pointer items-center gap-1.5 text-xs text-muted">
          <input
            type="checkbox"
            checked={showArchived}
            onChange={(e) => setShowArchived(e.target.checked)}
            className="accent-[color:var(--theme-accent)]"
          />
          Show archived
        </label>
        <span className="ml-auto text-xs text-muted">{tasks.length} of {allTasks.length}</span>
      </div>

      <div className="min-h-0 min-w-0 flex-1">
        {view === 'board' ? (
          <Kanban board={board} tasks={tasks} onOpen={openTicket} />
        ) : (
          <BoardList tasks={tasks} onOpen={openTicket} boardId={boardId} />
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
