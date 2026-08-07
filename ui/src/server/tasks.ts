// Task queue — Talaria owns it (ripped from mission-control). Tasks are cards on
// a board with ticket refs, effort, structured results, comments, activity,
// watchers, and a quality-review approval gate. Agents get assigned work via
// heartbeat and report via PUT /api/tasks/:id.
import { db } from './db/pg'
import { guardAgentFields, guardAgentWrite } from './agent-writes'
import { publishBoard } from './realtime'
import { taskUsage, type TaskUsage } from './usage'
import { listJudgeReviews, type JudgeReview } from './judge'
import { ensureLabels } from './labels'
import { OFF_BOARD_STATUSES, statusMeta, type StatusMeta } from './statuses'
import { boardAllowsAgent, boardFacts, boardInfo, boardRole, type BoardFacts } from './boards'
import { subjectModel, type AgentSubject } from './agent-auth'
import { pgNum } from '@/lib/task-const'
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
  t.estimated_hours::float8 as "estimatedHours", t.parent_id as "parentId",
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

/** Everyone who may be TOLD about an event on this ticket: the people watching
 *  it and the humans assigned to it, each confirmed against the board AS IT
 *  STANDS NOW.
 *
 *  ASKED AT FAN-OUT TIME, NOT TRUSTED FROM THE WRITE, and that is the whole
 *  point of it being one function. Both halves are validated when they are
 *  written — `invalidAssignee` on the two ticket write routes, `addWatcher`
 *  below — and neither check survives the day after:
 *
 *    · `unshareBoard` deletes a `board_members` row and touches NOTHING else.
 *      `tasks.assignees` keeps the `user:<uuid>`, `task_watchers` keeps the
 *      row, and every later status move mails both of them a ticket they now
 *      get a 403 from.
 *    · A watcher was never validated at all (see the Watchers section), so the
 *      stored string could name anyone with an account.
 *
 *  `maybeDispatchTicket` already works this way for the AGENT audience: it
 *  re-asks `agentTicketRefusal` per agent rather than trusting the assignment
 *  that put the agent there. This is the same rule for the human audience —
 *  membership is read at the moment of the send, by the one function both
 *  ticket fan-outs call, so a second fan-out cannot be added that skips it. */
