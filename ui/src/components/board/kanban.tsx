import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Input } from '@/components/ui/input'
import { Avatar } from '@/components/ui/avatar'
import { CopyLinkButton } from '@/components/ui/copy-link-button'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import { createTask, updateTask, type Board } from '@/lib/boards'
import { plainText } from '@/lib/plain-text'
import { EFFORT_LABEL, PRIORITY_COLOR, STATUS_LABEL, TASK_STATUSES, type Task, type TaskStatus } from '@/lib/task-const'

const COL_ACCENT: Record<string, string> = {
  inbox: 'var(--theme-muted)',
  assigned: 'var(--theme-accent)',
  in_progress: 'var(--theme-warning)',
  blocked: 'var(--theme-danger)',
  quality_review: 'var(--theme-accent-secondary)',
  done: 'var(--theme-success)',
}

// Kanban board — polished columns, per-column add, drag-and-drop between columns.
// Tasks are passed in (the board page owns fetching + filtering).
export function Kanban({ board, tasks, onOpen }: { board: Board; tasks: Task[]; onOpen: (taskId: string) => void }) {
  const qc = useQueryClient()
  const { data: fleet } = useAgents()
  const agents = fleet?.agents ?? []
  const label = (id: string) => agents.find((a) => a.id === id)?.label ?? id
  const canEdit = board.role === 'owner' || board.role === 'editor'
  const invalidate = () => qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  const [dragOver, setDragOver] = useState<string | null>(null)
  const [dragging, setDragging] = useState<string | null>(null)

  const addTo = async (status: TaskStatus, title: string) => {
    const { task } = await createTask(board.id, { title })
    if (status !== 'inbox') await updateTask(task.id, { status })
    invalidate()
  }
  const move = async (taskId: string, status: TaskStatus) => {
    await updateTask(taskId, { status })
    invalidate()
  }

  return (
    <div className="flex h-full gap-3 overflow-x-auto p-4">
      {TASK_STATUSES.map((col) => {
        const colTasks = tasks.filter((t) => t.status === col)
        return (
          <div
            key={col}
            onDragOver={canEdit ? (e) => { e.preventDefault(); setDragOver(col) } : undefined}
            onDragLeave={() => setDragOver((d) => (d === col ? null : d))}
            onDrop={
              canEdit
                ? (e) => {
                    e.preventDefault()
                    const id = e.dataTransfer.getData('text/task')
                    setDragOver(null)
                    if (id) void move(id, col)
                  }
                : undefined
            }
            className={cn(
              'flex w-80 shrink-0 flex-col rounded-xl bg-sidebar/60 ring-1 ring-transparent transition-shadow',
              dragOver === col && 'ring-[color:var(--theme-accent)]',
            )}
          >
            <div className="flex items-center gap-2 px-3 py-2">
              <span className="h-2 w-2 rounded-full" style={{ background: COL_ACCENT[col] }} />
              <span className="text-xs font-semibold uppercase tracking-wide text-fg">{STATUS_LABEL[col]}</span>
              <span className="text-xs text-muted">{colTasks.length}</span>
            </div>
            <div className="flex-1 space-y-2 overflow-y-auto px-2 pb-2">
              {colTasks.map((t) => (
                <Card
                  key={t.id}
                  task={t}
                  assignees={t.assignees.map(label)}
                  draggable={canEdit}
                  dim={dragging === t.id}
                  onDragStart={(e) => {
                    e.dataTransfer.setData('text/task', t.id)
                    e.dataTransfer.effectAllowed = 'move'
                    setDragging(t.id)
                  }}
                  onDragEnd={() => setDragging(null)}
                  onOpen={() => onOpen(t.id)}
                />
              ))}
              {canEdit && <AddCard onAdd={(title) => addTo(col, title)} />}
            </div>
          </div>
        )
      })}
    </div>
  )
}

function Card({
  task,
  assignees,
  draggable,
  dim,
  onDragStart,
  onDragEnd,
  onOpen,
}: {
  task: Task
  assignees: string[]
  draggable: boolean
  dim: boolean
  onDragStart: (e: React.DragEvent) => void
  onDragEnd: () => void
  onOpen: () => void
}) {
  return (
    <div
      draggable={draggable}
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      className={cn('group relative cursor-grab active:cursor-grabbing', dim && 'opacity-40')}
    >
      <CopyLinkButton
        path={`/boards/${task.boardId}/${task.id}`}
        className="absolute right-2 top-2 z-10 bg-card opacity-0 shadow-[var(--theme-shadow-1)] group-hover:opacity-100"
      />
      <button type="button" onClick={onOpen} className={cn('mercury-panel w-full rounded-xl p-4 text-left transition-shadow hover:shadow-[var(--theme-shadow-3)]', task.archivedAt && 'opacity-60')}>
        <div className="flex items-start gap-2.5">
          <span className="mt-1.5 h-2 w-2 shrink-0 rounded-full" style={{ background: PRIORITY_COLOR[task.priority] }} />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-1.5">
              {task.ticketRef && <span className="font-[var(--font-mono)] text-[11px] text-muted">{task.ticketRef}</span>}
              {task.effort && <span className="rounded border border-line-subtle px-1 text-[9px] font-semibold text-muted">{EFFORT_LABEL[task.effort]}</span>}
              {task.archivedAt && <span className="rounded border border-line-subtle px-1 text-[9px] uppercase tracking-wide text-muted">archived</span>}
            </div>
            <div className="text-[15px] font-medium leading-snug text-fg">{task.title}</div>
            {task.description && <div className="mt-1 line-clamp-3 text-xs leading-relaxed text-muted">{plainText(task.description)}</div>}
          </div>
        </div>
        {(assignees.length > 0 || task.dueDate || task.tags.length > 0) && (
          <div className="mt-3 flex flex-wrap items-center gap-2">
            {assignees.length > 0 && (
              <span className="flex items-center gap-1.5 text-[11px] text-muted">
                <span className="flex -space-x-1.5">
                  {assignees.slice(0, 3).map((a) => (
                    <Avatar key={a} name={a} className="h-5 w-5 ring-2 ring-[color:var(--theme-panel)]" />
                  ))}
                </span>
                {assignees.length === 1 ? assignees[0] : `${assignees.length} agents`}
              </span>
            )}
            {task.dueDate && <span className="text-[11px] text-muted">· {task.dueDate.slice(0, 10)}</span>}
            {task.tags.slice(0, 2).map((tag) => (
              <span key={tag} className="rounded-full border border-line-subtle px-1.5 py-0.5 text-[10px] text-muted">
                {tag}
              </span>
            ))}
          </div>
        )}
      </button>
    </div>
  )
}

function AddCard({ onAdd }: { onAdd: (title: string) => void }) {
  const [open, setOpen] = useState(false)
  const [title, setTitle] = useState('')
  const submit = () => {
    const t = title.trim()
    if (!t) return setOpen(false)
    setTitle('')
    onAdd(t)
  }
  if (!open)
    return (
      <button type="button" onClick={() => setOpen(true)} className="w-full rounded-lg px-2 py-1.5 text-left text-xs text-muted transition-colors hover:bg-card hover:text-fg">
        + Add card
      </button>
    )
  return (
    <Input
      autoFocus
      value={title}
      onChange={(e) => setTitle(e.target.value)}
      onKeyDown={(e) => (e.key === 'Enter' ? submit() : e.key === 'Escape' ? setOpen(false) : null)}
      onBlur={submit}
      placeholder="Card title"
      size="sm"
      className="w-full"
    />
  )
}
