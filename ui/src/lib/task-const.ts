// Task/board constants + types — shared by client and server (no server deps).

import type { Attachment } from './attachments'

export const TASK_STATUSES = ['inbox', 'assigned', 'in_progress', 'blocked', 'quality_review', 'done'] as const
export type TaskStatus = (typeof TASK_STATUSES)[number] | 'failed' | 'cancelled'
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
  timeSpentSeconds: number
  /** Human planning estimate, in hours. */
  estimatedHours: number | null
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
