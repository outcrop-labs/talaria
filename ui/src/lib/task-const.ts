// Task/board constants + types — shared by client and server (no server deps).

import type { Attachment } from './attachments'

export const TASK_STATUSES = ['inbox', 'assigned', 'in_progress', 'blocked', 'quality_review', 'done'] as const

/** THE OFF-BOARD LIST. Statuses legal on every board but never board COLUMNS:
 *  terminal in every UI that reads them, and agents may not park work there.
 *
 *  Said exactly twice, once per language that needs it: here as the client's
 *  wire vocabulary (the client cannot import from `server/`, and no TS module
 *  can be imported by Rust at all), and in the Rust statuses engine as
 *  `OFF_BOARD_STATUSES` (api/src/statuses.rs), next to the resolvers that
 *  exclude them. `TaskStatus` DERIVES from the list here, and the pin test in
 *  `task-const.test.ts` reads the Rust source and fails if the two languages
 *  ever disagree. Two copies, each held shut. */
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
 *  The Rust engine casts at the SQL boundary (`estimated_hours::float8`,
 *  `time_spent_seconds` typed i64 — api/src/tasks.rs `TASK_SELECT`), so the
 *  task wire carries real numbers. The tolerant union stays anyway: the wire is
 *  JSON across an API boundary, and a numeric column that ships uncast arrives
 *  as a string no matter what the type says — a lie the compiler cannot catch
 *  in exactly the places arithmetic is implicit. Callers convert through
 *  `pgNum`/`pgNumOr` below; a number passes through unchanged. */
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
  /** `time_spent_seconds bigint` — the Rust read types it i64 (`TASK_SELECT`,
   *  api/src/tasks.rs), so it arrives as a real number. Read it through
   *  `taskTimeSpent(t)` anyway: one honest path for the arithmetic, whatever
   *  the wire does (see `PgNumeric` above).
   *
   *  @deprecated Not really deprecated — read it through `taskTimeSpent(t)`.
   *  The tag is here for the editor strike-through, which is the only friction
   *  that exists against writing the raw field into arithmetic again. */
  timeSpentSeconds: number
  /** Human planning estimate, in hours. `estimated_hours numeric`, cast
   *  `::float8` at the Rust boundary — see `PgNumeric` above. */
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
