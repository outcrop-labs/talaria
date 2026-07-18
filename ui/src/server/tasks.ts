// Task queue — Talaria owns it (ripped from mission-control). Tasks are cards on
// a board with ticket refs, effort, structured results, comments, activity,
// watchers, and a quality-review approval gate. Agents get assigned work via
// heartbeat and report via PUT /api/tasks/:id.
import { db } from './db/pg'
import { publishBoard } from './realtime'
import { taskUsage, type TaskUsage } from './usage'
import { listJudgeReviews, type JudgeReview } from './judge'
import type { Effort, Priority, QualityReview, Task, TaskActivity, TaskComment, TaskLink, TaskStatus } from '@/lib/task-const'

async function taskBoardId(id: string): Promise<string | null> {
  const sql = await db()
  const rows = await sql`select board_id from tasks where id = ${id}`
  return rows.length ? (rows[0] as { board_id: string }).board_id : null
}

export { TASK_STATUSES, PRIORITIES, EFFORTS } from '@/lib/task-const'
export type { Effort, Priority, Task, TaskStatus } from '@/lib/task-const'

// Ticket ref is board.ticket_prefix + '-' + task.ticket_no.
const TASK_SELECT = `select t.id, t.board_id as "boardId",
  case when t.ticket_no is not null then coalesce(b.ticket_prefix,'TASK') || '-' || t.ticket_no end as "ticketRef",
  t.title, t.description, t.status, t.priority, t.effort, t.assignees, t.created_by as "createdBy",
  t.due_date as "dueDate", t.tags, t.attachments, t.time_spent_seconds as "timeSpentSeconds",
  t.outcome, t.resolution, t.error_message as "errorMessage",
  t.created_at as "createdAt", t.updated_at as "updatedAt", t.completed_at as "completedAt",
  t.archived_at as "archivedAt"
  from tasks t join boards b on b.id = t.board_id`

// Minimal shape for dependency links (blocked-by / blocks).
const LINK_SELECT = `select t.id,
  case when t.ticket_no is not null then coalesce(b.ticket_prefix,'TASK') || '-' || t.ticket_no end as "ticketRef",
  t.title, t.status from tasks t join boards b on b.id = t.board_id`

export async function listBoardTasks(boardId: string, includeArchived = false): Promise<Task[]> {
  const sql = await db()
  const where = includeArchived ? 't.board_id = $1' : 't.board_id = $1 and t.archived_at is null'
  return (await sql.unsafe(`${TASK_SELECT} where ${where} order by t.updated_at desc`, [boardId])) as unknown as Task[]
}

export async function getTask(id: string): Promise<Task | null> {
  const sql = await db()
  const rows = await sql.unsafe(`${TASK_SELECT} where t.id = $1`, [id])
  return (rows[0] as unknown as Task) ?? null
}

export async function getTaskFull(id: string): Promise<{
  task: Task
  comments: TaskComment[]
  activity: TaskActivity[]
  watchers: string[]
  reviews: QualityReview[]
  judgeReviews: JudgeReview[]
  blockedBy: TaskLink[]
  blocks: TaskLink[]
  usage: TaskUsage
} | null> {
  const task = await getTask(id)
  if (!task) return null
  const [[blockedBy, blocks], comments, activity, watchers, reviews, judgeReviews, usage] = await Promise.all([
    listDependencies(id),
    listComments(id),
    listActivity(id),
    listWatchers(id),
    listReviews(id),
    listJudgeReviews(id),
    taskUsage(id),
  ])
  return { task, comments, activity, watchers, reviews, judgeReviews, blockedBy, blocks, usage }
}

export async function createTask(input: {
  boardId: string
  title: string
  description?: string | null
  priority?: Priority
  effort?: Effort | null
  assignees?: string[]
  dueDate?: string | null
  createdBy: string
}): Promise<Task> {
  const sql = await db()
  const assignees = input.assignees ?? []
  const status = assignees.length > 0 ? 'assigned' : 'inbox'
  const id = await sql.begin(async (tx) => {
    const seq = await tx`update boards set ticket_seq = ticket_seq + 1, updated_at = now() where id = ${input.boardId} returning ticket_seq`
    const ticketNo = (seq[0] as { ticket_seq: number }).ticket_seq
    const rows = await tx`
      insert into tasks (board_id, ticket_no, title, description, priority, effort, assignees, due_date, created_by, status)
      values (${input.boardId}, ${ticketNo}, ${input.title}, ${input.description ?? null}, ${input.priority ?? 'medium'},
              ${input.effort ?? null}, ${sql.json(assignees)}, ${input.dueDate ?? null}, ${input.createdBy}, ${status})
      returning id
    `
    return (rows[0] as { id: string }).id
  })
  await logActivity(id, input.createdBy, 'created', 'created this task')
  if (assignees.length) await logActivity(id, input.createdBy, 'assigned', `assigned to ${assignees.join(', ')}`)
  publishBoard(input.boardId, { type: 'task', taskId: id })
  return (await getTask(id))!
}

