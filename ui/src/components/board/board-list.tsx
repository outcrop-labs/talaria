import { useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useQueryClient } from '@tanstack/react-query'
import { SlidersHorizontal, ChevronUp, ChevronDown, GripVertical } from 'lucide-react'
import { useAgents } from '@/lib/agents'
import { archiveTask, updateTask, useBoardLabels, type BoardMember } from '@/lib/boards'
import { assigneeInfo } from '@/lib/assignees'
import { ticketMenuEntries } from '@/components/board/ticket-menu'
import { AssigneesPill, DuePill, EstimatePill, LabelsPill, PriorityPill, StatusPill, LABEL_CSS } from '@/components/board/field-pills'
import { statusColorOf, statusLabelOf, useBoardStatuses } from '@/lib/statuses'
import { FieldPill } from '@/components/ui/field-pill'
import { useSession } from '@/lib/session'
import { userAssignee } from '@/lib/assignees'
import { CopyLinkButton } from '@/components/ui/copy-link-button'
import { useContextMenu, DropdownMenu } from '@/components/ui/context-menu'
import { Skeleton } from '@/components/ui/skeleton'
import { QueryError } from '@/components/ui/query-state'
import { cn } from '@/lib/cn'
import { EFFORT_LABEL, PRIORITIES, PRIORITY_COLOR, TASK_STATUSES, type Priority, type Task, type TaskStatus } from '@/lib/task-const'
import { relativeTime } from '@/lib/fleet'

type ColumnKey = 'ticket' | 'title' | 'status' | 'priority' | 'effort' | 'estimate' | 'assignees' | 'due' | 'time' | 'labels' | 'updated' | 'created'
interface ListColumn {
  key: ColumnKey
  label: string
  align?: 'right'
  default?: boolean
  fixed?: boolean // always shown (can't be toggled off)
}

const LIST_COLUMNS: ListColumn[] = [
  { key: 'ticket', label: 'Ticket', default: true },
  { key: 'title', label: 'Title', default: true, fixed: true },
  { key: 'status', label: 'Status', default: true },
  { key: 'priority', label: 'Priority', default: true },
  { key: 'effort', label: 'Effort' },
  { key: 'estimate', label: 'Est.', align: 'right' },
  { key: 'assignees', label: 'Assignees', default: true },
  { key: 'due', label: 'Due' },
  { key: 'time', label: 'Time', align: 'right' },
  { key: 'labels', label: 'Labels', default: true },
  { key: 'updated', label: 'Updated', align: 'right', default: true },
  { key: 'created', label: 'Created', align: 'right' },
]
const ALL_KEYS = LIST_COLUMNS.map((c) => c.key)
const DEFAULT_COLUMNS = LIST_COLUMNS.filter((c) => c.default).map((c) => c.key)
const storeKey = (boardId: string) => `talaria:list-cols:${boardId}`
const sortKey = (boardId: string) => `talaria:list-sort:${boardId}`
const orderKey = (boardId: string) => `talaria:list-order:${boardId}`

/** Stored order, merged with any columns added since (appended, none dropped). */
function loadOrder(boardId: string): ColumnKey[] {
  try {
    const raw = localStorage.getItem(orderKey(boardId))
    if (raw) {
      const arr = JSON.parse(raw) as unknown
      if (Array.isArray(arr)) {
        const kept = arr.filter((k): k is ColumnKey => ALL_KEYS.includes(k as ColumnKey))
        return [...kept, ...ALL_KEYS.filter((k) => !kept.includes(k))]
      }
    }
  } catch {
    /* ignore */
  }
  return ALL_KEYS
}

const PRIORITY_RANK: Record<string, number> = { low: 0, medium: 1, high: 2, urgent: 3 }
const EFFORT_RANK: Record<string, number> = { xs: 0, s: 1, m: 2, l: 3, xl: 4 }

type SortDir = 'asc' | 'desc'
interface SortState {
  key: ColumnKey | null
  dir: SortDir
}

function loadSort(boardId: string): SortState {
  try {
    const raw = localStorage.getItem(sortKey(boardId))
    if (raw) {
      const s = JSON.parse(raw) as SortState
      if (s && (s.key === null || LIST_COLUMNS.some((c) => c.key === s.key))) return s
    }
  } catch {
    /* ignore */
  }
  return { key: null, dir: 'asc' }
}

