// Task/board constants + types — shared by client and server (no server deps).

import type { Attachment } from './attachments'

export const TASK_STATUSES = ['inbox', 'assigned', 'in_progress', 'blocked', 'quality_review', 'done'] as const

/** THE OFF-BOARD LIST. Statuses legal on every board but never board COLUMNS:
 *  terminal in every UI that reads them, and agents may not park work there.
 *
 *  This used to be said three times — here as the type union `| 'failed' |
 *  'cancelled'`, in `server/statuses.ts` as `OFF_BOARD_STATUSES`, and in
 *  `kanban.tsx` as a bare `['failed','cancelled']` literal that decided which
 *  legacy terminal columns render AT ALL. Three copies of "which statuses exist
 *  but are not columns" is three chances for a board to hold a ticket in a
 *  status no view will draw, and a ticket in no column is work that has silently
 *  disappeared from the board.
 *
 *  It lives HERE because this file is the one both sides may import: the client
 *  cannot import from `server/`, but `server/` already imports from `@/lib`
 *  (see `server/tasks.ts`, `server/workflows.ts`). `TaskStatus` now DERIVES from
 *  it, so the union cannot drift from the list.
 *
 *  `server/statuses.ts` still declares its own copy next to the resolvers that
 *  exclude them. CI (`scripts/check-invariants.mjs`) fails if the two lists ever
 *  disagree, and the permanent fix — one line, out of scope for the round that
 *  wrote this — is for that file to `import { OFF_BOARD_STATUSES } from
 *  '@/lib/task-const'` and delete its own, at which point the CI cross-check
 *  deletes itself too. */
export const OFF_BOARD_STATUSES = ['failed', 'cancelled'] as const
export type OffBoardStatus = (typeof OFF_BOARD_STATUSES)[number]
export type TaskStatus = (typeof TASK_STATUSES)[number] | OffBoardStatus

/** Is this key one of the off-board terminals? Narrows, so callers get the
 *  union back rather than re-testing the strings. */
export const isOffBoardStatus = (key: string): key is OffBoardStatus =>
  (OFF_BOARD_STATUSES as readonly string[]).includes(key)
export const PRIORITIES = ['low', 'medium', 'high', 'urgent'] as const
export type Priority = (typeof PRIORITIES)[number]

// Agent-appropriate effort sizing (t-shirt sizes) — estimates in hours are silly
// for agents, so effort is a relative complexity signal instead.
export const TICKET_COLORS = ['slate', 'bronze', 'green', 'amber', 'red', 'blue', 'purple', 'teal', 'pink', 'orange', 'lime', 'cyan', 'indigo', 'magenta', 'olive', 'brown'] as const
export type TicketColor = (typeof TICKET_COLORS)[number]

export const EFFORTS = ['xs', 's', 'm', 'l', 'xl'] as const
export type Effort = (typeof EFFORTS)[number]
export const EFFORT_LABEL: Record<Effort, string> = { xs: 'XS', s: 'S', m: 'M', l: 'L', xl: 'XL' }

// Assignees mix AGENT model ids (bare strings) and HUMANS as `user:<uuid>`.
export const isHumanAssignee = (a: string): boolean => a.startsWith('user:')
export const humanAssigneeId = (a: string): string => a.slice(5)

/** ── The wire type of a postgres NUMERIC / BIGINT ───────────────────────────
 *  postgres.js hands `numeric` and `int8` back as STRINGS. There is no `types`
 *  override on the pool (`server/db/pg.ts` constructs `postgres(url, {max,
 *  idle_timeout, onnotice})` and nothing else), and no mapping layer between
 *  the row and this interface — `server/tasks.ts` selects
 *  `t.estimated_hours as "estimatedHours"` and casts the rows
 *  `as unknown as Task[]`. So `typeof task.estimatedHours === 'string'` after a
 *  round trip, and the same value is JSON-serialised as a string to the client.
 *
 *  That is not cosmetic. `updateTask` decides whether to write an activity line
 *  with `patch.estimatedHours !== cur.estimatedHours` — a number from the API
 *  body against a string from the row — so re-saving an unchanged estimate logs
 *  a spurious "estimate → 4h" every time, and client rollups that do
 *  `sum + t.estimatedHours` build a concatenated string instead of a total.
 *
 *  Declaring these `number` did not make them numbers; it only stopped the
 *  compiler from mentioning it. So the type says what arrives, and callers
 *  convert through `pgNum`/`pgNumOr` below.
 *
 *  THE REAL FIX is one line in `server/db/pg.ts` — a postgres.js `types`
 *  override that parses numeric/int8 as declared (audit task 21) — after which
 *  `PgNumeric` collapses back to `number` here and the coercions go away. That
 *  file is outside this change's scope. */
export type PgNumeric = number | string

/** Wire numeric → a real number. Null/undefined pass through as null, and
 *  anything unparseable becomes null rather than NaN: NaN survives arithmetic
 *  silently and reappears as "NaNh" in the UI, which is the same class of
 *  failure this type exists to stop. */
export const pgNum = (v: PgNumeric | null | undefined): number | null => {
  if (v === null || v === undefined) return null
  const n = typeof v === 'number' ? v : Number(v)
  return Number.isFinite(n) ? n : null
}

