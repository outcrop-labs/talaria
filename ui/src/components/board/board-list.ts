// Shared bits of the list-view split (BoardList.svelte + ColumnsMenu.svelte):
// the column registry, per-board localStorage persistence, sort ranks, and
// the grouping types.
import { readStored } from '@/lib/persist'
import type { Task } from '@/lib/task-const'

export type ColumnKey = 'ticket' | 'title' | 'status' | 'priority' | 'effort' | 'estimate' | 'assignees' | 'due' | 'time' | 'labels' | 'updated' | 'created'
export interface ListColumn {
  key: ColumnKey
  label: string
  align?: 'right'
  default?: boolean
  fixed?: boolean // always shown (can't be toggled off)
}

export const LIST_COLUMNS: ListColumn[] = [
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
export const ALL_KEYS = LIST_COLUMNS.map((c) => c.key)
export const DEFAULT_COLUMNS = LIST_COLUMNS.filter((c) => c.default).map((c) => c.key)
export const storeKey = (boardId: string) => `talaria:list-cols:${boardId}`
export const sortKey = (boardId: string) => `talaria:list-sort:${boardId}`
export const orderKey = (boardId: string) => `talaria:list-order:${boardId}`

/** Stored order, merged with any columns added since (appended, none dropped). */
export function loadOrder(boardId: string): ColumnKey[] {
  return readStored<ColumnKey[]>(
    orderKey(boardId),
    (value) => {
      if (!Array.isArray(value)) return null
      const kept = value.filter((k): k is ColumnKey => ALL_KEYS.includes(k as ColumnKey))
      return [...kept, ...ALL_KEYS.filter((k) => !kept.includes(k))]
    },
    ALL_KEYS,
  )
}

export const PRIORITY_RANK: Record<string, number> = { low: 0, medium: 1, high: 2, urgent: 3 }
export const EFFORT_RANK: Record<string, number> = { xs: 0, s: 1, m: 2, l: 3, xl: 4 }

export type SortDir = 'asc' | 'desc'
export interface SortState {
  key: ColumnKey | null
  dir: SortDir
}

export function loadSort(boardId: string): SortState {
  return readStored<SortState>(
    sortKey(boardId),
    (value) => {
      const s = value as SortState | null
      return s && (s.key === null || LIST_COLUMNS.some((c) => c.key === s.key)) ? s : null
    },
    { key: null, dir: 'asc' },
  )
}

export function loadColumns(boardId: string): ColumnKey[] {
  return readStored<ColumnKey[]>(
    storeKey(boardId),
    (value) => (Array.isArray(value) ? value.filter((k): k is ColumnKey => LIST_COLUMNS.some((c) => c.key === k)) : null),
    DEFAULT_COLUMNS,
  )
}

/** The list's Time column, in the spelling the ticket detail also renders.
 *  Shared now — `fmtDuration` in `@/lib/format`, which takes SECONDS — and
 *  re-exported under the name BoardList already imports it by. */
export { fmtDuration as fmtTime } from '@/lib/format'

export type GroupByKey = 'status' | 'priority' | 'assignee' | 'label' | 'none'

export interface RowGroup {
  key: string
  label: string
  dot?: string
  hours: number
  tasks: Task[]
}