export interface TaskPatch {
  title?: string
  description?: string | null
  status?: TaskStatus
  priority?: Priority
  effort?: Effort | null
  assignees?: string[]
  dueDate?: string | null
  tags?: string[]
  outcome?: string | null
  resolution?: string | null
  errorMessage?: string | null
  archived?: boolean
  /** Full replacement list of attachment chips (uploads + refs), already
   *  resolved/ACL-checked by the route — same shape as message attachments. */
  attachments?: unknown[]
  /** Seconds to add to accumulated time-spent (agents report per iteration). */
  addTimeSpentSeconds?: number
}

export async function updateTask(id: string, patch: TaskPatch, actor: string): Promise<Task | null> {
  const sql = await db()
  const cur = await getTask(id)
  if (!cur) return null

  const pick = <T>(v: T | undefined, fallback: T): T => (v === undefined ? fallback : v)
  const assignees = patch.assignees ?? cur.assignees
  const attachments = patch.attachments ?? cur.attachments
  const next = {
    title: patch.title ?? cur.title,
    description: pick(patch.description, cur.description),
    effort: pick(patch.effort, cur.effort),
    priority: patch.priority ?? cur.priority,
    dueDate: pick(patch.dueDate, cur.dueDate),
    tags: patch.tags ?? cur.tags,
    outcome: pick(patch.outcome, cur.outcome),
    resolution: pick(patch.resolution, cur.resolution),
    errorMessage: pick(patch.errorMessage, cur.errorMessage),
    status: (patch.status ?? (assignees.length && cur.status === 'inbox' ? 'assigned' : cur.status)) as TaskStatus,
  }
  const completedAt = next.status === 'done' ? (cur.completedAt ?? new Date().toISOString()) : null
  const archivedAt =
    patch.archived === undefined ? cur.archivedAt : patch.archived ? (cur.archivedAt ?? new Date().toISOString()) : null
  const addSeconds = Math.max(0, Math.round(patch.addTimeSpentSeconds ?? 0))

  await sql`
    update tasks set title=${next.title}, description=${next.description}, status=${next.status},
      priority=${next.priority}, effort=${next.effort}, assignees=${sql.json(assignees)}, due_date=${next.dueDate},
      tags=${sql.json(next.tags as unknown as Parameters<typeof sql.json>[0])},
      attachments=${sql.json(attachments as unknown as Parameters<typeof sql.json>[0])},
      outcome=${next.outcome}, resolution=${next.resolution}, error_message=${next.errorMessage},
      completed_at=${completedAt}, archived_at=${archivedAt},
      time_spent_seconds=time_spent_seconds + ${addSeconds}, updated_at=now()
    where id=${id}
  `
  if (patch.archived !== undefined && patch.archived !== !!cur.archivedAt)
    await logActivity(id, actor, 'archived', patch.archived ? 'archived this ticket' : 'restored this ticket')

  if (patch.status && patch.status !== cur.status) await logActivity(id, actor, 'status', `moved to ${patch.status}`)
  if (patch.assignees !== undefined && assignees.join(',') !== cur.assignees.join(','))
    await logActivity(id, actor, 'assigned', assignees.length ? `assigned to ${assignees.join(', ')}` : 'unassigned')
  if (patch.priority && patch.priority !== cur.priority) await logActivity(id, actor, 'priority', `priority → ${patch.priority}`)
  if (patch.effort !== undefined && patch.effort !== cur.effort)
    await logActivity(id, actor, 'effort', patch.effort ? `effort → ${patch.effort.toUpperCase()}` : 'effort cleared')
  if (patch.outcome && patch.outcome !== cur.outcome) await logActivity(id, actor, 'outcome', 'reported an outcome')
  if (patch.attachments !== undefined && attachments.length !== cur.attachments.length)
    await logActivity(
      id,
      actor,
      'attachments',
      attachments.length > cur.attachments.length ? `attached ${attachments.length - cur.attachments.length} file(s)` : 'removed an attachment',
    )
  publishBoard(cur.boardId, { type: 'task', taskId: id })
  return getTask(id)
}

export async function deleteTask(id: string): Promise<void> {
  const sql = await db()
  const boardId = await taskBoardId(id)
  await sql`delete from tasks where id = ${id}`
  if (boardId) publishBoard(boardId, { type: 'task', taskId: id, deleted: true })
}