function loadColumns(boardId: string): ColumnKey[] {
  try {
    const raw = localStorage.getItem(storeKey(boardId))
    if (raw) {
      const arr = JSON.parse(raw) as unknown
      if (Array.isArray(arr)) return arr.filter((k): k is ColumnKey => LIST_COLUMNS.some((c) => c.key === k))
    }
  } catch {
    /* ignore */
  }
  return DEFAULT_COLUMNS
}

function fmtTime(s: number): string {
  if (!s) return '—'
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  if (h) return m ? `${h}h ${m}m` : `${h}h`
  if (m) return `${m}m`
  return `${s}s`
}

export type GroupByKey = 'status' | 'priority' | 'assignee' | 'label' | 'none'

interface RowGroup {
  key: string
  label: string
  dot?: string
  hours: number
  tasks: Task[]
}

// List view of a board's tasks: grouped (ClickUp-style collapsible sections),
// configurable columns (persisted per board), inline property pills, and
// bulk selection with a floating action bar.
export function BoardList({
  tasks,
  onOpen,
  boardId,
  members = [],
  canEdit = false,
  groupBy = 'status',
  showEmptyGroups = false,
}: {
  tasks: Task[]
  onOpen: (id: string) => void
  boardId: string
  members?: BoardMember[]
  canEdit?: boolean
  groupBy?: GroupByKey
  /** Status/priority grouping: render EVERY group (drop lanes included). */
  showEmptyGroups?: boolean
}) {
  const qc = useQueryClient()
  const { openMenu, menu } = useContextMenu()
  const { data: fleet, isLoading: agentsLoading } = useAgents()
  const { data: me } = useSession()
  // The kanban's trap in list clothing: `= []` sends grouping and the status
  // picker down the `TASK_STATUSES` fallback path, so a failed read groups the
  // board under a workflow it may not use and offers statuses it may not have.
  const labelsQuery = useBoardLabels(boardId)
  const statusesQuery = useBoardStatuses(boardId)
  const boardLabels = labelsQuery.data ?? []
  const boardStatuses = statusesQuery.data ?? []

  const invalidate = () => qc.invalidateQueries({ queryKey: ['board-tasks', boardId] })
  const patch = async (taskId: string, p: Parameters<typeof updateTask>[1]) => {
    await updateTask(taskId, p)
    invalidate()
  }

  // Right-click a row — the SAME quick-control menu the kanban cards serve.
  const rowMenu = (e: React.MouseEvent, t: Task) =>
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
  const label = (id: string) => assigneeInfo(id, fleet?.agents ?? [], members).label
  const assigneeText = (ids: string[]) => (ids.length ? ids.map(label).join(', ') : '—')

  // Init to defaults (SSR-safe), then hydrate from localStorage on the client.
  const [visible, setVisible] = useState<ColumnKey[]>(DEFAULT_COLUMNS)
  const [order, setOrder] = useState<ColumnKey[]>(ALL_KEYS)
  const [sort, setSort] = useState<SortState>({ key: null, dir: 'asc' })
  useEffect(() => {
    setVisible(loadColumns(boardId))
    setOrder(loadOrder(boardId))
    setSort(loadSort(boardId))
  }, [boardId])
  const updateCols = (next: ColumnKey[]) => {
    setVisible(next)
    try {
      localStorage.setItem(storeKey(boardId), JSON.stringify(next))
    } catch {
      /* ignore */
    }
  }
  const updateOrder = (next: ColumnKey[]) => {
    setOrder(next)
    try {
      localStorage.setItem(orderKey(boardId), JSON.stringify(next))
    } catch {
      /* ignore */
    }
  }
  // Click a header to sort: unsorted → asc → desc → unsorted.
  const onSort = (key: ColumnKey) => {
    const next: SortState =
      sort.key !== key ? { key, dir: 'asc' } : sort.dir === 'asc' ? { key, dir: 'desc' } : { key: null, dir: 'asc' }
    setSort(next)
    try {
      localStorage.setItem(sortKey(boardId), JSON.stringify(next))
    } catch {
      /* ignore */
    }
  }

  const byKey = (k: ColumnKey) => LIST_COLUMNS.find((c) => c.key === k)!
  const cols = order.map(byKey).filter((c) => c.fixed || visible.includes(c.key))

  const sortVal = (t: Task, key: ColumnKey): number | string => {
    switch (key) {
      case 'ticket':
        return Number(t.ticketRef?.match(/(\d+)\s*$/)?.[1] ?? 0)
      case 'title':
        return t.title.toLowerCase()
      case 'status': {
        const keys = boardStatuses.length ? boardStatuses.map((st) => st.key) : ([...TASK_STATUSES] as string[])
        return keys.indexOf(t.status)
      }
      case 'priority':
        return PRIORITY_RANK[t.priority] ?? 0
      case 'effort':
        return t.effort ? EFFORT_RANK[t.effort] ?? 0 : -1
      case 'estimate':
        return t.estimatedHours ?? -1
      case 'assignees':
        return assigneeText(t.assignees).toLowerCase()
      case 'due':
        return t.dueDate ? Date.parse(t.dueDate) : Infinity // undated sort last (asc)
      case 'time':
        return t.timeSpentSeconds
      case 'labels':
        return t.tags.join(',').toLowerCase()
      case 'updated':
        return Date.parse(t.updatedAt)
      case 'created':
        return Date.parse(t.createdAt)
      default:
        return 0
    }
  }

  const sorted = useMemo(() => {
    if (!sort.key) return tasks
    const dir = sort.dir === 'asc' ? 1 : -1
    return [...tasks].sort((a, b) => {
      const av = sortVal(a, sort.key!)
      const bv = sortVal(b, sort.key!)
      return av < bv ? -dir : av > bv ? dir : 0
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tasks, sort, fleet])

  // ── Grouping (ClickUp-style sections; a multi-assignee/multi-label ticket
  //    appears in each of its groups) ──────────────────────────────────────
  const groups = useMemo<RowGroup[]>(() => {
    const sum = (ts: Task[]) => Math.round(ts.reduce((s, t) => s + (t.estimatedHours ?? 0), 0) * 10) / 10
    if (groupBy === 'none') return [{ key: 'all', label: '', hours: 0, tasks: sorted }]
    if (groupBy === 'status') {
      const boardKeys = boardStatuses.length ? boardStatuses.map((st) => st.key) : ([...TASK_STATUSES] as string[])
      const order = [...boardKeys, 'failed', 'cancelled']
      return order
        .map((st) => ({
          key: st,
          label: statusLabelOf(st, boardStatuses),
          dot: statusColorOf(st, boardStatuses),
          tasks: sorted.filter((t) => t.status === st),
        }))
        // Board statuses always show when empty-groups is on (drop lanes);
        // failed/cancelled only when occupied.
        .filter((g) => g.tasks.length > 0 || (showEmptyGroups && boardKeys.includes(g.key)))
        .map((g) => ({ ...g, hours: sum(g.tasks) }))
    }
    if (groupBy === 'priority') {
      return [...PRIORITIES]
        .reverse()
        .map((p) => ({ key: p, label: p, dot: PRIORITY_COLOR[p], tasks: sorted.filter((t) => t.priority === p) }))
        .filter((g) => g.tasks.length > 0 || showEmptyGroups)
        .map((g) => ({ ...g, hours: sum(g.tasks) }))
    }
    if (groupBy === 'assignee') {
      const byKey = new Map<string, Task[]>()
      for (const t of sorted) {
        const keys = t.assignees.length ? t.assignees : ['__none']
        for (const k of keys) byKey.set(k, [...(byKey.get(k) ?? []), t])
      }
      return [...byKey.entries()]
        .map(([k, ts]) => ({
          key: k,
          label: k === '__none' ? 'Unassigned' : assigneeInfo(k, fleet?.agents ?? [], members).label,
          tasks: ts,
        }))
        .sort((a, b) => (a.key === '__none' ? 1 : b.key === '__none' ? -1 : a.label.localeCompare(b.label)))
        .map((g) => ({ ...g, hours: sum(g.tasks) }))
    }
    // label
    const byTag = new Map<string, Task[]>()
    for (const t of sorted) {
      const keys = t.tags.length ? t.tags : ['__none']
      for (const k of keys) byTag.set(k, [...(byTag.get(k) ?? []), t])
    }
    return [...byTag.entries()]
      .map(([k, ts]) => ({
        key: k,
        label: k === '__none' ? 'No label' : k,
        dot: k === '__none' ? undefined : LABEL_CSS[boardLabels.find((l) => l.name === k)?.color ?? 'slate'],
        tasks: ts,
      }))
      .sort((a, b) => (a.key === '__none' ? 1 : b.key === '__none' ? -1 : a.label.localeCompare(b.label)))
      .map((g) => ({ ...g, hours: sum(g.tasks) }))
  }, [sorted, groupBy, fleet, members, boardLabels, boardStatuses, showEmptyGroups])

  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const toggleGroup = (k: string) =>
    setCollapsed((c) => {
      const next = new Set(c)
      if (next.has(k)) next.delete(k)
      else next.add(k)
      return next
    })

  // ── Drag rows between groups (status/priority groups are droppable) ─────
  const [dragRow, setDragRow] = useState<string | null>(null)
  const [dragOverGroup, setDragOverGroup] = useState<string | null>(null)
  const groupDroppable = canEdit && (groupBy === 'status' || groupBy === 'priority')
  const dropOnGroup = (g: RowGroup) => {
    const id = dragRow
    setDragRow(null)
    setDragOverGroup(null)
    if (!id || !groupDroppable) return
    if (groupBy === 'status') void patch(id, { status: g.key as TaskStatus })
    else void patch(id, { priority: g.key as Priority })
  }

  // ── Bulk selection ───────────────────────────────────────────────────────
  const [sel, setSel] = useState<Set<string>>(new Set())
  useEffect(() => setSel(new Set()), [boardId, groupBy])
  const toggleSel = (id: string) =>
    setSel((s) => {
      const next = new Set(s)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  const bulk = async (action: { status?: TaskStatus; priority?: Priority; assignToMe?: boolean; archive?: boolean }) => {
    for (const id of sel) {
      if (action.archive) {
        await archiveTask(id, true)
      } else if (action.assignToMe && me) {
        const t = tasks.find((x) => x.id === id)
        const meKey = userAssignee(me.id)
        if (t && !t.assignees.includes(meKey)) await updateTask(id, { assignees: [...t.assignees, meKey] })
      } else {
        await updateTask(id, action)
      }
    }
    setSel(new Set())
    invalidate()
  }

  // Property pills (FieldPill + DropdownMenu) — the shared clickable-inline
  // affordance; pills own their clicks so row-click still opens the ticket.
  const pctx = (t: Task) => ({
    canEdit,
    onPatch: (p: Parameters<typeof updateTask>[1]) => void patch(t.id, p),
    agents: fleet?.agents ?? [],
    members,
    meId: me?.id,
    labels: boardLabels,
    statuses: boardStatuses,
    boardId,
  })

  const cell = (t: Task, key: ColumnKey) => {
    switch (key) {
      case 'ticket':
        return <span className="font-[var(--font-mono)] text-xs text-muted">{t.ticketRef ?? ''}</span>
      case 'title':
        return <span className="font-sans text-fg">{t.title}</span>
      case 'status':
        return <StatusPill t={t} ctx={pctx(t)} />
      case 'priority':
        return <PriorityPill t={t} ctx={pctx(t)} />
      case 'effort':
        return <span className="text-muted">{t.effort ? EFFORT_LABEL[t.effort] : '—'}</span>
      case 'estimate':
        return <EstimatePill t={t} ctx={pctx(t)} />
      case 'assignees':
        // Raw model ids would flash then swap to labels (jumping the column
        // width) — hold the cell with a short bar until the fleet resolves.
        if (agentsLoading && t.assignees.length > 0) return <Skeleton className="h-2.5 w-16 rounded-full" />
        return <AssigneesPill t={t} ctx={pctx(t)} />
      case 'due':
        return <DuePill t={t} ctx={pctx(t)} />
      case 'time':
        return <span className="text-muted">{fmtTime(t.timeSpentSeconds)}</span>
      case 'labels':
        return <LabelsPill t={t} ctx={pctx(t)} />
      case 'updated':
        return <span className="text-muted">{relativeTime(t.updatedAt)}</span>
      case 'created':
        return <span className="text-muted">{relativeTime(t.createdAt)}</span>
    }
  }

  return (
    <div className="flex h-full flex-col">
      {/* The rows below are the tickets, which loaded. Keep them and mark the
          reference read that failed — grouping headers, status colours and the
          status picker are all derived from it. */}
      {(statusesQuery.isError || labelsQuery.isError) && (
        <QueryError
          variant="inline"
          className="border-b border-line-subtle px-4 py-2"
          title={
            statusesQuery.isError
              ? statusesQuery.data === undefined
                ? 'Could not load this board’s statuses — grouping falls back to defaults'
                : 'Statuses may be out of date'
              : labelsQuery.data === undefined
                ? 'Could not load labels — the pills below show names without their colours'
                : 'Labels may be out of date'
          }
          error={statusesQuery.isError ? statusesQuery.error : labelsQuery.error}
          onRetry={() => {
            if (statusesQuery.isError) void statusesQuery.refetch()
            if (labelsQuery.isError) void labelsQuery.refetch()
          }}
        />
      )}
      <div className="relative min-h-0 flex-1 overflow-auto p-4">
        {tasks.length === 0 ? (
          <div className="grid h-full place-items-center text-sm text-muted">No tasks match.</div>
        ) : (
          <table className="w-full text-sm">
            <thead className="border-b border-line text-xs uppercase tracking-wide text-muted">
              <tr>
                <th className="w-7 py-2 pl-2">
                  {canEdit && (
                    <input
                      type="checkbox"
                      title="Select all"
                      checked={sorted.length > 0 && sorted.every((t) => sel.has(t.id))}
                      onChange={(e) => setSel(e.target.checked ? new Set(sorted.map((t) => t.id)) : new Set())}
                      className="accent-[var(--theme-accent)]"
                    />
                  )}
                </th>
                {cols.map((c) => (
                  <th key={c.key} className={cn('px-3 py-2 font-semibold', c.align === 'right' ? 'text-right' : 'text-left')}>
                    <button
                      onClick={() => onSort(c.key)}
                      className={cn(
                        'inline-flex items-center gap-1 uppercase tracking-wide transition-colors hover:text-fg',
                        sort.key === c.key ? 'text-fg' : 'text-muted',
                        c.align === 'right' && 'flex-row-reverse',
                      )}
                    >
                      {c.label}
                      {sort.key === c.key && (sort.dir === 'asc' ? <ChevronUp size={12} /> : <ChevronDown size={12} />)}
                    </button>
                  </th>
                ))}
                <th className="w-8 py-1 pr-2 text-right">
                  <ColumnsMenu visible={visible} order={order} onChangeVisible={updateCols} onChangeOrder={updateOrder} />
                </th>
              </tr>
            </thead>
            {groups.map((g) => {
              const open = !collapsed.has(g.key)
              return (
                <tbody
                  key={g.key}
                  className={cn('divide-y divide-line-subtle', dragOverGroup === g.key && dragRow && 'bg-accent/5')}
                  onDragOver={
                    groupDroppable
                      ? (e) => {
                          e.preventDefault()
                          setDragOverGroup(g.key)
                        }
                      : undefined
                  }
                  onDragLeave={() => setDragOverGroup((d) => (d === g.key ? null : d))}
                  onDrop={groupDroppable ? (e) => { e.preventDefault(); dropOnGroup(g) } : undefined}
                >
                  {groupBy !== 'none' && (
                    <tr
                      className={cn('cursor-pointer select-none bg-sidebar/60', dragOverGroup === g.key && dragRow && 'ring-1 ring-inset ring-[color:var(--theme-accent)]')}
                      onClick={() => toggleGroup(g.key)}
                    >
                      <td colSpan={cols.length + 2} className="px-2 py-1.5">
                        <span className="flex items-center gap-2 text-xs">
                          <span className="text-muted">{open ? '▾' : '▸'}</span>
                          {g.dot && <span className="h-2 w-2 rounded-full" style={{ background: g.dot }} />}
                          <span className="font-semibold uppercase tracking-wide text-fg">{g.label}</span>
                          <span className="text-muted">{g.tasks.length}</span>
                          {g.hours > 0 && <span className="text-[10px] text-muted">Σ {g.hours}h</span>}
                        </span>
                      </td>
                    </tr>
                  )}
                  {open && g.tasks.length === 0 && (
                    <tr>
                      <td colSpan={cols.length + 2} className={cn('px-4 py-2 text-xs italic text-muted/70', dragOverGroup === g.key && dragRow && 'text-accent')}>
                        {dragRow ? 'Drop here' : 'No tickets'}
                      </td>
                    </tr>
                  )}
                  {open &&
                    g.tasks.map((t) => (
                      <tr
                        key={`${g.key}:${t.id}`}
                        draggable={groupDroppable}
                        onDragStart={(e) => {
                          e.dataTransfer.setData('text/task', t.id)
                          e.dataTransfer.effectAllowed = 'move'
                          setDragRow(t.id)
                        }}
                        onDragEnd={() => {
                          setDragRow(null)
                          setDragOverGroup(null)
                        }}
                        onClick={() => onOpen(t.id)}
                        onContextMenu={(e) => rowMenu(e, t)}
                        className={cn('group cursor-pointer transition-colors hover:bg-card', sel.has(t.id) && 'bg-card/70', dragRow === t.id && 'opacity-40')}
                      >
                        <td className="pl-2" onClick={(e) => e.stopPropagation()}>
                          {canEdit ? (
                            <input
                              type="checkbox"
                              checked={sel.has(t.id)}
                              onChange={() => toggleSel(t.id)}
                              className={cn('accent-[var(--theme-accent)] transition-opacity', sel.size === 0 && 'opacity-0 group-hover:opacity-100')}
                            />
                          ) : (
                            <CopyLinkButton path={`/boards/${t.boardId}/${t.id}`} className="opacity-0 group-hover:opacity-100" />
                          )}
                        </td>
                        {cols.map((c) => (
                          <td key={c.key} className={cn('px-3 py-2', c.align === 'right' && 'text-right')}>
                            {cell(t, c.key)}
                          </td>
                        ))}
                        <td />
                      </tr>
                    ))}
                </tbody>
              )
            })}
          </table>
        )}
      </div>

      {/* Bulk action bar — appears with a selection, acts on every selected
          ticket, then clears. */}
      {sel.size > 0 && (
        <div className="pointer-events-none absolute inset-x-0 bottom-4 z-30 flex justify-center">
          <div className="mercury-panel pointer-events-auto flex items-center gap-1 rounded-xl px-2 py-1.5 shadow-[var(--theme-shadow-3)]">
            <span className="px-2 text-xs font-medium text-fg">{sel.size} selected</span>
            <DropdownMenu
              up
              align="left"
              trigger={(open) => (
                <FieldPill active={open} className="text-xs">
                  Move to
                </FieldPill>
              )}
              items={(boardStatuses.length ? boardStatuses.map((st) => st.key) : ([...TASK_STATUSES] as string[])).map((st) => ({
                label: statusLabelOf(st, boardStatuses),
                icon: <span className="h-2 w-2 rounded-full" style={{ background: statusColorOf(st, boardStatuses) }} />,
                onSelect: () => void bulk({ status: st as TaskStatus }),
              }))}
            />
            <DropdownMenu
              up
              align="left"
              trigger={(open) => (
                <FieldPill active={open} className="text-xs">
                  Priority
                </FieldPill>
              )}
              items={[...PRIORITIES].reverse().map((pr) => ({
                label: pr,
                icon: <span className="h-2 w-2 rounded-full" style={{ background: PRIORITY_COLOR[pr] }} />,
                onSelect: () => void bulk({ priority: pr }),
              }))}
            />
            {me && (
              <button onClick={() => void bulk({ assignToMe: true })} className="rounded-md px-2 py-0.5 text-xs text-muted transition-colors hover:bg-card hover:text-fg">
                Assign to me
              </button>
            )}
            <button onClick={() => void bulk({ archive: true })} className="rounded-md px-2 py-0.5 text-xs text-muted transition-colors hover:bg-card hover:text-[color:var(--theme-danger)]">
              Archive
            </button>
            <button onClick={() => setSel(new Set())} title="Clear selection" className="rounded-md px-1.5 py-0.5 text-xs text-muted transition-colors hover:text-fg">
              ✕
            </button>
          </div>
        </div>
      )}
      {menu}
    </div>
  )
}

function ColumnsMenu({
  visible,
  order,
  onChangeVisible,
  onChangeOrder,
}: {
  visible: ColumnKey[]
  order: ColumnKey[]
  onChangeVisible: (c: ColumnKey[]) => void
  onChangeOrder: (c: ColumnKey[]) => void
}) {
  const [open, setOpen] = useState(false)
  const [dragKey, setDragKey] = useState<ColumnKey | null>(null)
  const [overKey, setOverKey] = useState<ColumnKey | null>(null)
  const [overPos, setOverPos] = useState<'before' | 'after'>('before')
  const ref = useRef<HTMLDivElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const onDoc = (e: MouseEvent) => {
      const t = e.target as Node
      if (ref.current && !ref.current.contains(t) && !panelRef.current?.contains(t)) setOpen(false)
    }
    document.addEventListener('mousedown', onDoc)
    return () => document.removeEventListener('mousedown', onDoc)
  }, [open])

  const toggle = (k: ColumnKey) =>
    onChangeVisible(visible.includes(k) ? visible.filter((x) => x !== k) : [...visible, k])

  // Drop the dragged column before/after the row, based on where it was released.
  const drop = (target: ColumnKey) => {
    if (dragKey && dragKey !== target) {
      const next = order.filter((k) => k !== dragKey)
      const idx = next.indexOf(target) + (overPos === 'after' ? 1 : 0)
      next.splice(idx, 0, dragKey)
      onChangeOrder(next)
    }
    setDragKey(null)
    setOverKey(null)
  }

  // Icon-only trigger living in the table's trailing header cell; the panel
  // portals with fixed positioning so the scroll container can't clip it.
  const [pos, setPos] = useState<React.CSSProperties | null>(null)
  const openPanel = () => {
    if (open) return setOpen(false)
    const r = ref.current?.getBoundingClientRect()
    if (!r) return
    setPos({ position: 'fixed', zIndex: 80, top: r.bottom + 4, right: Math.max(8, window.innerWidth - r.right) })
    setOpen(true)
  }
  return (
    <div ref={ref} className="relative inline-block">
      <button
        onClick={openPanel}
        title="Columns — show, hide, reorder"
        aria-label="Configure columns"
        className={cn(
          'grid h-6 w-6 place-items-center rounded-md transition-colors',
          open ? 'bg-card text-fg' : 'text-muted hover:bg-card hover:text-fg',
        )}
      >
        <SlidersHorizontal size={13} />
      </button>
      {open && pos && typeof document !== 'undefined' && createPortal(
        <div ref={panelRef} style={pos} className="mercury-panel w-48 rounded-xl p-1">
          <div className="px-2 pb-1 pt-1.5 text-[10px] uppercase tracking-wide text-muted">Drag to reorder</div>
          {order.map((key) => {
            const c = LIST_COLUMNS.find((x) => x.key === key)!
            return (
              <div
                key={c.key}
                draggable
                onDragStart={() => setDragKey(c.key)}
                onDragEnd={() => {
                  setDragKey(null)
                  setOverKey(null)
                }}
                onDragOver={(e) => {
                  e.preventDefault()
                  const rect = e.currentTarget.getBoundingClientRect()
                  setOverKey(c.key)
                  setOverPos(e.clientY > rect.top + rect.height / 2 ? 'after' : 'before')
                }}
                onDrop={(e) => {
                  e.preventDefault()
                  drop(c.key)
                }}
                className={cn(
                  'relative flex items-center gap-1.5 rounded-md px-1.5 py-1.5 text-sm text-fg',
                  dragKey === c.key && 'opacity-40',
                )}
              >
                {overKey === c.key && dragKey !== c.key && (
                  <span
                    className={cn(
                      'pointer-events-none absolute inset-x-1 h-0.5 rounded-full bg-accent',
                      overPos === 'before' ? '-top-px' : '-bottom-px',
                    )}
                  />
                )}
                <GripVertical size={13} className="shrink-0 cursor-grab text-muted active:cursor-grabbing" />
                <input
                  type="checkbox"
                  checked={c.fixed || visible.includes(c.key)}
                  disabled={c.fixed}
                  onChange={() => toggle(c.key)}
                  className="accent-[color:var(--theme-accent)]"
                />
                <span className={cn('cursor-pointer select-none', c.fixed && 'opacity-60')} onClick={() => !c.fixed && toggle(c.key)}>
                  {c.label}
                </span>
              </div>
            )
          })}
        </div>,
        document.body,
      )}
    </div>
  )
}
