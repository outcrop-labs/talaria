// Task queue — Talaria owns it (ripped from mission-control). Tasks are cards on
// a board with ticket refs, effort, structured results, comments, activity,
// watchers, and a quality-review approval gate. Agents get assigned work via
// heartbeat and report via PUT /api/tasks/:id.
import { db } from './db/pg'
import { publishBoard } from './realtime'
import { taskUsage, type TaskUsage } from './usage'
import { listJudgeReviews, type JudgeReview } from './judge'
import { ensureLabels } from './labels'
import { OFF_BOARD_STATUSES, statusMeta } from './statuses'
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
  // A ticket is BORN in intake, or in the pickup queue when it arrives already
  // assigned. Neither key is invented any more: `assignedKey` is null on a board
  // with no usable agent-start column, and the honest landing spot then is
  // intake — assigned to someone, not yet in anyone's pickup queue, and
  // therefore not dispatched. `defaultKey` null means the board has no intake
  // column at all, which no legal board can reach; say so instead of dropping
  // the ticket into whatever column happened to sort first (that fallback could
  // resolve to the system Blocked column).
  const status = (assignees.length > 0 ? (meta.assignedKey ?? meta.defaultKey) : meta.defaultKey) ?? null
  if (status === null) {
    throw new Error(
      `board ${await boardLabel(input.boardId)} has no intake column, so there is nowhere to put a new ticket. Add an open-category column in board settings → statuses.`,
    )
  }
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
  // Born assigned into an agent-start column → push the work to the agents.
  void import('./work-dispatch').then(({ maybeDispatchTicket }) => maybeDispatchTicket(task)).catch(() => {})
  void notifyTaskUsers(humanAssigneeIds(assignees), input.createdBy, {
    kind: 'task-assigned',
    title: `Assigned: ${input.title}`,
    body: task.ticketRef ?? undefined,
    href: `/boards/${input.boardId}/${id}`,
  })
  publishBoard(input.boardId, { type: 'task', taskId: id })
  return task
}

// ── Human in the loop ────────────────────────────────────────────────────────
// The product promise: agents create, triage and report, but assigning work and
// signing it off are a person's call. That invariant lives HERE rather than in
// the route, so every caller inherits it — an agent write is an agent write
// whether it arrives over PUT /api/tasks/:id or from a future tool.
export interface TaskActor {
  /** `platform` is Talaria itself (the QA judge, dispatch) — trusted like a
   *  human, and named as itself in the activity log. */
  kind: 'human' | 'agent' | 'platform'
  /** Activity/notification identity: an email for humans, a model id for
   *  agents, `judge:<model>` for the platform. */
  id: string
}

/** A write that needs a person. 403, not 400 — the request was well-formed. */
export class HumanApprovalRequired extends Error {}

/** The system Blocked column (statuses.ts owns the key and refuses to let any
 *  board mint another one), so a literal is the honest handle: no board_statuses
 *  row carries the 'blocked' category, which is exactly why it fell out of every
 *  key list in statusMeta. */
const BLOCKED_STATUS = 'blocked'

/** Board name + archival state. `label` is for diagnostics (falls back to the
 *  id); `archivedAt` is load-bearing — an archived BOARD is hidden from the
 *  people watching it exactly like an archived ticket. */
async function boardInfo(boardId: string): Promise<{ label: string; archivedAt: string | null }> {
  const sql = await db()
  const rows = (await sql`select name, archived_at as "archivedAt" from boards where id = ${boardId}`) as unknown as Array<{
    name: string | null
    archivedAt: string | Date | null
  }>
  const row = rows[0]
  return {
    label: row?.name ? `"${row.name}"` : boardId,
    archivedAt: row?.archivedAt ? new Date(row.archivedAt).toISOString() : null,
  }
}

/** Board name for a diagnostic, falling back to the id. Error paths only. */
const boardLabel = async (boardId: string): Promise<string> => (await boardInfo(boardId)).label