// ── Comments ─────────────────────────────────────────────────────────────────
export async function listComments(taskId: string): Promise<TaskComment[]> {
  const sql = await db()
  return (await sql`
    select id, author, content, parent_id as "parentId", created_at as "createdAt"
    from task_comments where task_id = ${taskId} order by created_at asc
  `) as unknown as TaskComment[]
}
export async function addComment(taskId: string, author: string, content: string, parentId?: string): Promise<TaskComment> {
  const sql = await db()
  const rows = await sql`
    insert into task_comments (task_id, author, content, parent_id)
    values (${taskId}, ${author}, ${content}, ${parentId ?? null})
    returning id, author, content, parent_id as "parentId", created_at as "createdAt"
  `
  await logActivity(taskId, author, 'comment', 'commented')
  const bid = await taskBoardId(taskId)
  if (bid) publishBoard(bid, { type: 'comment', taskId })
  return rows[0] as unknown as TaskComment
}

// ── Watchers ─────────────────────────────────────────────────────────────────
export async function listWatchers(taskId: string): Promise<string[]> {
  const sql = await db()
  const rows = await sql`select watcher from task_watchers where task_id = ${taskId} order by created_at asc`
  return (rows as unknown as Array<{ watcher: string }>).map((r) => r.watcher)
}
export async function addWatcher(taskId: string, watcher: string): Promise<void> {
  const sql = await db()
  await sql`insert into task_watchers (task_id, watcher) values (${taskId}, ${watcher}) on conflict do nothing`
}
export async function removeWatcher(taskId: string, watcher: string): Promise<void> {
  const sql = await db()
  await sql`delete from task_watchers where task_id = ${taskId} and watcher = ${watcher}`
}

// ── Quality reviews (approval gate) ──────────────────────────────────────────
export async function listReviews(taskId: string): Promise<QualityReview[]> {
  const sql = await db()
  return (await sql`
    select id, reviewer, status, notes, created_at as "createdAt"
    from quality_reviews where task_id = ${taskId} order by created_at desc
  `) as unknown as QualityReview[]
}
export async function addReview(taskId: string, reviewer: string, status: 'approved' | 'rejected', notes?: string): Promise<void> {
  const sql = await db()
  await sql`insert into quality_reviews (task_id, reviewer, status, notes) values (${taskId}, ${reviewer}, ${status}, ${notes ?? null})`
  await logActivity(taskId, reviewer, 'review', status === 'approved' ? 'approved this task' : 'requested changes')
  const bid = await taskBoardId(taskId)
  if (bid) publishBoard(bid, { type: 'task', taskId })
}

// ── Dependencies (blocked-by / blocks) ───────────────────────────────────────
/** Returns [blockedBy, blocks] — the tickets this task depends on, and the
 *  tickets that depend on this task. */
export async function listDependencies(taskId: string): Promise<[TaskLink[], TaskLink[]]> {
  const sql = await db()
  const blockedBy = await sql.unsafe(
    `${LINK_SELECT} join task_dependencies d on d.depends_on_id = t.id where d.task_id = $1 order by t.ticket_no`,
    [taskId],
  )
  const blocks = await sql.unsafe(
    `${LINK_SELECT} join task_dependencies d on d.task_id = t.id where d.depends_on_id = $1 order by t.ticket_no`,
    [taskId],
  )
  return [blockedBy as unknown as TaskLink[], blocks as unknown as TaskLink[]]
}

export async function addDependency(taskId: string, dependsOnId: string, actor: string): Promise<void> {
  if (taskId === dependsOnId) return
  const sql = await db()
  await sql`insert into task_dependencies (task_id, depends_on_id) values (${taskId}, ${dependsOnId}) on conflict do nothing`
  await logActivity(taskId, actor, 'dependency', 'added a dependency')
  const bid = await taskBoardId(taskId)
  if (bid) publishBoard(bid, { type: 'task', taskId })
}

export async function removeDependency(taskId: string, dependsOnId: string): Promise<void> {
  const sql = await db()
  await sql`delete from task_dependencies where task_id = ${taskId} and depends_on_id = ${dependsOnId}`
  const bid = await taskBoardId(taskId)
  if (bid) publishBoard(bid, { type: 'task', taskId })
}

// ── Activity ─────────────────────────────────────────────────────────────────
export async function logActivity(taskId: string, actor: string, type: string, description: string): Promise<void> {
  const sql = await db()
  await sql`insert into task_activity (task_id, actor, type, description) values (${taskId}, ${actor}, ${type}, ${description})`
}
export async function listActivity(taskId: string): Promise<TaskActivity[]> {
  const sql = await db()
  return (await sql`
    select id, actor, type, description, created_at as "createdAt"
    from task_activity where task_id = ${taskId} order by created_at desc limit 100
  `) as unknown as TaskActivity[]
}

/** Work assigned to an agent (by name), across all boards — for heartbeat. */
export async function assignedWork(agentName: string): Promise<Array<{ id: string; title: string; description: string | null }>> {
  const sql = await db()
  const rows = await sql`
    select id, title, description from tasks
    where assignees @> ${sql.json([agentName])}::jsonb and status in ('assigned', 'in_progress')
    order by created_at asc
  `
  return rows as unknown as Array<{ id: string; title: string; description: string | null }>
}
