import { useMemo, useState } from 'react'
import { useQueries } from '@tanstack/react-query'
import { Link } from '@tanstack/react-router'
import { FolderKanban, ListTodo, Search, X } from 'lucide-react'
import { useBoards, type Board } from '@/lib/boards'
import type { Task } from '@/lib/task-const'
import { cn } from '@/lib/cn'

interface IndexedTask extends Task {
  boardName: string
}

const DONE_STATUSES = new Set(['done', 'completed', 'cancelled'])

const isDone = (task: Task) => task.completedAt !== null || DONE_STATUSES.has(task.status)

async function fetchBoardTasks(board: Board): Promise<IndexedTask[]> {
  const response = await fetch(`/api/boards/${board.id}/tasks`, { credentials: 'same-origin' })
  if (!response.ok) return []
  const data = (await response.json()) as { tasks?: Task[] }
  return (data.tasks ?? []).map((task) => ({ ...task, boardName: board.name }))
}

export function SidebarWorkOverview() {
  const [query, setQuery] = useState('')
  const { data: boards = [], isLoading: boardsLoading } = useBoards()
  const taskQueries = useQueries({
    queries: boards.map((board) => ({
      queryKey: ['board-tasks', board.id, false],
      queryFn: () => fetchBoardTasks(board),
      staleTime: 30_000,
    })),
  })

  const tasks = useMemo(() => taskQueries.flatMap((result) => result.data ?? []), [taskQueries])
  const loading = boardsLoading || taskQueries.some((result) => result.isLoading)
  const activeProjects = boards.filter((board) => tasks.some((task) => task.boardId === board.id && !isDone(task))).length
  const completedTasks = tasks.filter(isDone).length
  const normalizedQuery = query.trim().toLowerCase()

  const projectMatches = normalizedQuery
    ? boards.filter((board) => board.name.toLowerCase().includes(normalizedQuery)).slice(0, 3)
    : []
  const taskMatches = normalizedQuery
    ? tasks
        .filter(
          (task) =>
            task.title.toLowerCase().includes(normalizedQuery) || task.boardName.toLowerCase().includes(normalizedQuery),
        )
        .slice(0, Math.max(0, 6 - projectMatches.length))
    : []

  return (
    <section aria-label="Project and task overview" className="mt-5 shrink-0 border-b border-line-subtle pb-4">
      <div className="relative">
        <Search
          size={14}
          strokeWidth={1.7}
          className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted"
          aria-hidden
        />
        <input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Escape') setQuery('')
          }}
          aria-label="Search projects and tasks"
          placeholder="Search projects & tasks"
          className="h-9 w-full rounded-lg border border-line bg-raised pl-9 pr-8 font-sans text-xs text-fg outline-none transition-colors placeholder:text-muted hover:border-line-strong focus:border-[color:var(--theme-accent-border)] focus:ring-1 focus:ring-[color:var(--theme-accent-border)]"
        />
        {query && (
          <button
            type="button"
            onClick={() => setQuery('')}
            aria-label="Clear search"
            className="absolute right-2 top-1/2 grid h-5 w-5 -translate-y-1/2 place-items-center rounded text-muted transition-colors hover:bg-hover hover:text-fg"
          >
            <X size={12} strokeWidth={1.7} />
          </button>
        )}
      </div>

      {normalizedQuery ? (
        <SearchResults
          query={query}
          projects={projectMatches}
          tasks={taskMatches}
          loading={loading}
          onOpen={() => setQuery('')}
        />
      ) : (
        <div className="mt-3 space-y-0.5">
          <SummaryRow
            to="projects"
            icon={<FolderKanban size={16} strokeWidth={1.5} />}
            label="Projects"
            value={activeProjects}
            total={boards.length}
            segments={3}
            loading={loading}
            hint={`${activeProjects} active of ${boards.length} visible projects`}
          />
          <SummaryRow
            to="tasks"
            icon={<ListTodo size={16} strokeWidth={1.5} />}
            label="Tasks"
            value={completedTasks}
            total={tasks.length}
            segments={10}
            loading={loading}
            hint={`${completedTasks} completed of ${tasks.length} visible tasks`}
          />
        </div>
      )}
    </section>
  )
}

