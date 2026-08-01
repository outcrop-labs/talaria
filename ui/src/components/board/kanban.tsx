import { useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { MessageSquare, GitBranch } from 'lucide-react'
import { ticketMenuEntries } from '@/components/board/ticket-menu'
import { Input } from '@/components/ui/input'
import { CopyLinkButton } from '@/components/ui/copy-link-button'
import { useContextMenu } from '@/components/ui/context-menu'
import { AssigneesPill, DuePill, EstimatePill, LabelsPill, LABEL_CSS, type PillCtx } from '@/components/board/field-pills'
import { cn } from '@/lib/cn'
import { useAgents } from '@/lib/agents'
import { archiveTask, createTask, updateTask, useBoardLabels, type Board, type BoardMember } from '@/lib/boards'
import { plainText } from '@/lib/plain-text'
import { EFFORT_LABEL, PRIORITY_COLOR, STATUS_LABEL, TASK_STATUSES, type Task, type TaskStatus } from '@/lib/task-const'
import { statusColorOf, useBoardStatuses } from '@/lib/statuses'
import { useSession } from '@/lib/session'

const COL_ACCENT: Record<string, string> = {
  inbox: 'var(--theme-muted)',
  assigned: 'var(--theme-accent)',
  in_progress: 'var(--theme-warning)',
  blocked: 'var(--theme-danger)',
  quality_review: 'var(--theme-accent-secondary)',
  done: 'var(--theme-success)',
}

/** Compact "4h" / "2.5h" — estimates render short or not at all. Coerces
 *  defensively: numeric columns can surface as strings at the API edge. */
const fmtHours = (h: number | string) => {
  const n = Number(h)
  if (!Number.isFinite(n)) return null
  return `${Number.isInteger(n) ? n : n.toFixed(1)}h`
}

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
  const { data: me } = useSession()
  const { data: boardLabels = [] } = useBoardLabels(board.id)
  const { data: boardStatuses = [] } = useBoardStatuses(board.id)
  // Columns: the board's status set (custom or defaults), plus the legacy
  // terminal extras only when occupied.
  const columns = boardStatuses.length
    ? [
        ...boardStatuses.map((st) => ({ key: st.key, label: st.label, color: statusColorOf(st.key, boardStatuses) })),
        ...(['failed', 'cancelled'] as const)
          .filter((k) => tasks.some((t) => t.status === k))
          .map((k) => ({ key: k as string, label: STATUS_LABEL[k] ?? k, color: COL_ACCENT[k] ?? 'var(--theme-muted)' })),
      ]
    : TASK_STATUSES.map((k) => ({ key: k as string, label: STATUS_LABEL[k] ?? k, color: COL_ACCENT[k] ?? 'var(--theme-muted)' }))
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

  const patch = async (taskId: string, p: Parameters<typeof updateTask>[1]) => {
    await updateTask(taskId, p)
    invalidate()
  }

  // Right-click a card — the shared ticket menu (quick controls + shortcuts).
  const cardMenu = (e: React.MouseEvent, t: Task) =>
    openMenu(
      e,
      ticketMenuEntries(t, {
        canEdit,
        meId: me?.id,
        statuses: boardStatuses,
        onOpen: () => onOpen(t.id),
        onPatch: (p) => void patch(t.id, p),
        onArchive: () => void archiveTask(t.id, !t.archivedAt).then(invalidate),
      }),
    )

  const addTo = async (status: TaskStatus, title: string) => {
    const { task } = (await createTask(board.id, { title })) as { task: Task }
    if (task.status !== status) await updateTask(task.id, { status })
    invalidate()
  }
  const move = async (taskId: string, status: TaskStatus) => {
    await updateTask(taskId, { status })
    invalidate()
  }

  return (
    <div className="flex h-full gap-3 overflow-x-auto p-4">
      {columns.map((col) => {
        const colTasks = tasks.filter((t) => t.status === col.key)
        // Column estimate rollup — visible planning weight per column.
        const colHours = colTasks.reduce((s, t) => {
          const n = Number(t.estimatedHours ?? 0)
          return s + (Number.isFinite(n) ? n : 0)
        }, 0)
        return (
          <div
            key={col.key}
            onDragOver={canEdit ? (e) => { e.preventDefault(); setDragOver(col.key) } : undefined}
            onDragLeave={() => setDragOver((d) => (d === col.key ? null : d))}
            onDrop={
              canEdit
                ? (e) => {
                    e.preventDefault()
                    const id = e.dataTransfer.getData('text/task')
                    setDragOver(null)
                    if (id) void move(id, col.key as TaskStatus)
                  }
                : undefined
            }
            className={cn(
              'flex w-80 shrink-0 flex-col rounded-lg bg-panel/60 ring-1 ring-transparent transition-shadow',
              dragOver === col.key && 'ring-accent',
            )}
          >
            <div className="flex items-center gap-2 px-3 py-2">
              <span className="h-1.5 w-1.5 rounded-full" style={{ background: col.color }} />
              <span className="font-mono text-[10px] font-medium uppercase tracking-[0.08em] text-ink-dim">{col.label}</span>
              <span className="font-mono text-[10px] tracking-[0.05em] text-muted">{colTasks.length}</span>
              {colHours > 0 && (
                <span className="ml-auto font-mono text-[10px] tracking-[0.05em] text-muted" title="Total estimated hours in this column">
                  Σ {fmtHours(colHours)}
                </span>
              )}
            </div>
            <div className="flex-1 space-y-2 overflow-y-auto px-2 pb-2">
              {colTasks.map((t) => (
                <Card
                  key={t.id}
                  task={t}
                  pillCtx={{ canEdit, onPatch: (p) => void patch(t.id, p), agents, members, meId: me?.id, labels: boardLabels, statuses: boardStatuses, boardId: board.id }}
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
              {canEdit && <AddCard onAdd={(title) => addTo(col.key as TaskStatus, title)} />}
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
  pillCtx,
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
  pillCtx: PillCtx
  subtasks: Task[]
  parentRef: string | null
  draggable: boolean
  dim: boolean
  onDragStart: (e: React.DragEvent) => void
  onDragEnd: () => void
  onOpen: () => void
  onContextMenu: (e: React.MouseEvent) => void
}) {
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
        className="absolute right-2 top-2 z-10 bg-raised opacity-0 shadow-[var(--theme-shadow-1)] group-hover:opacity-100"
      />
      {/* div, not <button>: the pills inside are buttons themselves. */}
      <div
        role="button"
        tabIndex={0}
        onClick={onOpen}
        onKeyDown={(e) => e.key === 'Enter' && e.target === e.currentTarget && onOpen()}
        className={cn(
          // §8 panel: panel surface + hairline + radius 8; hover raises the
          // hairline (matte — no glow, ~120ms).
          'relative w-full cursor-pointer overflow-hidden rounded-lg border border-line bg-panel p-4 text-left shadow-[var(--theme-shadow-1)] transition-colors hover:border-line-strong',
          task.archivedAt && 'opacity-60',
        )}
      >
        {/* Color-code stripe (ticket color, when set). */}
        {task.color && <span className="absolute inset-y-0 left-0 w-1" style={{ background: LABEL_CSS[task.color] }} />}
        <div className="flex items-start gap-2.5">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full" style={{ background: PRIORITY_COLOR[task.priority] }} />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-1.5">
              {task.ticketRef && <span className="font-mono text-[11px] tracking-[0.05em] text-muted">{task.ticketRef}</span>}
              {parentRef && (
                <span className="truncate font-mono text-[10px] text-muted" title={`Sub-task of ${parentRef}`}>
                  ↳ {parentRef}
                </span>
              )}
              {task.effort && <span className="rounded border border-line-subtle px-1 font-mono text-[9px] font-medium uppercase tracking-[0.05em] text-muted">{EFFORT_LABEL[task.effort]}</span>}
              {task.estimatedHours != null && fmtHours(task.estimatedHours) != null && (
                <span className="rounded border border-line-subtle px-1 font-mono text-[9px] font-medium tracking-[0.05em] text-muted" title="Estimate">
                  {fmtHours(task.estimatedHours)}
                </span>
              )}
              {task.archivedAt && <span className="rounded border border-line-subtle px-1 font-mono text-[9px] uppercase tracking-[0.05em] text-muted">archived</span>}
            </div>
            <div className="font-sans text-[15px] font-medium leading-snug text-fg">{task.title}</div>
            {task.description && <div className="mt-1 line-clamp-3 font-sans text-xs leading-relaxed text-muted">{plainText(task.description)}</div>}
          </div>
        </div>
        {/* Property pills — PERSISTENT controls: bordered, labeled, chevroned.
            What you can change is never a mystery. */}
        <div className="mt-2.5 flex flex-wrap items-center gap-1">
          <AssigneesPill t={task} ctx={pillCtx} persistent />
          <DuePill t={task} ctx={pillCtx} persistent />
          <EstimatePill t={task} ctx={pillCtx} persistent />
          <LabelsPill t={task} ctx={pillCtx} persistent />
          <span className="ml-auto flex shrink-0 items-center gap-1.5">
            {subtasks.length > 0 && (
              <span
                className={cn('flex items-center gap-1 font-mono text-[11px] tracking-[0.05em]', doneKids === subtasks.length ? 'text-success' : 'text-muted')}
                title="Sub-tasks done"
              >
                <GitBranch size={11} /> {doneKids}/{subtasks.length}
              </span>
            )}
            {task.commentCount > 0 && (
              <span className="flex items-center gap-1 font-mono text-[11px] tracking-[0.05em] text-muted" title="Comments">
                <MessageSquare size={11} /> {task.commentCount}
              </span>
            )}
          </span>
        </div>
      </div>
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
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="w-full rounded-md px-2 py-1.5 text-left font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:bg-hover hover:text-fg"
      >
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