// A misconfigured board is an OPERATOR problem, and the only party that sees
// the API response is the agent that got refused. So log it too — on a slow
// cadence per board (same idiom as agent-auth's legacy warning): a heartbeat
// loop must not bury the log, but one line from whenever the server started
// can't answer "is this still happening?".
const boardWarnedAt = new Map<string, number>()
const BOARD_WARN_EVERY_MS = 15 * 60 * 1000
function warnBoardConfig(boardId: string, line: string): void {
  const now = Date.now()
  if (now - (boardWarnedAt.get(boardId) ?? 0) < BOARD_WARN_EVERY_MS) return
  boardWarnedAt.set(boardId, now)
  console.warn(`[tasks] ${line}`)
}

/** Where an agent's terminal move actually lands.
 *
 *  THE COERCION MUST NOT INVENT A DESTINATION. statusMeta once fell back to the
 *  literal 'quality_review' when a board had no review column, so "report a
 *  result" could be rewritten into a key that had since been recategorised to
 *  `done` (ticket closed, completed_at stamped, no human) or to
 *  `active + agentStart` (handed straight back to the agent's own pickup queue,
 *  then frozen there by the review-exit rule). Both were reachable through the
 *  ordinary owner-facing statuses route.
 *
 *  So the destination is CHECKED, not assumed: a real review-category column,
 *  not a done column, not an agent-start pickup queue. When there isn't one the
 *  write is REFUSED, and the refusal names the board and the fix — a board an
 *  admin has to correct must not read as an agent that misbehaved. */
async function handoffTarget(cur: Task, meta: Awaited<ReturnType<typeof statusMeta>>): Promise<string> {
  const target = meta.reviewKey
  if (target !== null && meta.reviewKeys.includes(target) && !meta.doneKeys.includes(target) && !meta.agentStartKeys.includes(target)) {
    return target
  }
  const board = await boardLabel(cur.boardId)
  // Review columns EXIST but every one is also an agent-start pickup queue —
  // data that predates the cross-validation (statuses.ts now refuses to write
  // or keep it). Handing off there would loop, so say exactly that, and exactly
  // which switch to flip. Without this the write died on the assignment gate
  // below as a bare "agents cannot assign tickets": true of the symptom,
  // useless about the cause.
  if (meta.reviewKeys.length) {
    const cols = meta.reviewKeys.map((k) => `"${k}"`).join(', ')
    const many = meta.reviewKeys.length > 1
    const line = `board ${board} has no usable review column: ${cols} ${many ? 'are review columns' : 'is a review column'} but also flagged agent-start, so handing work off would drop it straight back into the agent pickup queue. Clear "agent start" on ${many ? 'those columns' : 'that column'} (board settings → statuses), or add a review column that agents do not pick up from.`
    warnBoardConfig(cur.boardId, line)
    throw new Error(line)
  }
  const line = `board ${board} has no review column, so there is nowhere to hand work off — a person has to close this ticket. Add a review-category column in board settings → statuses.`
  warnBoardConfig(cur.boardId, line)
  throw new Error(line)
}

/** The patch an agent is actually allowed to apply: assignment, planning and
 *  archival stripped; terminal moves redirected to the board's review catch.
 *  Throws where a weaker form would be a lie (assigning, reopening, taking work
 *  back out of review) instead of quietly doing something else. The clauses
 *  below are the whole invariant — a person assigns, a person signs off, a
 *  person unblocks, and what a person signed off on stays put. */
