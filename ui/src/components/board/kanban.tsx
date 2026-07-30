import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Archive, ExternalLink, Hash, Link as LinkIcon, MessageSquare, GitBranch } from 'lucide-react'
import { Input } from '@/components/ui/input'
import { Avatar } from '@/components/ui/avatar'
import { CopyLinkButton } from '@/components/ui/copy-link-button'
import { useContextMenu, copyAppLink, type ContextMenuEntry } from '@/components/ui/context-menu'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import { archiveTask, createTask, updateTask, type Board, type BoardMember } from '@/lib/boards'
import { assigneeInfo, type AssigneeInfo } from '@/lib/assignees'
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

const overdue = (t: Task) =>
  !!t.dueDate && new Date(t.dueDate).getTime() < Date.now() && !['done', 'cancelled'].includes(t.status)

/** Compact "4h" / "2.5h" — estimates render short or not at all. */
const fmtHours = (h: number) => `${Number.isInteger(h) ? h : h.toFixed(1)}h`

// Kanban board — polished columns, per-column add, drag-and-drop between columns.
// Tasks are passed in (the board page owns fetching + filtering).
export function Kanban({
  board,
  tasks,
  onOpen,
  members = [],
}: {
  board: Board
  tasks: Task[]
  onOpen: (taskId: string) => void
  members?: BoardMember[]
}) {
  const qc = useQueryClient()
  const { data: fleet } = useAgents()
  const agents = fleet?.agents ?? []
  const canEdit = board.role === 'owner' || board.role === 'editor'
  const invalidate = () => qc.invalidateQueries({ queryKey: ['board-tasks', board.id] })
  const [dragOver, setDragOver] = useState<string | null>(null)
  const [dragging, setDragging] = useState<string | null>(null)
  const { openMenu, menu } = useContextMenu()

  // Sub-task rollup: children by parent id (children render as cards too).
  const childrenOf = new Map<string, Task[]>()
  for (const t of tasks) {
    if (t.parentId) childrenOf.set(t.parentId, [...(childrenOf.get(t.parentId) ?? []), t])
  }
  const parentRef = (t: Task) => {
    if (!t.parentId) return null
    const p = tasks.find((x) => x.id === t.parentId)
    return p ? (p.ticketRef ?? p.title) : null
  }

  // Right-click a card — shortcuts to actions the card/board already offers.
  const cardMenu = (e: React.MouseEvent, t: Task) => {
    const items: ContextMenuEntry[] = [
      { label: 'Open', icon: <ExternalLink size={14} />, onSelect: () => onOpen(t.id) },
      { label: 'Copy link', icon: <LinkIcon size={14} />, onSelect: () => copyAppLink(`/boards/${t.boardId}/${t.id}`) },
    ]
    if (t.ticketRef) {
      const ref = t.ticketRef
      items.push({ label: 'Copy ticket ref', icon: <Hash size={14} />, onSelect: () => void navigator.clipboard.writeText(ref) })
    }
    if (canEdit) {
      items.push('sep', {
        label: t.archivedAt ? 'Unarchive' : 'Archive',
        icon: <Archive size={14} />,
        danger: !t.archivedAt,
        onSelect: () => {
          void archiveTask(t.id, !t.archivedAt).then(invalidate)
        },
      })
    }
    openMenu(e, items)
  }

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
        // Column estimate rollup — visible planning weight per column.
        const colHours = colTasks.reduce((s, t) => s + (t.estimatedHours ?? 0), 0)
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
              {colHours > 0 && (
                <span className="ml-auto text-[10px] text-muted" title="Total estimated hours in this column">
                  Σ {fmtHours(colHours)}
                </span>
              )}
            </div>
            <div className="flex-1 space-y-2 overflow-y-auto px-2 pb-2">
              {colTasks.map((t) => (
                <Card
                  key={t.id}
                  task={t}
                  assignees={t.assignees.map((a) => assigneeInfo(a, agents, members))}
                  subtasks={childrenOf.get(t.id) ?? []}
                  parentRef={parentRef(t)}
                  draggable={canEdit}
                  dim={dragging === t.id}
                  onDragStart={(e) => {
                    e.dataTransfer.setData('text/task', t.id)
                    e.dataTransfer.effectAllowed = 'move'
                    setDragging(t.id)
                  }}
                  onDragEnd={() => setDragging(null)}
                  onOpen={() => onOpen(t.id)}
                  onContextMenu={(e) => cardMenu(e, t)}
                />
              ))}
              {canEdit && <AddCard onAdd={(title) => addTo(col, title)} />}
            </div>
          </div>
        )
      })}
      {menu}
    </div>
  )
}