async function ticketAudience(taskId: string, boardId: string, assignees: string[]): Promise<string[]> {
  const audience = (await resolveWatchers(taskId, boardId)).map((w) => w.userId)
  for (const userId of humanAssigneeIds(assignees)) {
    if (await boardRole(userId, boardId)) audience.push(userId)
  }
  return audience
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
  // THE ONE DOOR, for the fourth write path (agent-writes.ts). `create_ticket`
  // is an MCP tool and its title and description are agent-authored text on its
  // way to a human — and, through `indexTicket`, on its way back into another
  // agent's context. `createdBy` is verified against agent_defs inside, so a
  // person's ticket is untouched.
  const { title, description } = await guardAgentFields('ticket-write', input.createdBy, {
    title: input.title,
    description: input.description ?? null,
  })
  const id = await sql.begin(async (tx) => {
    const seq = await tx`update boards set ticket_seq = ticket_seq + 1, updated_at = now() where id = ${input.boardId} returning ticket_seq`
    const ticketNo = (seq[0] as { ticket_seq: number }).ticket_seq
    const rows = await tx`
      insert into tasks (board_id, ticket_no, title, description, priority, effort, assignees, due_date, start_date, color, estimated_hours, parent_id, tags, created_by, status)
      values (${input.boardId}, ${ticketNo}, ${title ?? input.title}, ${description ?? null}, ${input.priority ?? 'medium'},
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
    title: `Assigned: ${title ?? input.title}`,
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

/** Board name for a diagnostic, falling back to the id. Error paths only. */
const boardLabel = async (boardId: string): Promise<string> => (await boardInfo(boardId)).label

// ── THE agent-authority question, as ONE exported predicate ──────────────────
// "May this agent act on this ticket?" had four answers in the tree, and each
// round of this work found the next one by fixing the last. The history is the
// argument for what follows:
//   · `agentSafePatch` held it privately, and four side doors that never reach
//     `updateTask` (log_usage, dependency edges, gap reports, the workbench MCP
//     ticket gate) COPIED the closed-status third of it — so an agent could
//     write to an archived ticket through any of the four.
//   · `closedToAgents` then held it, imported by eight call sites — and it asked
//     nothing about BOARD POLICY, so a board owner revoking an agent's grant
//     403'd every write route while `assignedWork` kept serving the ticket on
//     every heartbeat.
//   · `ticketState` in work-dispatch.ts was a fourth spelling that asked NEITHER
//     archival clause, so archiving a ticket (or its board) did not stop the
//     live work session already running on it.
// So the predicate now asks BOTH halves of the question — the board's agent
// policy AND the ticket's own state — and the agent is a required argument. It
// is no longer possible to ask half of it, because there is no longer a
// function that answers half of it.
//
// It returns the REASON rather than a boolean so the refusal is the same
// sentence wherever the write arrived, and a new caller cannot invent a vaguer
// one.

/** The ticket shape the predicate needs. `getTask` returns it; so does the row
 *  the heartbeat query selects. */
export interface AgentWriteTarget {
  boardId: string
  status: string
  archivedAt?: string | null
}

/** What the agent is trying to do. The two verbs differ in EXACTLY one clause,
 *  which is why they are one function and not two:
 *   · `write`   — change the ticket, or hang a record off it (status, fields,
 *                 dependency edges, spend rows, workbench jobs).
 *   · `comment` — say something on it. A CLOSED ticket still takes comments:
 *                 that is the agent's channel on work it can no longer edit,
 *                 and it is deliberate. Archival is not that; see below. */
export type AgentIntent = 'write' | 'comment'

/**
 * Why an agent may not act on this ticket, or null when it may.
 *
 * FOUR STOP CONDITIONS, in the order a person would ask them:
 *   · the BOARD does not allow this agent — revoked, never granted, or the
 *     board is archived//gone. The board question and the ticket question were
 *     separate predicates and the pull side asked only one of them;
 *   · the ticket is ARCHIVED — a person took this work off the table;
 *   · its BOARD is archived — the same act, one level up;
 *   · its status is CLOSED — a done column on this board, or an off-board
 *     terminal key. Sign-off is sticky and covers the record, not just the
 *     column. WRITE ONLY: see `AgentIntent`.
 *
 * WHY ARCHIVAL STOPS COMMENTS TOO, at both levels. The comment exemption exists
 * for work "the agent can no longer edit" but a person is still looking at — a
 * signed-off ticket sitting in a done column on a live board. Archival is not
 * that: it withdraws the work from view. Board archival already refused agent
 * comments (it runs through `boardAllowsAgent`) while ticket archival did not,
 * which is one act of a person's meaning two different things one level apart.
 * Made consistent in the direction that keeps the exemption's stated reason
 * true: closed keeps its channel, archived does not have one, at either level.
 * READS are not this question — an agent that can see the board can read the
 * ticket, because reading changes nothing.
 *
 * `facts` is a per-pass CACHE, never an answer: pass one when looping so the
 * board is resolved once, omit it and one is made here.
 */
export async function agentTicketRefusal(
  task: AgentWriteTarget,
  agent: AgentSubject,
  intent: AgentIntent,
  facts: BoardFacts = boardFacts(),
): Promise<string | null> {
  const board = await facts.info(task.boardId)
  if (!(await boardAllowsAgent(task.boardId, agent, facts))) {
    if (board.exists && board.archivedAt) {
      return `agents cannot work a ticket on an archived board — a person restores ${board.label} first`
    }
    return `agent "${subjectModel(agent)}" is not allowed on this board`
  }
  if (task.archivedAt) return 'agents cannot work an archived ticket — a person restores it first'
  if (intent === 'comment') return null
  const meta = await facts.meta(task.boardId)
  if (meta.terminal(task.status)) return 'agents cannot change a closed ticket'
  return null
}

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
 *  not a done column, not an agent-start pickup queue. That check now lives
 *  ENTIRELY in `statusMeta` — `reviewKey` is picked from `placeable` (never
 *  terminal, never Blocked) and additionally requires `!agentStart`, so all
 *  three conditions hold by construction. This function used to re-assert them
 *  here (`reviewKeys.includes(target) && !terminal(target) &&
 *  !agentStartKeys.includes(target)`), which was dead the day `placeable`
 *  landed — and a dead restatement of an invariant is the exact shape that
 *  drifts from the live one and becomes the next round's finding. A non-null
 *  `reviewKey` IS the answer. When there isn't one the write is REFUSED, and the
 *  refusal names the board and the fix — a board an admin has to correct must
 *  not read as an agent that misbehaved. */
async function handoffTarget(cur: Task, meta: StatusMeta): Promise<string> {
  if (meta.reviewKey !== null) return meta.reviewKey
  const board = await boardLabel(cur.boardId)
  // Review columns EXIST but not one of them can hold a hand-off: each is
  // either also an agent-start pickup queue (handing off would loop straight
  // back to the agent) or itself terminal (a review column labelled
  // "Cancelled"). Both are data that predates the cross-validation — statuses.ts
  // refuses to write it now — and both need an admin, so say which switch to
  // flip. Without this the write died on the assignment gate below as a bare
  // "agents cannot assign tickets": true of the symptom, useless about the cause.
  if (meta.reviewKeys.length) {
    const cols = meta.reviewKeys.map((k) => `"${k}"`).join(', ')
    const many = meta.reviewKeys.length > 1
    const line = `board ${board} has no usable review column: ${cols} ${many ? 'are review columns' : 'is a review column'} but ${many ? 'each is' : 'it is'} also flagged agent-start or itself terminal, so handing work off would drop it straight back into the agent pickup queue (or close it). Clear "agent start" on ${many ? 'those columns' : 'that column'} and rename ${many ? 'any' : 'it'} whose key is "failed"/"cancelled" (board settings → statuses), or add a review column that agents do not pick up from.`
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
async function agentSafePatch(
  cur: Task,
  patch: TaskPatch,
  agent: AgentSubject,
  meta: StatusMeta,
  facts: BoardFacts,
): Promise<TaskPatch> {
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
  // stop failing. That, its board-level twin, the board's agent policy and the
  // closed-status rule are all `agentTicketRefusal` now — the same question the
  // side doors, the heartbeat and both dispatch sides ask, from the one
  // definition above.
  const shut = await agentTicketRefusal(cur, agent, 'write', facts)
  if (shut) throw new HumanApprovalRequired(shut)
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
  if (meta.terminal(next.status)) {
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
  /** WHY the column moved, when the reason is not visible from the move itself
   *  — appended to the one 'status' activity line this function writes. It
   *  exists so a caller with a reason does not have to become a second writer of
   *  `tasks.status` to record one: `deleteStatus` reassigns a doomed column's
   *  tickets through here and says so. Never a substitute for the move itself. */
  statusNote?: string
}

export async function updateTask(id: string, patch: TaskPatch, who: TaskActor): Promise<Task | null> {
  const sql = await db()
  const cur = await getTask(id)
  if (!cur) return null

  const pick = <T>(v: T | undefined, fallback: T): T => (v === undefined ? fallback : v)
  // One pass over this board's facts for the whole update — the agent gate, the
  // status resolvers and the dispatch decision all read from it.
  const facts = boardFacts()
  const meta = await facts.meta(cur.boardId)
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
    // `who.id` IS the agent's fleet model id (TaskActor documents it as such),
    // so the board-policy half of the invariant is answered from the same actor
    // every other clause is answered from — no route needs to remember to check
    // it separately, and one added later inherits it.
    patch = await agentSafePatch(cur, patch, who.id, meta, facts)
    // Post-condition on the REWRITTEN patch (terminal moves land in the review
    // catch): whatever an agent write is about to store names a column this
    // board actually has. handoffTarget now guarantees the category as well —
    // asserting only that the key EXISTS was what let a review key that had
    // been recategorised to `done` pass straight through. Belt to its braces:
    // no path may store a status the board cannot render.
    if (patch.status !== undefined && !meta.keys.includes(patch.status)) {
      throw new Error(`"${patch.status}" is not a status on this board`)
    }
    // THE ONE DOOR, for the fifth write path (agent-writes.ts). `triage_ticket`
    // and `report_outcome` are MCP tools, so every string below is a tool
    // argument — model output that never touched a harness. `indexTicket`
    // re-indexes title+description on every update, `notifyMentions` mails the
    // description, and the judge reads `outcome`/`resolution` straight into
    // another model's prompt: the outcome path is how an agent's own credential
    // reached a third-party judge endpoint with the guard having said so one
    // statement earlier and done nothing about it.
    patch = {
      ...patch,
      ...(await guardAgentFields('ticket-write', { agent: who.id }, {
        title: patch.title,
        description: patch.description,
        outcome: patch.outcome,
        resolution: patch.resolution,
        errorMessage: patch.errorMessage,
      })),
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
  const completedAt = completedAtFor(meta, next.status, cur.completedAt)
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

  // THE COLUMN MOVED — however it moved. Gating this on `patch.status !== undefined`
  // meant PROMOTION BY ASSIGNMENT left nothing on the record: a person patching
  // only `assignees` moved the ticket out of intake and into the agent pickup
  // queue, and the ticket's own history said only "assigned to …". The watchers
  // were not told either. `next.status` is what was actually written, so ask it.
  if (next.status !== cur.status) {
    const why = patch.statusNote ?? (patch.status === undefined ? 'promoted by assignment' : null)
    await logActivity(id, who.id, 'status', `moved to ${next.status}${why ? ` (${why})` : ''}`)
    // Watchers + assigned humans hear about status moves (never the actor), and
    // `ticketAudience` re-reads board membership for both halves at this
    // moment — see its comment for why the write-time checks are not enough.
    void notifyTaskUsers(await ticketAudience(id, cur.boardId, assignees), who.id, {
      kind: 'task-status',
      title: `${cur.ticketRef ?? cur.title}: ${next.status.replace('_', ' ')}`,
      body: cur.title,
      href: `/boards/${cur.boardId}/${id}`,
    })
  }
  // ── ONE push-side call ─────────────────────────────────────────────────────
  // Two things mean "this is now someone's work": the ticket ENTERED a pickup
  // queue (a person moved the column, or promotion by assignment did), and/or it
  // GAINED agent assignees. Both used to call maybeDispatchTicket from their own
  // branch, so a promotion pushed twice and the status branch restated the
  // closed rule inline — the only place `doneKeys` stood in for the full closed
  // predicate, which an off-board-keyed pickup queue walked straight through.
  // Whether the ticket may be dispatched to AT ALL is maybeDispatchTicket's
  // call now; this decides only whether anything changed enough to ask.
  const addedAgents =
    patch.assignees !== undefined ? agentAssignees(assignees).filter((a) => !agentAssignees(cur.assignees).includes(a)) : []
  const enteredPickup =
    next.status !== cur.status && meta.agentStartKeys.includes(next.status) && !meta.agentStartKeys.includes(cur.status)
  if (enteredPickup || addedAgents.length) {
    void (async () => {
      const fresh = await getTask(id)
      // Entering the queue is approval for EVERY agent assignee; a new assignee
      // on a ticket already sitting there gets only their own push.
      if (fresh) await (await import('./work-dispatch')).maybeDispatchTicket(fresh, enteredPickup ? undefined : addedAgents)
    })().catch(() => {})
  }
  if (patch.assignees !== undefined && assignees.join(',') !== cur.assignees.join(',')) {
    await logActivity(id, who.id, 'assigned', assignees.length ? `assigned to ${assignees.join(', ')}` : 'unassigned')
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
  // Compare through `pgNum`: the patch carries a number off the API body while
  // the row carries a STRING (postgres.js hands back `numeric` unparsed), so a
  // raw `!==` was always true and every re-save wrote a spurious "estimate → 4h".
  if (patch.estimatedHours !== undefined && pgNum(patch.estimatedHours) !== pgNum(cur.estimatedHours))
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
/** THE comment write, and therefore the place the guard on agent-authored
 *  comments lives. `mcp comment` reaches here as a tool ARGUMENT — model output
 *  that never touched a harness and, until this call existed, never touched a
 *  guard either. `guardAgentWrite` decides whether this author is an agent at
 *  all (a person's comment is not model output and is left alone), records what
 *  the gate-safe rules find against that agent, and in strict mode hands back a
 *  redacted body. See ui/src/server/agent-writes.ts for why a credential is
 *  redacted rather than blocked.
 *
 *  Inside the write rather than at the route on purpose: `POST
 *  /api/tasks/:id/comments` is not the only caller — the workbench posts an
 *  agent's plan comment through here too — and a guard at one caller is a guard
 *  the next caller does not have. */
export async function addComment(taskId: string, author: string, content: string, parentId?: string): Promise<TaskComment> {
  const sql = await db()
  const body = (await guardAgentWrite('ticket-comment', author, content)).text
  const rows = await sql`
    insert into task_comments (task_id, author, content, parent_id)
    values (${taskId}, ${author}, ${body}, ${parentId ?? null})
    returning id, author, content, parent_id as "parentId", created_at as "createdAt"
  `
  await logActivity(taskId, author, 'comment', 'commented')
  const bid = await taskBoardId(taskId)
  if (bid) publishBoard(bid, { type: 'comment', taskId })
  return rows[0] as unknown as TaskComment
}

// ── Watchers ─────────────────────────────────────────────────────────────────
// A WATCHER IS AN AUDIENCE, and it was the one audience on a ticket that
// nothing in the product ever declared. `task_watchers.watcher` was free text:
// `POST /api/tasks/:id/watchers` took `z.string().min(1).max(200)` and inserted
// it, and the fan-out resolved anything containing '@' to a user id by email.
// Nobody asked whether that person could see the board. A user who was a member
// of NO board got `ALPH-1: in progress | <title> | /boards/<id>/<id>` in her
// inbox AND as SMTP bytes, followed by a 403 from the link she was sent, and
// could not unsubscribe herself because DELETE carried the same membership
// guard she was failing.
//
// The contrast is what makes it a defect rather than a design choice: a ticket
// ASSIGNEE is checked against board membership by `invalidAssignee` on both
// write routes. A watcher is the same fan-out with the check missing.
//
// So: ONE resolution of "who is watching this ticket", below, and every reader
// goes through it — `listWatchers` (the UI, the API response) and
// `ticketAudience` (the notifications). There is no path that reads
// `task_watchers` without asking it.
//
// WHAT HAPPENS TO A STORED WATCHER WHO NO LONGER QUALIFIES — the rows written
// before this check existed, and anyone whose board access is later revoked:
// the row is KEPT and the person is absent from both answers. Not shown in the
// ticket's watcher list, not notified, not mailed. The alternative shapes were
// both worse. Dropping them at send time while `listWatchers` still returned
// them is a lie in the UI — the panel names someone who is being told nothing —
// and that is the bug this replaces, one layer down. Deleting the row on read
// destroys a subscription on a read path, and makes a TEMPORARY removal from a
// board (or an accidental one) permanently unsubscribe someone with no record
// that it happened. Kept-but-dormant means restoring board access restores what
// they asked for, and `removeWatcher` is still the way to end it for good.
/** Stored watcher strings → the accounts they name. Email match, case- and
 *  whitespace-insensitive; a string that is not an email, or that names nobody
 *  with an account here, is simply absent from the map. */
async function watcherAccounts(watchers: string[]): Promise<Map<string, { id: string; email: string }>> {
  const emails = [...new Set(watchers.filter((w) => w.includes('@')).map((w) => w.trim().toLowerCase()))]
  const out = new Map<string, { id: string; email: string }>()
  if (emails.length === 0) return out
  const sql = await db()
  const rows = (await sql`select id, email from users where lower(email) = any(${emails})`) as unknown as Array<{
    id: string
    email: string
  }>
  const byEmail = new Map(rows.map((r) => [r.email.trim().toLowerCase(), { id: r.id, email: r.email }]))
  for (const w of watchers) {
    const account = byEmail.get(w.trim().toLowerCase())
    if (account) out.set(w, account)
  }
  return out
}

/** The watchers of a ticket who can still SEE the ticket, in stored order.
 *
 *  Membership is asked through `boardRole` one watcher at a time rather than
 *  joined in SQL on purpose: `boardRole` is the definition of "may this person
 *  see this board" and it already covers the two ways of holding one (a
 *  board_members row, or a team that owns the board). A join here would be a
 *  third hand-written copy of that rule, which is the shape this codebase keeps
 *  paying for. Tickets carry a handful of watchers; this is not a hot path. */
async function resolveWatchers(taskId: string, boardId: string | null): Promise<Array<{ watcher: string; userId: string }>> {
  const sql = await db()
  const rows = (await sql`
    select watcher from task_watchers where task_id = ${taskId} order by created_at asc
  `) as unknown as Array<{ watcher: string }>
  if (rows.length === 0) return []
  const board = boardId ?? (await taskBoardId(taskId))
  if (!board) return []
  const accounts = await watcherAccounts(rows.map((r) => r.watcher))
  const out: Array<{ watcher: string; userId: string }> = []
  for (const { watcher } of rows) {
    const account = accounts.get(watcher)
    if (!account) continue
    if (!(await boardRole(account.id, board))) continue
    out.push({ watcher, userId: account.id })
  }
  return out
}

/** The ticket's watchers as the product should show them: exactly the people
 *  who will be notified. */
export async function listWatchers(taskId: string): Promise<string[]> {
  return (await resolveWatchers(taskId, null)).map((w) => w.watcher)
}

/** Follow a ticket. Refuses anyone who cannot already see it — the same
 *  question `invalidAssignee` asks of an assignee, with the same answer shape,
 *  so the reason reaches the caller instead of a bare 403.
 *
 *  Stores the account's OWN email rather than the string that was typed, so
 *  case variants cannot become two rows for one person and the stored value is
 *  always something `watcherAccounts` can resolve. */
export async function addWatcher(taskId: string, watcher: string): Promise<{ ok: true } | { ok: false; error: string }> {
  const boardId = await taskBoardId(taskId)
  if (!boardId) return { ok: false, error: 'no such ticket' }
  const account = (await watcherAccounts([watcher])).get(watcher)
  if (!account) return { ok: false, error: 'a watcher is a person with an account here — pass the email address they sign in with' }
  if (!(await boardRole(account.id, boardId))) return { ok: false, error: 'watchers must be members of this board' }
  const sql = await db()
  await sql`insert into task_watchers (task_id, watcher) values (${taskId}, ${account.email}) on conflict do nothing`
  return { ok: true }
}

/** Unfollow. Case-insensitive on purpose: this is the escape hatch a person
 *  reaches for when mail is arriving about a board they cannot open, and it
 *  must not turn on whether their address was stored the way they typed it. */
export async function removeWatcher(taskId: string, watcher: string): Promise<void> {
  const sql = await db()
  await sql`delete from task_watchers where task_id = ${taskId} and lower(watcher) = ${watcher.trim().toLowerCase()}`
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

/** When does a status stamp `completed_at`?
 *
 *  This is the NARROW question — "is this a done-CATEGORY column on this board"
 *  — and deliberately NOT `meta.terminal()`, which also covers the off-board
 *  keys: a `cancelled` or `failed` ticket was never completed, so it must not
 *  carry a completion time. The two questions differ by exactly those keys and
 *  every place that has confused them so far was a bug, which is why
 *  `check-invariants.mjs` polices `doneKeys.includes(` and allows it here.
 *
 *  It lives in one function because it had just started living in two: the
 *  merge with #202 brought `completeQualityReview`, which spelled the identical
 *  ternary out a second time. That is the drift shape this codebase has spent a
 *  lot of effort removing, and the census caught it on the first CI run. */
function completedAtFor(meta: StatusMeta, status: string, previous: string | null): string | null {
  return meta.doneKeys.includes(status) ? (previous ?? new Date().toISOString()) : null
}

export async function completeQualityReview(
  taskId: string,
  reviewer: string,
  reviewStatus: 'approved' | 'rejected',
  nextStatus: TaskStatus,
): Promise<Task | null> {
  const sql = await db()
  const current = await getTask(taskId)
  if (!current) return null
  const meta = await statusMeta(current.boardId)
  if (!meta.reviewKeys.includes(current.status)) return null
  if (!meta.keys.includes(nextStatus)) throw new Error(`"${nextStatus}" is not a status on this board`)
  const completedAt = completedAtFor(meta, nextStatus, current.completedAt)
  const updated = await sql.begin(async (tx) => {
    const rows = await tx`
      update tasks set status = ${nextStatus}, completed_at = ${completedAt}, updated_at = now()
      where id = ${taskId} and status = ${current.status}
      returning id
    `
    if (!rows.length) return false
    await tx`
      insert into quality_reviews (task_id, reviewer, status)
      values (${taskId}, ${reviewer}, ${reviewStatus})
    `
    await tx`
      insert into task_activity (task_id, actor, type, description) values
        (${taskId}, ${reviewer}, 'review', ${reviewStatus === 'approved' ? 'approved this task' : 'requested changes'}),
        (${taskId}, ${reviewer}, 'status', ${`moved to ${nextStatus}`})
    `
    return true
  })
  if (!updated) return null

  if (meta.agentStartKeys.includes(nextStatus) && !meta.agentStartKeys.includes(current.status)) {
    void (async () => {
      const fresh = await getTask(taskId)
      if (fresh) await (await import('./work-dispatch')).maybeDispatchTicket(fresh)
    })().catch(() => {})
  }
  // Same audience question as the status move above, same one answer.
  void notifyTaskUsers(await ticketAudience(taskId, current.boardId, current.assignees), reviewer, {
    kind: 'task-status',
    title: `${current.ticketRef ?? current.title}: ${nextStatus.replace('_', ' ')}`,
    body: current.title,
    href: `/boards/${current.boardId}/${taskId}`,
  })
  publishBoard(current.boardId, { type: 'task', taskId })
  return getTask(taskId)
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
  // ── THE PULL SIDE ASKS THE SAME QUESTIONS THE WRITE SIDE ASKS ──────────────
  // Both of them, and neither in its own words. The history of this one query
  // is the whole lesson:
  //   · it selected on `bs.agent_start` alone, so it handed an agent work every
  //     write route would then refuse — an archived ticket, a ticket on an
  //     archived board, a ticket in a done-category or off-board-keyed pickup
  //     queue — and the agent looped on it every heartbeat;
  //   · then it asked `closedToAgents`, which said nothing about the board's
  //     AGENT POLICY, so a board owner revoking a grant through the ordinary
  //     settings path 403'd every write while the heartbeat kept serving;
  //   · and the SQL still carried a status predicate of its own — `agent_start`,
  //     plus a hardcoded legacy `('assigned','in_progress')` pair — which is a
  //     FIFTH spelling of "where may a ticket be worked?", and it DISAGREED with
  //     `workingKeys`: an agent that obeyed step 2 of its own dispatch prompt and
  //     moved the ticket into the board's active column fell off the heartbeat
  //     mid-session, because an active column need not carry `agent_start`.
  // So the SQL states NO rule. It narrows by ASSIGNMENT, and by a (board, key)
  // pair list built from `workingKeys` itself — the SQL is now a projection of
  // the one predicate rather than a rival to it. The legacy branch went with it:
  // `listStatuses` already answers for a board with no rows (it returns the
  // shipped defaults), so `statusMeta` is right about legacy boards too and the
  // literal pair had nothing left to do.
  const boards = (await sql`
    select distinct board_id as "boardId" from tasks where assignees @> ${sql.json([agentName])}::jsonb
  `) as unknown as Array<{ boardId: string }>
  // ONE pass over board facts for the whole heartbeat — columns, archival AND
  // agent policy, each resolved once per distinct board rather than once per
  // ticket. The hand-rolled `metaByBoard` map that used to live here memoised
  // only `statusMeta`, so `closedToAgents` re-read `boardInfo` for every
  // candidate: an N+1 on the hottest read in the product, and it existed
  // because the caller was memoising an ANSWER instead of holding a cache. A
  // board the agent is not allowed on is dropped HERE, before its tickets are
  // ever fetched.
  const facts = boardFacts()
  const pairBoards: string[] = []
  const pairKeys: string[] = []
  for (const { boardId } of boards) {
    if (!(await boardAllowsAgent(boardId, agentName, facts))) continue
    for (const key of (await facts.meta(boardId)).workingKeys) {
      pairBoards.push(boardId)
      pairKeys.push(key)
    }
  }
  if (!pairBoards.length) return []
  const rows = await sql`
    select t.id, t.title, t.description, t.tags, t.board_id as "boardId", t.status, t.archived_at as "archivedAt"
    from tasks t
    join unnest(${pairBoards}::text[], ${pairKeys}::text[]) as w(board_id, status)
      on w.board_id = t.board_id::text and w.status = t.status
    where t.assignees @> ${sql.json([agentName])}::jsonb
    order by t.created_at asc
  `
  const items = rows as unknown as Array<{
    id: string
    title: string
    description: string | null
    tags: string[]
    boardId: string
    status: string
    archivedAt: string | null
  }>
  // The pairs above already answered "is this column in play?"; this is the
  // TICKET half of the same authority question — its own archival, and the
  // board's, re-asked from the one predicate rather than restated in SQL.
  const servable: typeof items = []
  for (const t of items) {
    if (await agentTicketRefusal(t, agentName, 'write', facts)) continue
    servable.push(t)
  }
  // Matched workflows ride with the pull channel too (plugin-side dispatch).
  // ONE read of the workflow list for the whole heartbeat: calling
  // `workflowsForTask` per ticket re-read the table each time — the same N+1 as
  // `boardInfo`, hiding one line lower. Workflows are org-wide, not
  // board-scoped, so there is nothing to key a cache by; the list is read once
  // and handed to `workflowsFrom`, which is the SAME match the one-off path
  // uses. Spelling the filter out here instead would have made this the round's
  // own counter-example: a second expression of a rule, free to drift.
  const { listWorkflows, workflowsFrom } = await import('./workflows')
  const flows = servable.length ? await listWorkflows() : []
  return servable.map(({ status: _status, archivedAt: _archivedAt, ...t }) => ({
    ...t,
    workflows: workflowsFrom(flows, t),
  }))
}