async function agentSafePatch(cur: Task, patch: TaskPatch, meta: Awaited<ReturnType<typeof statusMeta>>): Promise<TaskPatch> {
  // ── PRESENCE, NOT TRUTHINESS ───────────────────────────────────────────────
  // Every guard below asks `patch.status !== undefined`, never `patch.status`.
  // `status: ''` is a legal JSON value and a FALSY one, so `patch.status && …`
  // SKIPPED THE LOT — board validation, the review exit, the blocked exit, the
  // stranded-status rule and the assignment gate — on a patch that then wrote
  // an empty status straight to the column. updateTask now rejects a status
  // that is not a column on the board BEFORE we get here (so '' dies at the
  // door, from any caller, agent or not), and the route's zod carries `.min(1)`;
  // this file states presence explicitly so a third layer can't be the one that
  // was load-bearing.
  //
  // ARCHIVAL IS NOT ONE-DIRECTIONAL. `archived` is stripped below precisely
  // because archival hides work from the people watching it — which is the same
  // reason an agent may not keep WORKING something already hidden. A person
  // archives a ticket to stop it; the agent noticing and writing anyway is the
  // stop failing. (assignedWork stops serving it too, so the pull side agrees.)
  if (cur.archivedAt) {
    throw new HumanApprovalRequired('agents cannot change an archived ticket — a person restores it first')
  }
  const board = await boardInfo(cur.boardId)
  if (board.archivedAt) {
    throw new HumanApprovalRequired(`agents cannot change a ticket on an archived board — a person restores ${board.label} first`)
  }
  // Sign-off is sticky, and it covers the RECORD as well as the column: closed
  // work is what the org signed off on, so an agent does not re-title it,
  // re-word its outcome, or move it. Comments stay open — that is the agent's
  // channel on work it can no longer edit.
  if (meta.doneKeys.includes(cur.status) || OFF_BOARD_STATUSES.includes(cur.status)) {
    throw new HumanApprovalRequired('agents cannot change a closed ticket')
  }
  // A review column is the human sign-off QUEUE. Once an agent hands work over,
  // only a person — or the platform, whose judge bounces revisions back — takes
  // it out again. Otherwise the agent pulls its own work off the reviewer's
  // board and mints itself a fresh work session on every lap.
  if (meta.reviewKeys.includes(cur.status) && patch.status !== undefined && patch.status !== cur.status) {
    throw new HumanApprovalRequired('agents cannot take a ticket out of review — a person signs it off')
  }
  // Blocked is a STOP SIGNAL raised for a person, and the dispatch prompt sells
  // it as one ("set status blocked … ends the session"). An agent that can walk
  // back out erases the signal the human was watching for and, because
  // work-dispatch treats every active column as working, restarts its own
  // session. Same rule as review: raising it is the agent's call, clearing it
  // is not.
  if (cur.status === BLOCKED_STATUS && patch.status !== undefined && patch.status !== cur.status) {
    throw new HumanApprovalRequired('agents cannot move a ticket out of blocked — a person unblocks it')
  }
  // A ticket sitting in a status that is not a column on its board is STRANDED
  // (the column was deleted or recategorised under it). Nothing above can class
  // it — it is in no category — so an agent moving it would be moving work
  // nobody can see out of a state nobody chose. A person places it. This is the
  // honest replacement for statusMeta's old literal fallbacks, which used to
  // catch some of these cases by accident and mis-class the rest.
  if (patch.status !== undefined && patch.status !== cur.status && !meta.keys.includes(cur.status)) {
    throw new HumanApprovalRequired(`"${cur.status}" is no longer a column on this board — a person has to place this ticket`)
  }
  // Assignment stays human; so do the planning fields (estimate, sub-task
  // structure) and archival, which hides work from the people watching it.
  const next: TaskPatch = { ...patch, assignees: undefined, estimatedHours: undefined, parentId: undefined, archived: undefined }
  if (next.status === undefined) return next
  // Every terminal move lands in review instead — the board's done columns AND
  // the off-board keys. Reporting a result is the agent's job, closing on it
  // is not. handoffTarget REFUSES rather than guessing when the board has no
  // column that can hold a hand-off; it never returns a done column or an
  // agent-start one, which is what made this rewrite launderable. The result
  // still FALLS THROUGH to the assignment gate rather than returning: a coerced
  // destination is still a destination.
  if (meta.doneKeys.includes(next.status) || OFF_BOARD_STATUSES.includes(next.status)) {
    next.status = (await handoffTarget(cur, meta)) as TaskStatus
  }
  // ENTERING an agent-start column is assignment — the destination is the gate,
  // whatever column the ticket came from and whether the agent named that
  // column or the coercion above picked it. Only a ticket already sitting in
  // one moves freely (an agent working what it was given). Gating on the SOURCE
  // let any intermediate status launder a self-assignment in two writes, so a
  // human dragging work back to intake to stop an agent didn't actually stop it.
  if (meta.agentStartKeys.includes(next.status) && !meta.agentStartKeys.includes(cur.status)) {
    throw new HumanApprovalRequired('agents cannot assign tickets')
  }
  return next
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

export async function updateTask(id: string, patch: TaskPatch, who: TaskActor): Promise<Task | null> {
  const sql = await db()
  const cur = await getTask(id)
  if (!cur) return null

  const pick = <T>(v: T | undefined, fallback: T): T => (v === undefined ? fallback : v)
  const meta = await statusMeta(cur.boardId)
  // PRESENCE, not truthiness — and this is the door every other status rule
  // stands behind. `status: ''` passed `patch.status &&` as absent while the
  // writer below treated it as present, so an agent could blank a ticket's
  // column with every guard in agentSafePatch skipped. Asking `!== undefined`
  // sends '' straight into the board-membership check, which no board can
  // satisfy: '' is not a column anywhere, so it is refused here, once, for
  // every caller — route, judge, workbench or a future tool.
  if (patch.status !== undefined && ![...meta.keys, ...OFF_BOARD_STATUSES].includes(patch.status)) {
    throw new Error(`"${patch.status}" is not a status on this board`)
  }
  if (who.kind === 'agent') {
    patch = await agentSafePatch(cur, patch, meta)
    // Post-condition on the REWRITTEN patch (terminal moves land in the review
    // catch): whatever an agent write is about to store names a column this
    // board actually has. handoffTarget now guarantees the category as well —
    // asserting only that the key EXISTS was what let a review key that had
    // been recategorised to `done` pass straight through. Belt to its braces:
    // no path may store a status the board cannot render.
    if (patch.status !== undefined && !meta.keys.includes(patch.status)) {
      throw new Error(`"${patch.status}" is not a status on this board`)
    }
  }
  const assignees = patch.assignees ?? cur.assignees
  const attachments = patch.attachments ?? cur.attachments
  if (patch.parentId) await assertValidParent(id, patch.parentId, cur.boardId)
  if (patch.tags?.length) await ensureLabels(cur.boardId, patch.tags)
  // Assignees on an intake ticket promote it into the pickup queue — approval
  // by assignment, so an agent's own write never triggers it. The DESTINATION
  // is never invented: with no usable agent-start column (`assignedKey` null)
  // there is no pickup queue to promote into, and the ticket stays in intake
  // rather than being moved somewhere that merely sorted first — which, before
  // `assignedKey` got this treatment, could be a DONE column, so assigning
  // someone closed the ticket.
  const promotedTo =
    who.kind !== 'agent' && assignees.length > 0 && meta.defaultKey !== null && cur.status === meta.defaultKey
      ? meta.assignedKey
      : null
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
    status: (patch.status ?? promotedTo ?? cur.status) as TaskStatus,
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
    await logActivity(id, who.id, 'archived', patch.archived ? 'archived this ticket' : 'restored this ticket')

  if (patch.status !== undefined && patch.status !== cur.status) {
    await logActivity(id, who.id, 'status', `moved to ${patch.status}`)
    // Moving INTO an agent-start column re-dispatches to agent assignees
    // (human approval by column move) — but never into a column that CLOSES the
    // ticket. createStatus/updateStatus refuse `done + agent_start` now; this is
    // the same rule stated where the damage happened, so a legacy row written
    // before that guard can't still turn signing work off into dispatching it.
    if (
      meta.agentStartKeys.includes(patch.status) &&
      !meta.doneKeys.includes(patch.status) &&
      !meta.agentStartKeys.includes(cur.status)
    ) {
      void (async () => {
        const fresh = await getTask(id)
        if (fresh) await (await import('./work-dispatch')).maybeDispatchTicket(fresh)
      })().catch(() => {})
    }
    // Watchers + assigned humans hear about status moves (never the actor).
    const audience = [...(await watcherUserIds(id)), ...humanAssigneeIds(assignees)]
    void notifyTaskUsers(audience, who.id, {
      kind: 'task-status',
      title: `${cur.ticketRef ?? cur.title}: ${patch.status.replace('_', ' ')}`,
      body: cur.title,
      href: `/boards/${cur.boardId}/${id}`,
    })
  }
  if (patch.assignees !== undefined && assignees.join(',') !== cur.assignees.join(',')) {
    await logActivity(id, who.id, 'assigned', assignees.length ? `assigned to ${assignees.join(', ')}` : 'unassigned')
    // NEWLY added agents get the work pushed into their run loop.
    const addedAgents = agentAssignees(assignees).filter((a) => !agentAssignees(cur.assignees).includes(a))
    if (addedAgents.length) {
      void (async () => {
        const fresh = await getTask(id)
        if (fresh) await (await import('./work-dispatch')).maybeDispatchTicket(fresh, addedAgents)
      })().catch(() => {})
    }
    // NEWLY added humans get an inbox nudge.
    const added = humanAssigneeIds(assignees).filter((uid) => !humanAssigneeIds(cur.assignees).includes(uid))
    void notifyTaskUsers(added, who.id, {
      kind: 'task-assigned',
      title: `Assigned: ${cur.title}`,
      body: cur.ticketRef ?? undefined,
      href: `/boards/${cur.boardId}/${id}`,
    })
  }
  if (patch.priority !== undefined && patch.priority !== cur.priority)
    await logActivity(id, who.id, 'priority', `priority → ${patch.priority}`)
  if (patch.effort !== undefined && patch.effort !== cur.effort)
    await logActivity(id, who.id, 'effort', patch.effort ? `effort → ${patch.effort.toUpperCase()}` : 'effort cleared')
  // `estimatedHours: 0` is a real estimate and a FALSY one — it used to be
  // stored as 0 and reported as "estimate cleared". null is the clear.
  if (patch.estimatedHours !== undefined && patch.estimatedHours !== cur.estimatedHours)
    await logActivity(id, who.id, 'estimate', patch.estimatedHours === null ? 'estimate cleared' : `estimate → ${patch.estimatedHours}h`)
  if (patch.parentId !== undefined && patch.parentId !== cur.parentId)
    await logActivity(id, who.id, 'parent', patch.parentId ? 'made a sub-task' : 'promoted to top level')
  // ── The reported RESULT, and the labels ────────────────────────────────────
  // Presence, not truthiness, again — and here the falsy value is the DAMAGING
  // one. `if (patch.outcome && …)` meant an agent could BLANK the outcome a
  // reviewer was about to read (`outcome: ''`, or null) and leave nothing on the
  // record: the ticket sat in review with an empty result and an activity log
  // that still said "reported an outcome" from the write before. Same for the
  // rest of the result fields, which had no activity line at all, and for tags,
  // where `tags: []` cleared every label silently.
  if (patch.outcome !== undefined && patch.outcome !== cur.outcome)
    await logActivity(id, who.id, 'outcome', patch.outcome ? 'reported an outcome' : 'cleared the outcome')
  if (patch.resolution !== undefined && patch.resolution !== cur.resolution)
    await logActivity(id, who.id, 'resolution', patch.resolution ? 'wrote a resolution' : 'cleared the resolution')
  if (patch.errorMessage !== undefined && patch.errorMessage !== cur.errorMessage)
    await logActivity(id, who.id, 'error', patch.errorMessage ? 'reported an error' : 'cleared the error')
  if (patch.tags !== undefined && JSON.stringify(next.tags) !== JSON.stringify(cur.tags))
    await logActivity(id, who.id, 'tags', next.tags.length ? `labels → ${next.tags.join(', ')}` : 'cleared every label')
  // Length told you nothing about a 1-for-1 swap: replacing the evidence on a
  // ticket in review was invisible. Compare the LIST.
  if (patch.attachments !== undefined && JSON.stringify(attachments) !== JSON.stringify(cur.attachments))
    await logActivity(
      id,
      who.id,
      'attachments',
      attachments.length > cur.attachments.length
        ? `attached ${attachments.length - cur.attachments.length} file(s)`
        : attachments.length < cur.attachments.length
          ? 'removed an attachment'
          : 'replaced an attachment',
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

/** Adding `task → dependsOn` closes a cycle iff `dependsOn` already reaches
 *  `task` through the existing edges. Self-edges are the length-1 case and were
 *  the ONLY case this guarded, so an agent could write X blocked-by Y and then
 *  Y blocked-by X — a dependency graph no ticket in it can ever satisfy, with
 *  nothing in the product able to tell the operator why nothing unblocks. One
 *  recursive walk, on the write, is the cheap place to say no. */
async function wouldCycle(taskId: string, dependsOnId: string): Promise<boolean> {
  if (taskId === dependsOnId) return true
  const sql = await db()
  const rows = await sql`
    with recursive reach(id) as (
      select depends_on_id from task_dependencies where task_id = ${dependsOnId}
      union
      select d.depends_on_id from task_dependencies d join reach r on d.task_id = r.id
    )
    select 1 from reach where id = ${taskId} limit 1
  `
  return rows.length > 0
}

export async function addDependency(taskId: string, dependsOnId: string, actor: string): Promise<void> {
  if (await wouldCycle(taskId, dependsOnId)) {
    throw new Error(
      taskId === dependsOnId
        ? 'a ticket cannot block itself'
        : 'that would make the two tickets block each other — neither could ever be unblocked',
    )
  }
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
export async function assignedWork(agentName: string): Promise<Array<{ id: string; title: string; description: string | null; workflows: unknown[] }>> {
  const sql = await db()
  // Custom-status boards: pickup = statuses flagged agent_start. Boards
  // without rows keep the classic ('assigned','in_progress') semantics.
  //
  // ARCHIVED WORK IS NOT SERVED. Archival is how a person takes work off the
  // table — agentSafePatch strips `archived` from an agent patch for exactly
  // that reason — but this query had no archival predicate at all, so a human
  // archiving a ticket (or a whole board) left the heartbeat handing it back on
  // the very next poll and the agent carried on working something nobody was
  // watching. Both levels: the ticket AND its board.
  const rows = await sql`
    select t.id, t.title, t.description, t.tags, t.board_id as "boardId"
    from tasks t join boards b on b.id = t.board_id
    where t.assignees @> ${sql.json([agentName])}::jsonb
      and t.archived_at is null
      and b.archived_at is null
      and (
        t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.agent_start)
        or (
          not exists (select 1 from board_statuses bs where bs.board_id = t.board_id)
          and t.status in ('assigned', 'in_progress')
        )
      )
    order by t.created_at asc
  `
  // Matched workflows ride with the pull channel too (plugin-side dispatch).
  const { workflowsForTask } = await import('./workflows')
  const items = rows as unknown as Array<{ id: string; title: string; description: string | null; tags: string[]; boardId: string }>
  return Promise.all(
    items.map(async (t) => ({ ...t, workflows: await workflowsForTask(t) })),
  )
}
