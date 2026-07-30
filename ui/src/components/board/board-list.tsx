import { useEffect, useMemo, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { SlidersHorizontal, ChevronUp, ChevronDown, GripVertical, Archive, ExternalLink, Hash, Link as LinkIcon } from 'lucide-react'
import { useAgents } from '@/lib/agents'
import { archiveTask, type BoardMember } from '@/lib/boards'
import { assigneeInfo } from '@/lib/assignees'
import { CopyLinkButton } from '@/components/ui/copy-link-button'
import { useContextMenu, copyAppLink, type ContextMenuEntry } from '@/components/ui/context-menu'
import { Skeleton } from '@/components/ui/skeleton'
import { cn } from '@/lib/cn'
import { EFFORT_LABEL, PRIORITY_COLOR, STATUS_LABEL, TASK_STATUSES, type Task } from '@/lib/task-const'
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
  { key: 'labels', label: 'Labels' },
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

// List view of a board's tasks with configurable columns (persisted per board).
export function BoardList({
  tasks,
  onOpen,
  boardId,
  members = [],
}: {
  tasks: Task[]
  onOpen: (id: string) => void
  boardId: string
  members?: BoardMember[]
}) {
  const qc = useQueryClient()
  const { openMenu, menu } = useContextMenu()
  const { data: fleet, isLoading: agentsLoading } = useAgents()

  // Right-click a row — shortcuts to actions the board already offers.
  const rowMenu = (e: React.MouseEvent, t: Task) => {
    const items: ContextMenuEntry[] = [
      { label: 'Open', icon: <ExternalLink size={14} />, onSelect: () => onOpen(t.id) },
      { label: 'Copy link', icon: <LinkIcon size={14} />, onSelect: () => copyAppLink(`/boards/${t.boardId}/${t.id}`) },
    ]
    if (t.ticketRef) {
      const ref = t.ticketRef
      items.push({ label: 'Copy ticket ref', icon: <Hash size={14} />, onSelect: () => void navigator.clipboard.writeText(ref) })
    }
    items.push('sep', {
      label: t.archivedAt ? 'Unarchive' : 'Archive',
      icon: <Archive size={14} />,
      danger: !t.archivedAt,
      onSelect: () => {
        void archiveTask(t.id, !t.archivedAt).then(() => qc.invalidateQueries({ queryKey: ['board-tasks', boardId] }))
      },
    })
    openMenu(e, items)
  }
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
      case 'status':
        return TASK_STATUSES.indexOf(t.status as (typeof TASK_STATUSES)[number])
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

  const cell = (t: Task, key: ColumnKey) => {
    switch (key) {
      case 'ticket':
        return <span className="font-[var(--font-mono)] text-xs text-muted">{t.ticketRef ?? ''}</span>
      case 'title':
        return <span className="font-sans text-fg">{t.title}</span>
      case 'status':
        return <span className="text-muted">{STATUS_LABEL[t.status]}</span>
      case 'priority':
        return (
          <span className="inline-flex items-center gap-1.5 text-muted">
            <span className="h-2 w-2 rounded-full" style={{ background: PRIORITY_COLOR[t.priority] }} />
            {t.priority}
          </span>
        )
      case 'effort':
        return <span className="text-muted">{t.effort ? EFFORT_LABEL[t.effort] : '—'}</span>
      case 'estimate':
        return <span className="text-muted">{t.estimatedHours != null ? `${t.estimatedHours}h` : '—'}</span>
      case 'assignees':
        // Raw model ids would flash then swap to labels (jumping the column
        // width) — hold the cell with a short bar until the fleet resolves.
        if (agentsLoading && t.assignees.length > 0) return <Skeleton className="h-2.5 w-16 rounded-full" />
        return <span className="text-muted">{assigneeText(t.assignees)}</span>
      case 'due':
        return <span className="text-muted">{t.dueDate ? t.dueDate.slice(0, 10) : '—'}</span>
      case 'time':
        return <span className="text-muted">{fmtTime(t.timeSpentSeconds)}</span>
      case 'labels':
        return <span className="text-muted">{t.tags.length ? t.tags.join(', ') : '—'}</span>
      case 'updated':
        return <span className="text-muted">{relativeTime(t.updatedAt)}</span>
      case 'created':
        return <span className="text-muted">{relativeTime(t.createdAt)}</span>
    }
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center justify-end border-b border-line-subtle px-4 py-2">
        <ColumnsMenu visible={visible} order={order} onChangeVisible={updateCols} onChangeOrder={updateOrder} />
      </div>

      <div className="min-h-0 flex-1 overflow-auto p-4">
        {tasks.length === 0 ? (
          <div className="grid h-full place-items-center text-sm text-muted">No tasks match.</div>
        ) : (
          <table className="w-full text-sm">
            <thead className="border-b border-line text-xs uppercase tracking-wide text-muted">
              <tr>
                <th className="w-6 py-2" />
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
              </tr>
            </thead>
            <tbody className="divide-y divide-line-subtle">
              {sorted.map((t) => (
                <tr
                  key={t.id}
                  onClick={() => onOpen(t.id)}
                  onContextMenu={(e) => rowMenu(e, t)}
                  className="group cursor-pointer transition-colors hover:bg-card"
                >
                  <td className="pl-2">
                    <CopyLinkButton path={`/boards/${t.boardId}/${t.id}`} className="opacity-0 group-hover:opacity-100" />
                  </td>
                  {cols.map((c) => (
                    <td key={c.key} className={cn('px-3 py-2', c.align === 'right' && 'text-right')}>
                      {cell(t, c.key)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
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
  useEffect(() => {
    if (!open) return
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false)
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

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex h-8 items-center gap-1.5 rounded-lg border border-line px-2.5 text-xs text-muted transition-colors hover:text-fg"
      >
        <SlidersHorizontal size={14} /> Columns
      </button>
      {open && (
        <div className="mercury-panel absolute right-0 z-30 mt-1 w-48 rounded-xl p-1">
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
        </div>
      )}
    </div>
  )
}