function SummaryRow({
  to,
  icon,
  label,
  value,
  total,
  segments,
  loading,
  hint,
}: {
  to: 'projects' | 'tasks'
  icon: React.ReactNode
  label: string
  value: number
  total: number
  segments: number
  loading: boolean
  hint: string
}) {
  const content = (
    <>
      <span className="grid h-4 w-4 shrink-0 place-items-center text-muted">{icon}</span>
      <span className="min-w-0 flex-1 truncate font-sans text-[13px] text-fg">{label}</span>
      <SegmentMeter value={value} total={total} segments={segments} />
      <span className="w-[34px] shrink-0 text-right font-mono text-[10px] tracking-[0.02em] text-muted">
        {loading ? '—' : `${value}/${total}`}
      </span>
    </>
  )
  const className =
    'flex h-8 w-full items-center gap-2 rounded-md px-2 text-left transition-colors duration-[120ms] hover:bg-hover'

  if (to === 'projects')
    return (
      <Link to="/boards" className={className} title={hint}>
        {content}
      </Link>
    )

  return (
    <Link to="/" search={{ tab: 'boards' }} className={className} title={hint}>
      {content}
    </Link>
  )
}

function SegmentMeter({ value, total, segments }: { value: number; total: number; segments: number }) {
  const filled = total > 0 ? Math.max(value > 0 ? 1 : 0, Math.round((value / total) * segments)) : 0
  return (
    <span className="flex h-4 shrink-0 items-center gap-[2px]" aria-hidden>
      {Array.from({ length: segments }, (_, index) => (
        <span
          key={index}
          className={cn(
            'h-3 w-[3px] rounded-[1px] bg-line-strong transition-colors',
            index < filled && 'bg-[#9d87ff] shadow-[0_0_5px_rgba(157,135,255,0.22)]',
          )}
        />
      ))}
    </span>
  )
}

function SearchResults({
  query,
  projects,
  tasks,
  loading,
  onOpen,
}: {
  query: string
  projects: Board[]
  tasks: IndexedTask[]
  loading: boolean
  onOpen: () => void
}) {
  if (loading)
    return <div className="px-2 pt-3 font-mono text-[9px] uppercase tracking-[0.08em] text-muted">Searching…</div>

  if (projects.length === 0 && tasks.length === 0)
    return (
      <div className="px-2 pt-3 font-sans text-[11px] leading-4 text-muted">
        No projects or tasks match “{query.trim()}”.
      </div>
    )

  return (
    <div className="mt-2 max-h-52 space-y-0.5 overflow-y-auto">
      {projects.map((board) => (
        <Link
          key={board.id}
          to="/boards/$boardId"
          params={{ boardId: board.id }}
          onClick={onOpen}
          className="flex min-h-8 items-center gap-2 rounded-md px-2 py-1 text-muted transition-colors hover:bg-hover hover:text-fg"
        >
          <FolderKanban size={14} strokeWidth={1.5} className="shrink-0" />
          <span className="min-w-0 flex-1 truncate font-sans text-[12px]">{board.name}</span>
          <span className="font-mono text-[9px] uppercase tracking-[0.06em] text-ink-dim">Project</span>
        </Link>
      ))}
      {tasks.map((task) => (
        <Link
          key={task.id}
          to="/boards/$boardId/$taskId"
          params={{ boardId: task.boardId, taskId: task.id }}
          onClick={onOpen}
          className="flex min-h-8 items-start gap-2 rounded-md px-2 py-1.5 text-muted transition-colors hover:bg-hover hover:text-fg"
        >
          <ListTodo size={14} strokeWidth={1.5} className="mt-px shrink-0" />
          <span className="min-w-0 flex-1">
            <span className="block truncate font-sans text-[12px] text-fg">{task.title}</span>
            <span className="block truncate font-mono text-[9px] uppercase tracking-[0.05em] text-ink-dim">
              {task.boardName}
            </span>
          </span>
        </Link>
      ))}
    </div>
  )
}
