// Ticket property pills — the ClickUp-style direct-manipulation layer, done
// the Mercury way. Each pill shows the value quietly, lights up on hover
// (FieldPill), and opens its picker in place (DropdownMenu). One set of
// components serves kanban cards, list rows, and group headers, so every
// surface manipulates tickets identically. All pickers stop propagation —
// the row/card click still opens the ticket.
//
// This module is the plain-TS home: shared types, the label palette,
// and the closed/overdue helpers. The pills themselves are the sibling
// PascalCase .svelte components (StatusPill, PriorityPill, DuePill,
// EstimatePill, AssigneesPill, LabelsPill, ColorPill, LabelChip).
import type { MenuIcon } from '@/components/ui/context-menu.svelte'
import type { BoardLabel, BoardMember, LabelColor } from '@/lib/boards.svelte'
import type { BoardStatus } from '@/lib/statuses'
import { isOffBoardStatus, type Priority, type Task, type TaskStatus } from '@/lib/task-const'
import ColorDot from './ColorDot.svelte'

export type TicketPatch = {
  status?: TaskStatus
  priority?: Priority
  dueDate?: string | null
  estimatedHours?: number | null
  assignees?: string[]
  tags?: string[]
}

/** The label palette, keyed by the stored color name. Semantic hues ride the
 *  theme tokens (they follow light/dark); orange rides chart-2. The remaining
 *  hues are DATA colors (user-picked label identities, like chart series)
 *  with no token equivalent — kept literal by design. Blue stays literal:
 *  chart-1 (#68B6C8) would collide with the adjacent cyan/teal entries. */
export const LABEL_CSS: Record<LabelColor, string> = {
  slate: 'var(--theme-muted)',
  bronze: 'var(--theme-accent)',
  green: 'var(--theme-success)',
  amber: 'var(--theme-warning)',
  red: 'var(--theme-danger)',
  blue: '#6b9bd1',
  orange: 'var(--theme-chart-2)',
  purple: '#a78bda',
  teal: '#5fb8ad',
  pink: '#d189a8',
  lime: '#a2c05b',
  cyan: '#5fb6d4',
  indigo: '#7a86d9',
  magenta: '#c069c9',
  olive: '#a8a45e',
  brown: '#a8795a',
}

export const labelColor = (name: string, labels: BoardLabel[]): string =>
  LABEL_CSS[labels.find((l) => l.name === name)?.color ?? 'slate']

export interface PillCtx {
  canEdit: boolean
  onPatch: (p: TicketPatch) => void
  agents: Array<{ id: string; label: string }>
  members: BoardMember[]
  meId?: string | null
  /** The board's label registry — powers LabelsPill + tinted chips. */
  labels?: BoardLabel[]
  /** The board's status set — powers StatusPill options + closed checks. */
  statuses?: BoardStatus[]
  boardId?: string
}

export const STATUS_COLOR: Record<string, string> = {
  inbox: 'var(--theme-muted)',
  assigned: 'var(--theme-accent)',
  in_progress: 'var(--theme-warning)',
  blocked: 'var(--theme-danger)',
  quality_review: 'var(--theme-accent-secondary)',
  done: 'var(--theme-success)',
  failed: 'var(--theme-danger)',
  cancelled: 'var(--theme-muted)',
}

/** Closed = done-category on this board, plus the legacy terminals: the bare
 *  `done` key (boards that predate custom statuses) and the OFF-BOARD list,
 *  which is imported rather than spelled out — see `@/lib/task-const`.
 *
 *  THE ONE CLIENT-SIDE CLOSED PREDICATE — pills and overdue filters both call
 *  it, so no board can disagree with itself about what closed means. */
export const isClosedStatus = (key: string, statuses?: BoardStatus[]): boolean =>
  statuses?.find((s) => s.key === key)?.category === 'done' || key === 'done' || isOffBoardStatus(key)

export const isOverdueTask = (t: Pick<Task, 'dueDate' | 'status'>, statuses?: BoardStatus[]) =>
  !!t.dueDate && new Date(t.dueDate).getTime() < Date.now() && !isClosedStatus(t.status, statuses)

/** Colored-dot menu icon (the MenuIcon tuple form) — for entry arrays built
 *  in plain TS, where a component can't be inlined. */
export const dotIcon = (color: string, cls = 'h-1.5 w-1.5 rounded-full'): MenuIcon => [
  ColorDot,
  { color, class: cls },
]