function Card({
  task,
  assignees,
  subtasks,
  parentRef,
  draggable,
  dim,
  onDragStart,
  onDragEnd,
  onOpen,
  onContextMenu,
}: {
  task: Task
  assignees: AssigneeInfo[]
  subtasks: Task[]
  parentRef: string | null
  draggable: boolean
  dim: boolean
  onDragStart: (e: React.DragEvent) => void
  onDragEnd: () => void
  onOpen: () => void
  onContextMenu: (e: React.MouseEvent) => void
}) {
  const late = overdue(task)
  const doneKids = subtasks.filter((s) => s.status === 'done').length
  return (
    <div
      draggable={draggable}
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      onContextMenu={onContextMenu}
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
              {parentRef && (
                <span className="truncate text-[10px] text-muted" title={`Sub-task of ${parentRef}`}>
                  ↳ {parentRef}
                </span>
              )}
              {task.effort && <span className="rounded border border-line-subtle px-1 text-[9px] font-semibold text-muted">{EFFORT_LABEL[task.effort]}</span>}
              {task.estimatedHours != null && (
                <span className="rounded border border-line-subtle px-1 text-[9px] font-semibold text-muted" title="Estimate">
                  {fmtHours(task.estimatedHours)}
                </span>
              )}
              {task.archivedAt && <span className="rounded border border-line-subtle px-1 text-[9px] uppercase tracking-wide text-muted">archived</span>}
            </div>
            <div className="font-sans text-[15px] font-medium leading-snug text-fg">{task.title}</div>
            {task.description && <div className="mt-1 line-clamp-3 font-sans text-xs leading-relaxed text-muted">{plainText(task.description)}</div>}
          </div>
        </div>
        {(assignees.length > 0 || task.dueDate || task.tags.length > 0 || subtasks.length > 0 || task.commentCount > 0) && (
          <div className="mt-3 flex flex-wrap items-center gap-2">
            {assignees.length > 0 && (
              <span className="flex items-center gap-1.5 text-[11px] text-muted">
                <span className="flex -space-x-1.5">
                  {assignees.slice(0, 3).map((a) => (
                    <Avatar key={a.key} name={a.label} className="h-5 w-5 ring-2 ring-[color:var(--theme-panel)]" />
                  ))}
                </span>
                {assignees.length === 1
                  ? assignees[0]!.label
                  : assignees.length <= 3
                    ? `${assignees.length} assignees`
                    : `+${assignees.length - 3} · ${assignees.length} assignees`}
              </span>
            )}
            {task.dueDate && (
              <span
                className={cn('text-[11px]', late ? 'font-medium text-[color:var(--theme-danger)]' : 'text-muted')}
                title={late ? 'Past due' : 'Due date'}
              >
                · {task.dueDate.slice(0, 10)}
              </span>
            )}
            {subtasks.length > 0 && (
              <span
                className={cn('flex items-center gap-1 text-[11px]', doneKids === subtasks.length ? 'text-[color:var(--theme-success)]' : 'text-muted')}
                title="Sub-tasks done"
              >
                <GitBranch size={11} /> {doneKids}/{subtasks.length}
              </span>
            )}
            {task.commentCount > 0 && (
              <span className="flex items-center gap-1 text-[11px] text-muted" title="Comments">
                <MessageSquare size={11} /> {task.commentCount}
              </span>
            )}
            {task.tags.slice(0, 2).map((tag) => (
              <span key={tag} className="rounded-full border border-line-subtle px-1.5 py-0.5 text-[10px] text-muted">
                {tag}
              </span>
            ))}
            {task.tags.length > 2 && <span className="text-[10px] text-muted">+{task.tags.length - 2}</span>}
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
