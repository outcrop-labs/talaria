// Task queue — Talaria owns it (ripped from mission-control). Tasks are cards on
// a board with ticket refs, effort, structured results, comments, activity,
// watchers, and a quality-review approval gate. Agents get assigned work via
// heartbeat and report via PUT /api/tasks/:id.
import { db } from './db/pg'
import { publishBoard } from './realtime'
import { taskUsage, type TaskUsage } from './usage'
import { listJudgeReviews, type JudgeReview } from './judge'
import { ensureLabels } from './labels'
import { statusMeta } from './statuses'
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
  t.due_date as "dueDate", t.start_date as "startDate", t.color, t.tags, t.attachments, t.time_spent_seconds as "timeSpentSeconds",
  t.estimated_hours as "estimatedHours", t.parent_id as "parentId",
  (select count(*)::int from task_comments c where c.task_id = t.id) as "commentCount",
  t.outcome, t.resolution, t.error_message as "errorMessage",
  t.created_at as "createdAt", t.updated_at as "updatedAt", t.completed_at as "completedAt",
  t.archived_at as "archivedAt"
  from tasks t join boards b on b.id = t.board_id`

// ── Mixed assignees ──────────────────────────────────────────────────────────
// The assignees array mixes AGENT model ids (bare strings, unchanged — the
// heartbeat/outreach `@>` predicates keep matching) and HUMANS as
// `user:<uuid>`. Helpers below split the two worlds.
export const isHumanAssignee = (a: string): boolean => a.startsWith('user:')
export const humanAssigneeIds = (assignees: string[]): string[] =>
  assignees.filter(isHumanAssignee).map((a) => a.slice(5))
export const agentAssignees = (assignees: string[]): string[] => assignees.filter((a) => !isHumanAssignee(a))

/** Notify users about a task event (self-notifications excluded by actor email). */
async function notifyTaskUsers(
  userIds: string[],
  actor: string,
  n: { kind: string; title: string; body?: string; href?: string },
): Promise<void> {
  if (userIds.length === 0) return
  const sql = await db()
  const { addNotification } = await import('./notifications')
  const self = actor.includes('@')
    ? ((await sql`select id from users where lower(email) = ${actor.toLowerCase()}`) as unknown as Array<{ id: string }>)[0]?.id
    : null
  for (const id of new Set(userIds)) {
    if (id === self) continue
    await addNotification(id, n).catch(() => {})
  }
}

/** Resolve watcher strings (emails) to user ids for notification fan-out. */
async function watcherUserIds(taskId: string): Promise<string[]> {
  const sql = await db()
  const watchers = await listWatchers(taskId)
  const emails = watchers.filter((w) => w.includes('@')).map((w) => w.toLowerCase())
  if (emails.length === 0) return []
  const rows = (await sql`select id from users where lower(email) = any(${emails})`) as unknown as Array<{ id: string }>
  return rows.map((r) => r.id)
}

/** Sub-task guard: one level deep, same board, no self/cycle. Throws with a
 *  human-readable reason. */
export async function assertValidParent(taskId: string | null, parentId: string, boardId: string): Promise<void> {
  if (taskId && taskId === parentId) throw new Error('a ticket cannot be its own parent')
  const parent = await getTask(parentId)
  if (!parent || parent.boardId !== boardId) throw new Error('parent must be a ticket on the same board')
  if (parent.parentId) throw new Error('sub-tasks go one level deep — that ticket is already a sub-task')
  if (taskId) {
    const sql = await db()
    const kids = await sql`select 1 from tasks where parent_id = ${taskId} limit 1`
    if (kids.length) throw new Error('this ticket has sub-tasks of its own — it cannot become a sub-task')
  }
}

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
  startDate?: string | null
  color?: string | null
  estimatedHours?: number | null
  parentId?: string | null
  tags?: string[]
  createdBy: string
}): Promise<Task> {
  const sql = await db()
  const assignees = input.assignees ?? []
  const meta = await statusMeta(input.boardId)
  const status = assignees.length > 0 ? meta.assignedKey : meta.defaultKey
  if (input.parentId) await assertValidParent(null, input.parentId, input.boardId)
  if (input.tags?.length) await ensureLabels(input.boardId, input.tags)
  const id = await sql.begin(async (tx) => {
    const seq = await tx`update boards set ticket_seq = ticket_seq + 1, updated_at = now() where id = ${input.boardId} returning ticket_seq`
    const ticketNo = (seq[0] as { ticket_seq: number }).ticket_seq
    const rows = await tx`
      insert into tasks (board_id, ticket_no, title, description, priority, effort, assignees, due_date, start_date, color, estimated_hours, parent_id, tags, created_by, status)
      values (${input.boardId}, ${ticketNo}, ${input.title}, ${input.description ?? null}, ${input.priority ?? 'medium'},
              ${input.effort ?? null}, ${sql.json(assignees)}, ${input.dueDate ?? null}, ${input.startDate ?? null}, ${input.color ?? null}, ${input.estimatedHours ?? null},
              ${input.parentId ?? null}, ${sql.json(input.tags ?? [])}, ${input.createdBy}, ${status})
      returning id
    `
    return (rows[0] as { id: string }).id
  })
  await logActivity(id, input.createdBy, 'created', 'created this task')
  if (assignees.length) await logActivity(id, input.createdBy, 'assigned', `assigned to ${assignees.join(', ')}`)
  const task = (await getTask(id))!
  void notifyTaskUsers(humanAssigneeIds(assignees), input.createdBy, {
    kind: 'task-assigned',
    title: `Assigned: ${input.title}`,
    body: task.ticketRef ?? undefined,
    href: `/boards/${input.boardId}/${id}`,
  })
  publishBoard(input.boardId, { type: 'task', taskId: id })
  return task
}

export interface TaskPatch {
  title?: string
  description?: string | null
  status?: TaskStatus
  priority?: Priority
  effort?: Effort | null
  assignees?: string[]
  dueDate?: string | null
  startDate?: string | null
  color?: string | null
  tags?: string[]
  outcome?: string | null
  resolution?: string | null
  errorMessage?: string | null
  archived?: boolean
  /** Human planning estimate, in hours (null clears). */
  estimatedHours?: number | null
  /** Sub-task parent (null promotes back to top level). */
  parentId?: string | null
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
  const meta = await statusMeta(cur.boardId)
  if (patch.status && ![...meta.keys, 'failed', 'cancelled'].includes(patch.status)) {
    throw new Error(`"${patch.status}" is not a status on this board`)
  }
  if (patch.parentId) await assertValidParent(id, patch.parentId, cur.boardId)
  if (patch.tags?.length) await ensureLabels(cur.boardId, patch.tags)
  const next = {
    title: patch.title ?? cur.title,
    description: pick(patch.description, cur.description),
    effort: pick(patch.effort, cur.effort),
    priority: patch.priority ?? cur.priority,
    dueDate: pick(patch.dueDate, cur.dueDate),
    startDate: pick(patch.startDate, cur.startDate),
    color: pick(patch.color, cur.color),
    estimatedHours: pick(patch.estimatedHours, cur.estimatedHours),
    parentId: pick(patch.parentId, cur.parentId),
    tags: patch.tags ?? cur.tags,
    outcome: pick(patch.outcome, cur.outcome),
    resolution: pick(patch.resolution, cur.resolution),
    errorMessage: pick(patch.errorMessage, cur.errorMessage),
    status: (patch.status ?? (assignees.length && cur.status === meta.defaultKey ? meta.assignedKey : cur.status)) as TaskStatus,
  }
  const completedAt = meta.doneKeys.includes(next.status) ? (cur.completedAt ?? new Date().toISOString()) : null
  const archivedAt =
    patch.archived === undefined ? cur.archivedAt : patch.archived ? (cur.archivedAt ?? new Date().toISOString()) : null
  const addSeconds = Math.max(0, Math.round(patch.addTimeSpentSeconds ?? 0))

  await sql`
    update tasks set title=${next.title}, description=${next.description}, status=${next.status},
      priority=${next.priority}, effort=${next.effort}, assignees=${sql.json(assignees)}, due_date=${next.dueDate}, start_date=${next.startDate}, color=${next.color},
      estimated_hours=${next.estimatedHours}, parent_id=${next.parentId},
      tags=${sql.json(next.tags as unknown as Parameters<typeof sql.json>[0])},
      attachments=${sql.json(attachments as unknown as Parameters<typeof sql.json>[0])},
      outcome=${next.outcome}, resolution=${next.resolution}, error_message=${next.errorMessage},
      completed_at=${completedAt}, archived_at=${archivedAt},
      time_spent_seconds=time_spent_seconds + ${addSeconds}, updated_at=now()
    where id=${id}
  `
  if (patch.archived !== undefined && patch.archived !== !!cur.archivedAt)
    await logActivity(id, actor, 'archived', patch.archived ? 'archived this ticket' : 'restored this ticket')

  if (patch.status && patch.status !== cur.status) {
    await logActivity(id, actor, 'status', `moved to ${patch.status}`)
    // Watchers + assigned humans hear about status moves (never the actor).
    const audience = [...(await watcherUserIds(id)), ...humanAssigneeIds(assignees)]
    void notifyTaskUsers(audience, actor, {
      kind: 'task-status',
      title: `${cur.ticketRef ?? cur.title}: ${patch.status.replace('_', ' ')}`,
      body: cur.title,
      href: `/boards/${cur.boardId}/${id}`,
    })
  }
  if (patch.assignees !== undefined && assignees.join(',') !== cur.assignees.join(',')) {
    await logActivity(id, actor, 'assigned', assignees.length ? `assigned to ${assignees.join(', ')}` : 'unassigned')
    // NEWLY added humans get an inbox nudge.
    const added = humanAssigneeIds(assignees).filter((uid) => !humanAssigneeIds(cur.assignees).includes(uid))
    void notifyTaskUsers(added, actor, {
      kind: 'task-assigned',
      title: `Assigned: ${cur.title}`,
      body: cur.ticketRef ?? undefined,
      href: `/boards/${cur.boardId}/${id}`,
    })
  }
  if (patch.priority && patch.priority !== cur.priority) await logActivity(id, actor, 'priority', `priority → ${patch.priority}`)
  if (patch.effort !== undefined && patch.effort !== cur.effort)
    await logActivity(id, actor, 'effort', patch.effort ? `effort → ${patch.effort.toUpperCase()}` : 'effort cleared')
  if (patch.estimatedHours !== undefined && patch.estimatedHours !== cur.estimatedHours)
    await logActivity(id, actor, 'estimate', patch.estimatedHours ? `estimate → ${patch.estimatedHours}h` : 'estimate cleared')
  if (patch.parentId !== undefined && patch.parentId !== cur.parentId)
    await logActivity(id, actor, 'parent', patch.parentId ? 'made a sub-task' : 'promoted to top level')
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
  // Custom-status boards: pickup = statuses flagged agent_start. Boards
  // without rows keep the classic ('assigned','in_progress') semantics.
  const rows = await sql`
    select t.id, t.title, t.description from tasks t
    where t.assignees @> ${sql.json([agentName])}::jsonb
      and (
        t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.agent_start)
        or (
          not exists (select 1 from board_statuses bs where bs.board_id = t.board_id)
          and t.status in ('assigned', 'in_progress')
        )
      )
    order by t.created_at asc
  `
  return rows as unknown as Array<{ id: string; title: string; description: string | null }>
}