/** `pgNum` with a floor, for sums and rollups where "no estimate" means zero. */
export const pgNumOr = (v: PgNumeric | null | undefined, fallback: number): number => pgNum(v) ?? fallback

/** Accumulated agent time in seconds, as a real number.
 *
 *  Exists so no surface has to remember that `timeSpentSeconds` is declared
 *  `number` and arrives as a string (see the field's own comment). Two bugs came
 *  from reading it raw, and neither was a type error: sorting compared the
 *  strings, so `"100"` sorted before `"99"`; and `if (!seconds) return '—'` never
 *  fired for zero, because `"0"` is truthy — a ticket with no logged time
 *  rendered "0s" instead of an em dash.
 *
 *  Takes the structural type rather than `Task` so it keeps working unchanged
 *  when the field is widened to `PgNumeric`. */
export const taskTimeSpent = (t: { timeSpentSeconds: PgNumeric }): number => pgNumOr(t.timeSpentSeconds, 0)

export interface Task {
  id: string
  boardId: string
  ticketRef: string | null
  title: string
  description: string | null
  status: TaskStatus
  priority: Priority
  effort: Effort | null
  assignees: string[]
  createdBy: string
  dueDate: string | null
  /** Gantt scheduling: bars run startDate → dueDate. */
  startDate: string | null
  /** Color-coding (palette key); null = status/priority defaults. */
  color: TicketColor | null
  tags: string[]
  attachments: Attachment[]
  /** `time_spent_seconds bigint` — the SAME wire lie as `estimatedHours` below:
   *  it arrives as a STRING and this declaration says `number`.
   *
   *  MEASURED, not assumed. Changing this line to `PgNumeric` and running
   *  `npx tsc --noEmit` in `ui/` produces EXACTLY TWO errors, both call sites of
   *  a `(seconds: number)` formatter:
   *
   *    components/board/board-list.tsx:408   fmtTime(t.timeSpentSeconds)
   *    components/board/task-detail.tsx:416  formatDuration(t.timeSpentSeconds)
   *
   *  The first is gone — board-list reads through `taskTimeSpent` below. So the
   *  whole remaining cost of telling the truth here is ONE signature:
   *  `formatDuration(seconds: number)` at task-detail.tsx:756. That file was
   *  outside the round that wrote this; the change is `PgNumeric` in the
   *  signature and `pgNumOr(seconds, 0)` on its first line.
   *
   *  Note what tsc did NOT catch, because it is the reason this comment is long:
   *  `board-list.tsx` sorted by this field with a bare `return t.timeSpentSeconds`
   *  into a comparator whose operands are inferred. No error, and `"100" < "99"`
   *  is true, so the time column sorted lexically. A type that lies is not caught
   *  by the compiler in exactly the places arithmetic is implicit — which is most
   *  of them. Read through the accessor.
   *
   *  THE GENERAL FIX is one line in `server/db/pg.ts`: a postgres.js `types`
   *  override parsing numeric/int8 as declared (audit task 21). Then `PgNumeric`
   *  collapses to `number`, this field is honest for free, and `taskTimeSpent`
   *  becomes a pass-through.
   *
   *  @deprecated Not really deprecated — read it through `taskTimeSpent(t)`. The
   *  tag is here for the editor strike-through, which is the only friction that
   *  exists against writing the raw field into arithmetic again. */
  timeSpentSeconds: number
  /** Human planning estimate, in hours. `estimated_hours numeric` — see
   *  `PgNumeric`: this arrives as a STRING and the arithmetic on it was wrong. */
  estimatedHours: PgNumeric | null
  /** Sub-task parent (one level deep). */
  parentId: string | null
  commentCount: number
  outcome: string | null
  resolution: string | null
  errorMessage: string | null
  createdAt: string
  updatedAt: string
  completedAt: string | null
  archivedAt: string | null
}

/** A ticket this task depends on (blocked by) or that depends on it (blocks). */
export interface TaskLink {
  id: string
  ticketRef: string | null
  title: string
  status: TaskStatus
}

export interface QualityReview {
  id: string
  reviewer: string
  status: string
  notes: string | null
  createdAt: string
}

export interface TaskComment {
  id: string
  author: string
  content: string
  parentId: string | null
  createdAt: string
}

export interface TaskActivity {
  id: string
  actor: string
  type: string
  description: string
  createdAt: string
}

export const STATUS_LABEL: Record<string, string> = {
  inbox: 'Inbox',
  assigned: 'Assigned',
  in_progress: 'In progress',
  blocked: 'Blocked',
  quality_review: 'Quality review',
  done: 'Done',
  failed: 'Failed',
  cancelled: 'Cancelled',
}

export const PRIORITY_COLOR: Record<Priority, string> = {
  low: 'var(--theme-muted)',
  medium: 'var(--theme-accent)',
  high: 'var(--theme-warning)',
  urgent: 'var(--theme-danger)',
}

export const PRIORITY_ICON: Record<Priority, string> = {
  low: '▁',
  medium: '▄',
  high: '▆',
  urgent: '⏻',
}
