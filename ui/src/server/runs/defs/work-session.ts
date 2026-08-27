// AGENT WORK SESSIONS, as a durable run.
//
// WHAT MOVED HERE, AND WHY
//   `server/work-dispatch.ts` drove a session as a `for` loop inside a
//   fire-and-forget promise, guarded by a process-local `Set`:
//
//     const liveSessions = new Set<string>()
//     TODO(multi-instance): this is process-local; running several app
//     instances (or restarting mid-session) can double-run a session.
//
//   Both halves of that TODO were real. Two instances could each start a
//   session for one ticket+agent, and a restart mid-session dropped the loop on
//   the floor: the ticket kept its "work pushed to <agent>" line, the agent
//   kept whatever it had half-done, and NOTHING told the person who assigned it
//   that the session had stopped. The heartbeat eventually re-dispatched, from
//   turn one, with the turn cap reset.
//
//   The turn loop was already a step loop. So it is one: `liveSessions` is now
//   the run's Redis lease (one driver, fleet-wide), the loop index is the
//   CHECKPOINT, and a driver that dies is reclaimed by server/runs/reclaim.ts,
//   which re-enters this file at the turn the checkpoint says.
//
//   The push-side choke point (`maybeDispatchTicket`) and the enqueue stay in
//   work-dispatch.ts, where their shipped bugs are documented. Everything that
//   is the SESSION — the authority re-check, the turn cap, the reconcile nudge,
//   the activity trail — is here, with its comments, unchanged in behavior.
//
// THE STEP MACHINE, and why it has three stages rather than one turn per step.
//   A turn does TWO outward things: it calls a model (billed, and the agent's
//   tools may write to the ticket, push a branch, open a PR) and it writes a
//   line to `task_activity`. The runtime is AT-LEAST-ONCE — a step that ran and
//   did not checkpoint runs again — so those two live in different steps, each
//   checkpointing straight after its own effect:
//
//     (checkpoint null)  brief   → re-check authority, write the dispatch line
//     stage 'send'               → call the model for turn N, CHECKPOINT THE REPLY
//     stage 'record'             → write turn N's activity line, advance to N+1
//     stage 'failed'             → write the failure line, then fail the run
//
//   THE REPLY IS PERSISTED BEFORE IT IS ACTED ON, which is the whole reason
//   `record` is a separate step. Everything the session does with a reply — the
//   activity line, and the DONE/BLOCKED test that decides the next prompt — runs
//   off the checkpoint, so a crash anywhere after the model answered costs a
//   database write and not a second billed turn.
//
// THE ONE EFFECT THIS FILE CANNOT MAKE IDEMPOTENT is the model call itself.
// `runHarness` returns a value, not a provider request id, so there is no handle
// from the far side to carry into the checkpoint — and the agent on the other
// end of a work-session turn is not a completion, it is a tool loop that may
// have already commented on the ticket and pushed a branch. So a re-entry that
// re-sent the prompt would be TWO turns of one session against one ticket, quite
// possibly concurrently: the driver that died did not stop the agent, it only
// stopped listening to it.
//
// So this file does not re-send. `stageAttempt` records `ctx.attempt` at the
// moment a stage was written; a `send` step that finds `ctx.attempt` HIGHER than
// that knows a driver died while this turn was in flight, and it retires the
// turn instead of repeating it — one activity line saying the outcome is
// unknown, a settle interval for anything still running on the far side, and on
// to the next turn against the ticket's LIVE state. A lost turn is cheap. A
// duplicated one is a second agent working the same ticket.
import { createHash } from 'node:crypto'
import { defineRun, registerRun, type RunRow, type RunStepContext, type StepResult } from '../define'
import type { Authority } from '../../approvals'
import type { Finding } from '../../guardrails'
import type { WorkflowDelivery } from '../../workflows'
import type { Task } from '@/lib/task-const'
import { dispatchPrompt as dispatchBrief } from '../../harness/defs/work-session'
import { errLine, errText } from '../../errors'

const LOG = '[work-session]'


/** The registry key, and the `kind` column on every row this definition has
 *  ever produced. Stable forever — a rename orphans every session mid-flight. */
export const WORK_SESSION_KIND = 'work-session'

/** Turn budget: generous enough for real feature work, finite enough that a
 *  looping agent can't burn unbounded tokens. The session also ends the
 *  moment the ticket leaves the working statuses.
 *
 *  IT IS IN THE CHECKPOINT, so it SURVIVES A RESUME. That is the difference the
 *  port makes: before, a restart mid-session lost the count, the heartbeat
 *  re-dispatched, and the agent got a fresh twelve turns on work it had already
 *  spent twelve turns on. */
export const MAX_SESSION_TURNS = 12

/** How long a reclaimed session waits before sending its NEXT turn, when the
 *  turn it was reclaimed from may still be running.
 *
 *  The driver dying killed the HTTP request, not the agent: the persona has the
 *  prompt and is inside its own tool loop, and `workSessionHarness.holdMs` says
 *  we are willing to wait ten minutes for one of those. Sending the next turn
 *  immediately would put two prompts into one session. Two minutes is the
 *  compromise the other end of this path already uses (`proxyChat`'s default
 *  wait) — long enough that the common case has landed, short enough that a
 *  reclaimed session reads as a pause rather than a stall. */
export const INTERRUPTED_TURN_SETTLE_MS = 120_000

// ── The row's identity ───────────────────────────────────────────────────────

/** THE run id for one session, derived and not random.
 *
 *  `enqueue` deduplicates nothing above the row, so a caller that retries its
 *  request — or a second instance handling the same ticket mutation — creates a
 *  SECOND run doing the same work. That is precisely the bug `liveSessions` was
 *  written to prevent, and the runtime's answer to it is a deterministic id: the
 *  primary key refuses the duplicate, so a double dispatch produces one run and
 *  one loser that reads its own rejection as "somebody already has this".
 *
 *  `generation` is what keeps that claim from becoming a life sentence. The
 *  invariant is ONE LIVE SESSION per ticket+agent, not one ever: a ticket that
 *  legitimately comes back to the same agent next week needs a new row, and a
 *  row whose id is a pure function of (task, agent) has nowhere to put it. So
 *  `dispatchTicketWork` walks generations, skipping the finished ones and
 *  standing down on the first live one it finds. See its comment for the walk.
 *
 *  A NAME-BASED UUID rather than a hash string, because `runs.id` is `uuid`.
 *  Version 8 is RFC 9562's "custom" — an honest label for "the bytes are a
 *  SHA-256 of a name", which is what they are; v5 would claim SHA-1. */
export function sessionRunId(taskId: string, agentModel: string, generation: number): string {
  const digest = createHash('sha256').update(`${WORK_SESSION_KIND}\u0000${taskId}\u0000${agentModel}\u0000${generation}`).digest()
  const b = Buffer.from(digest.subarray(0, 16))
  b.writeUInt8((b.readUInt8(6) & 0x0f) | 0x80, 6) // version 8
  b.writeUInt8((b.readUInt8(8) & 0x3f) | 0x80, 8) // variant 10x
  const hex = b.toString('hex')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

// ── Input and checkpoint ─────────────────────────────────────────────────────

export interface WorkSessionInput {
  taskId: string
  agentModel: string
  /** The board, carried on the INPUT rather than looked up, because
   *  `audience()` is synchronous and gets nothing but the row — and the row's
   *  subject is the TICKET. A run that could not name its board could not name
   *  who may decide anything it ever asked. */
  boardId: string
  /** Only ever set by a caller that already had it; it goes into the dispatch
   *  prompt's header and nowhere else. */
  boardName?: string
  /** Which session this is for this ticket+agent. See `sessionRunId`. */
  generation: number
}

/** What one turn's reply is kept as.
 *
 *  BOUNDED SLICES, not the whole reply, and each one is a slice the session
 *  actually reads: `head` is what goes on the activity line (the widest of the
 *  three cuts the lines take), `tail` is the window `/\b(DONE|BLOCKED)\b/i` is
 *  tested against, `checks` is the guard finding ids. The checkpoint is read at
 *  every step boundary and published nowhere; keeping a whole model reply in it
 *  would be paying for the full transcript on every beat of the session. */
interface TurnReply {
  head: string
  tail: string
  checks: string[]
}

/** Which prompt produced a reply — and therefore how its activity line reads. */
type TurnKind = 'dispatch' | 'turn' | 'reconcile'

export type WorkSessionCheckpoint =
  /** A turn is owed. Nothing has been sent for `turn` yet — or, after a reclaim,
   *  something may have been and we will never know. */
  | { stage: 'send'; turn: number; stageAttempt: number; lastTail: string; notBefore?: number }
  /** A reply is persisted and has not reached the ticket yet. `reply: null` is
   *  the retired turn — the model call may or may not have happened. */
  | { stage: 'record'; turn: number; stageAttempt: number; said: TurnKind; reply: TurnReply | null }
  /** The session is over and the run must be filed as an error, but the ticket
   *  has not been told yet. */
  | { stage: 'failed'; turn: number; stageAttempt: number; error: string }

// ── What the session is allowed to see ───────────────────────────────────────

/** Where the ticket is now — the session's continue/stop signal.
 *
 *  THIS ASKS THE ONE PREDICATE. It used to compute its own:
 *  `working = agentStartKeys ∪ {category === 'active'}`, then
 *  `terminal = !assigned || !working.has(status)` — a fourth spelling of "may
 *  this agent still work this ticket?", in the one file the consolidation
 *  rounds treated as a caller rather than as a copy. It omitted BOTH archival
 *  clauses and the board's agent policy, so a person archiving the ticket,
 *  archiving its board, or revoking the agent's grant did not stop the live
 *  session already running: it kept driving turns against work that had been
 *  withdrawn, and every tool call it made came back 403.
 *  Now: `agentTicketRefusal` answers authority, `workingKeys` answers "still in
 *  play", assignment is the one thing left that is genuinely local to a
 *  session — and `stop` carries the reason so the activity line says WHY the
 *  session ended instead of just naming a column.
 *
 *  IT IS ASKED ON EVERY TURN, and the port makes that MORE load-bearing rather
 *  than less: a resumed session has been away for as long as a deploy plus a
 *  reclaim, so the world it left is not the world it wakes up in. It also hands
 *  back the TICKET it asked about, so the prompt for that turn is built from the
 *  same read the authority came from and cannot describe a ticket that has since
 *  moved. */
export interface SessionState {
  task: Task
  stop: string | null
}

export interface WorkSessionDeps {
  /** Authority + "still in play" + the live ticket, in one read. */
  sessionState: (taskId: string, agentModel: string) => Promise<SessionState | null>
  /** The two destinations the dispatch prompt may name. From `statusMeta`, so
   *  they come from `placeable` like every other destination in the product. */
  boardHint: (boardId: string) => Promise<{ activeKey: string | null; assignedKey: string | null }>
  workflowsForTask: (task: Task) => Promise<WorkflowDelivery[]>
  /** Skill names this agent can actually load: the shared root + its own. */
  skillNames: (agentModel: string) => Promise<Set<string>>
  /** ONE TURN = ONE HARNESS RUN. Throws when the turn produced nothing usable. */
  turn: (args: { agentModel: string; taskId: string; prompt: string; signal: AbortSignal }) => Promise<{ text: string; findings: Finding[] }>
  logActivity: (taskId: string, actor: string, type: string, description: string) => Promise<void>
  /** The ticket's recent activity, read ONLY to keep a line from landing twice
   *  after a driver died between writing it and checkpointing that it had. */
  recentActivity: (taskId: string) => Promise<Array<{ actor: string; type: string; description: string }>>
  now: () => number
}

/** The real edges, each behind a lazy import — the same idiom work-dispatch.ts
 *  has always used, and the reason this module can be imported at boot (to
 *  register the kind) without dragging the tasks/boards/harness graph in with
 *  it. Every one is overridable per call, so work-session.test.ts drives whole
 *  sessions with no database, no Redis and no model. */
const REAL_DEPS: WorkSessionDeps = {
  sessionState: async (taskId, agentModel) => {
    const { getTask, agentTicketRefusal } = await import('../../tasks')
    const { boardFacts } = await import('../../boards')
    const t = await getTask(taskId)
    if (!t) return null
    const facts = boardFacts()
    const refusal = await agentTicketRefusal(t, agentModel, 'write', facts)
    if (refusal) return { task: t, stop: refusal }
    if (!t.assignees.includes(agentModel)) return { task: t, stop: 'no longer assigned to this agent' }
    const meta = await facts.meta(t.boardId)
    if (!meta.workingKeys.includes(t.status)) return { task: t, stop: `ticket moved to "${t.status}"` }
    return { task: t, stop: null }
  },
  boardHint: async (boardId) => {
    const { statusMeta } = await import('../../statuses')
    const meta = await statusMeta(boardId)
    return { activeKey: meta.activeKey, assignedKey: meta.assignedKey }
  },
  workflowsForTask: async (task) => (await import('../../workflows')).workflowsForTask(task),
  skillNames: async (agentModel) => {
    const { listAllSkills, SHARED } = await import('../../agent-skills')
    const { db } = await import('../../db/pg')
    const sql = await db()
    const [def] = (await sql`select slug from agent_defs where model = ${agentModel}`) as unknown as Array<{ slug: string }>
    const names = new Set<string>()
    for (const o of await listAllSkills()) {
      if (o.owner === SHARED || o.owner === def?.slug) for (const sk of o.skills) names.add(sk.name)
    }
    return names
  },
  turn: async ({ agentModel, taskId, prompt, signal }) => {
    const { runHarness } = await import('../../harness/run')
    const { workSessionHarness } = await import('../../harness/defs/work-session')
    // THE LEDGER LINE IS THE ONE THING ONLY THIS CALL CAN SAY. `recordUsage`
    // writes `task_id`, and `taskUsage` sums a ticket's cost by that column
    // alone — so without `taskId` here a session's spend lands in the ledger and
    // never in the number the ticket's owner reads. `source` stays 'chat'
    // deliberately: 'ticket' rows are agent-SELF-REPORTED through MCP
    // `log_usage`, and this turn is metered by Talaria on its own request path.
    //
    // `signal` is the run's — the driver aborts it when the step blows its
    // budget or the lease moves to another instance, and it reaches the
    // transport's own fetch. It is the only thing that can actually stop a turn
    // in flight; without it a reclaimed session would leave a live model call
    // behind on the instance that lost the lease.
    const run = await runHarness(
      workSessionHarness,
      { prompt },
      { caller: `ticket:${taskId}`, model: agentModel, ledger: { source: 'chat', refId: taskId, taskId }, signal },
    )
    if (run.value === null) throw new Error(run.error ?? `no reply from ${agentModel}`)
    return { text: run.value, findings: run.findings }
  },
  logActivity: async (taskId, actor, type, description) => (await import('../../tasks')).logActivity(taskId, actor, type, description),
  recentActivity: async (taskId) => (await import('../../tasks')).listActivity(taskId),
  now: () => Date.now(),
}

// ── The activity trail ───────────────────────────────────────────────────────

/** Guard findings, on the line a HUMAN reads.
 *
 *  `runHarness` already writes them to `guard_findings`, which is where the
 *  fitness page reads a per-model confabulation rate — but nobody reviewing
 *  PLAT-118 opens that table. A turn flagged `zero_tool_claim` on a ticket is
 *  the exact thing the reviewer signing that ticket off needs to see, so the
 *  check ids ride on the activity line, in FRONT of the reply so a bounded
 *  slice can never truncate them away.
 *
 *  IDS ONLY — never `message` and above all never `snippet`, which is a verbatim
 *  excerpt of the flagged text. And this never travels back to the agent: the
 *  next turn's prompt is built from the ticket's state, not from this line.
 *  Feeding a finding back would break guardrails.ts's cardinal invariant and
 *  teach the agent to argue with the guard instead of to call a tool. */
const noted = (line: string, checks: string[]): string => (checks.length ? `[guard: ${[...new Set(checks)].join(', ')}] ${line}` : line)

/** Write one line of the session's trail.
 *
 *  A LOST LINE MUST NOT DESTROY THE SESSION. The trail is the RECORD of the
 *  work, not the work: a database blip while writing "session turn 4" is not a
 *  reason to abandon a ticket an agent is halfway through, and under the
 *  runtime a throw here would do exactly that — the step fails, the run is filed
 *  as an error, and the eleven remaining turns never happen. So a failed write
 *  is loud in the log and invisible to the step. (Pre-port, the first two lines
 *  were unguarded and the rest carried `.catch(() => {})`; this makes all of
 *  them the same, in the direction the guarded ones already chose.)
 *
 *  `dedupe` is the at-least-once guard, and it is asked for ONLY on the path
 *  where it can be true: a driver died between this write landing and the
 *  checkpoint that records it, so the step is re-entered and would say the same
 *  thing twice. Every line this file writes names its turn number or its
 *  reason, so an exact (actor, type, description) match on the ticket's recent
 *  history is that line and not a coincidence. */
async function say(
  d: WorkSessionDeps,
  taskId: string,
  actor: string,
  description: string,
  opts: { dedupe: boolean },
): Promise<void> {
  try {
    if (opts.dedupe) {
      const recent = await d.recentActivity(taskId)
      if (recent.some((a) => a.actor === actor && a.type === 'dispatch' && a.description === description)) {
        console.warn(`${LOG} ${taskId}: "${description.slice(0, 80)}" is already on the ticket — a driver died after writing it, not before`)
        return
      }
    }
    await d.logActivity(taskId, actor, 'dispatch', description)
  } catch (e) {
    console.error(`${LOG} ${taskId}: could not record "${description.slice(0, 80)}" on the ticket:`, errText(e))
  }
}

// ── The prompts ──────────────────────────────────────────────────────────────

/** The workflow block, and the gap signal that rides with it.
 *
 *  Workflows name SKILLS — Hermes loads the flow content from the skill mounts
 *  it already reads; we never paste flow prose into the prompt. Skills the
 *  target agent can't see are flagged, not silently named (the future gap loop
 *  starts from exactly this signal). */
async function workflowContext(d: WorkSessionDeps, task: Task, agentModel: string): Promise<{ block: string; line: string }> {
  const workflows = await d.workflowsForTask(task)
  const available = workflows.some((w) => w.skills.length) ? await d.skillNames(agentModel) : new Set<string>()
  const missing = workflows.flatMap((w) => w.skills.filter((sk) => !available.has(sk)))
  const block = workflows.length
    ? `\n\nHOW THIS KIND OF WORK IS DONE HERE (workflow${workflows.length > 1 ? 's' : ''}):\n` +
      workflows
        .map((w) => {
          const skills = w.skills.filter((sk) => available.has(sk))
          const flow = skills.length
            ? `Load and follow your skill${skills.length > 1 ? 's' : ''}: ${skills.map((sk) => `"${sk}"`).join(', ')}.`
            : 'Use your judgment — no specific skill is bound to this workflow.'
          const kits = w.toolkits.length
            ? `\nExpected toolkits: ${w.toolkits.map((t) => t.server + (t.tools?.length ? ` (${t.tools.join(', ')})` : '')).join('; ')}`
            : ''
          return `── ${w.name} ──\n${flow}${kits}`
        })
        .join('\n\n')
    : ''
  const line =
    `work pushed to ${agentModel}` +
    (workflows.length ? ` with workflow ${workflows.map((w) => w.name + (w.skills.length ? ` [${w.skills.join(', ')}]` : '')).join(', ')}` : '') +
    (missing.length ? ` — skill ${missing.map((m) => `"${m}"`).join(', ')} not available to this agent` : '')
  return { block, line }
}

/** The dispatch brief — turn one, and the only turn whose prompt describes the
 *  ticket rather than pointing at it. The TEMPLATE is the harness's
 *  `dispatchPrompt` (the two were verbatim twins until the durability port
 *  forked them — a fork is how the fence and the wording drift); this side
 *  supplies what only the durable run knows: the board hint, the workflow
 *  block, and step 2's status instruction. */
async function dispatchPrompt(d: WorkSessionDeps, task: Task, agentModel: string, boardName: string | undefined): Promise<string> {
  const hint = await d.boardHint(task.boardId)
  const { block } = await workflowContext(d, task, agentModel)
  // THE PROMPT HANDS THIS TO THE AGENT VERBATIM, so it is a destination and
  // must come from `statusMeta`'s `placeable` list like every other one. It
  // used to be `listStatuses(...).find(s => s.category === 'active')?.key`,
  // which does not exclude terminal columns: on a board whose first active
  // column is labelled "Cancelled" (slug `cancelled`, an off-board terminal
  // key — legal, and `agentStartConflict` does not refuse it) the hint was
  // "cancelled", so an agent obeying step 2 of its own work-session prompt
  // sent a TERMINAL move. `activeKey` is picked from `placeable`, and with no
  // active column at all we fall back to the pickup queue; with neither we say
  // so rather than invent `in_progress`, a key the board may not have.
  const activeHint = hint.activeKey ?? hint.assignedKey
  const step2 = activeHint
    ? `comment a one-line acknowledgment, and triage_ticket to status "${activeHint}" while you work.`
    : `comment a one-line acknowledgment. Leave the status where it is — this board has no working column for you to move it to.`
  return dispatchBrief({
    taskId: task.id,
    ticketRef: task.ticketRef ?? task.id,
    title: task.title,
    description: task.description,
    boardName,
    workflowBlock: block,
    step2,
  })
}

/** The agent says it's finished but the ticket disagrees — one nudge to
 *  reconcile (report_outcome / set blocked), then stop pushing. */
const reconcilePrompt = (status: string): string =>
  `[Work session — reconcile] You said DONE/BLOCKED but the ticket is still "${status}". If finished: report_outcome now. If blocked: set status "blocked" with a comment. If neither, keep working.`

/** Every continuation carries the LIVE ticket state, so the agent never works a
 *  stale picture — which is also what makes a resumed session safe to continue:
 *  the prompt is built from the ticket as it is now, not as it was before the
 *  deploy. */
const continuePrompt = (turn: number, status: string): string =>
  `[Work session — turn ${turn}/${MAX_SESSION_TURNS}] You're mid-work on this ticket (status: "${status}"). Continue like a developer: next step, run it, read the result, adjust. Verify before you finish — tests, your own diff, and for UI work drive it in a real browser (Playwright) and attach evidence. When genuinely done: report_outcome. If stuck: status "blocked" + comment. End with your status line.`

// ── The step ─────────────────────────────────────────────────────────────────

const asInput = (raw: unknown): WorkSessionInput => {
  const v = raw as Partial<WorkSessionInput> | null
  if (!v || typeof v.taskId !== 'string' || typeof v.agentModel !== 'string' || typeof v.boardId !== 'string')
    // A row whose input this deploy cannot read is a bug report, not something
    // to guess at: guessing would dispatch a turn at an agent nobody named.
    throw new Error('work-session run has no readable input (taskId / agentModel / boardId)')
  return { taskId: v.taskId, agentModel: v.agentModel, boardId: v.boardId, boardName: v.boardName, generation: v.generation ?? 0 }
}

const asCheckpoint = (raw: unknown): WorkSessionCheckpoint | null => {
  if (raw === null || raw === undefined) return null
  const v = raw as WorkSessionCheckpoint
  if (v.stage !== 'send' && v.stage !== 'record' && v.stage !== 'failed')
    throw new Error(`work-session checkpoint has an unknown stage "${String((v as { stage?: unknown }).stage)}"`)
  return v
}

/** `turn - 1` turns are behind us. The plural is spelled out rather than
 *  `turn > 2` because this line is now also reachable at turn 1, where the old
 *  loop could not go: a run parked, reclaimed or simply slow enough that the
 *  world changed between the enqueue and the first prompt. */
const turnsSoFar = (turn: number): string => `${turn - 1} turn${turn - 1 === 1 ? '' : 's'}`

/** ONE UNIT OF PROGRESS. Exported with its deps so a test can drive a whole
 *  session — including a reclaim — with no database, no Redis and no model. */
export async function workSessionStep(
  ctx: RunStepContext<WorkSessionInput, WorkSessionCheckpoint>,
  overrides: Partial<WorkSessionDeps> = {},
): Promise<StepResult<WorkSessionCheckpoint>> {
  const d: WorkSessionDeps = { ...REAL_DEPS, ...overrides }
  // OUTSIDE the try, deliberately. Every other failure below is told to the
  // ticket before the run is filed as an error; a row whose input or checkpoint
  // this deploy cannot read is the one failure with nowhere to say it — we do
  // not know which ticket it is about. It goes straight out to the driver, which
  // writes the message onto the run.
  const input = asInput(ctx.input)
  const cp = asCheckpoint(ctx.checkpoint)
  const { taskId, agentModel } = input

  try {
    // ── The failure tail ─────────────────────────────────────────────────────
    // Last, so the ticket learns the session failed even though the run is about
    // to be filed as an error. Its own line is deduped for the same reason every
    // other one is: the throw below is what makes the run terminal, and a driver
    // that died between the line and the throw re-enters here.
    if (cp?.stage === 'failed') {
      await say(d, taskId, 'talaria', `work session with ${agentModel} failed: ${cp.error.slice(0, 200)}`, {
        dedupe: ctx.attempt > cp.stageAttempt,
      })
      throw new Error(cp.error)
    }

    // ── Record: the reply is already durable, put it on the ticket ────────────
    if (cp?.stage === 'record') {
      const dedupe = ctx.attempt > cp.stageAttempt
      if (cp.reply === null) {
        // THE RETIRED TURN. The `send` step found that a driver had died while
        // this turn was in flight, so we do not know whether the agent was asked
        // — and did not ask again. Saying so on the ticket is the whole point:
        // the pre-port failure mode was a session that simply stopped, with the
        // ticket still reading "work pushed to <agent>" and nothing else.
        await say(d, taskId, 'talaria', `turn ${cp.turn} was interrupted by a restart — its outcome is unknown; continuing from the ticket's current state`, { dedupe })
      } else {
        const line =
          cp.said === 'dispatch'
            ? `picked up: ${cp.reply.head.slice(0, 300) || '(no reply)'}`
            : cp.said === 'reconcile'
              ? `session reconcile: ${cp.reply.head.slice(0, 200) || '(no reply)'}`
              : `session turn ${cp.turn}: ${cp.reply.head.slice(0, 250) || '(no reply)'}`
        await say(d, taskId, agentModel, noted(line, cp.reply.checks), { dedupe })
      }
      return {
        kind: 'next',
        checkpoint: {
          stage: 'send',
          turn: cp.turn + 1,
          stageAttempt: ctx.attempt,
          lastTail: cp.reply?.tail ?? '',
          ...(cp.reply === null ? { notBefore: d.now() + INTERRUPTED_TURN_SETTLE_MS } : {}),
        },
        phase: `turn ${cp.turn} recorded`,
      }
    }

    // ── The brief: nothing has been said on this ticket yet ───────────────────
    if (cp === null) {
      const state = await d.sessionState(taskId, agentModel)
      // REFUSED BEFORE A WORD IS WRITTEN, and silently, exactly as
      // `maybeDispatchTicket` refuses: nothing was pushed, so there is nothing
      // to explain on the ticket. The gap this closes is new to the port — a run
      // waits in the queue, and the ticket can be archived, closed or taken away
      // from the agent between the enqueue and the first step.
      if (!state || state.stop)
        return { kind: 'done', result: { turns: 0, ended: state?.stop ?? 'ticket gone', dispatched: false } }
      const { line } = await workflowContext(d, state.task, agentModel)
      // The lifecycle is auditable from the ticket: every step lands in
      // task_activity (dispatch start w/ matched workflows + skills, the
      // agent's reply or the failure, then the agent's own actions).
      await say(d, taskId, 'talaria', line, { dedupe: ctx.attempt > 0 })
      return { kind: 'next', checkpoint: { stage: 'send', turn: 1, stageAttempt: ctx.attempt, lastTail: '' }, phase: `pushed to ${agentModel}` }
    }

    // ── Send: the one step that spends money ─────────────────────────────────
    const { turn } = cp

    // The cap is checked BEFORE authority, because that is the order the loop
    // had: turn 12 was the last one sent, and the line after the loop was
    // written without asking the ticket anything.
    if (turn > MAX_SESSION_TURNS) {
      await say(d, taskId, 'talaria', `work session hit the ${MAX_SESSION_TURNS}-turn cap — leaving the ticket to the agent/heartbeat`, {
        dedupe: ctx.attempt > cp.stageAttempt,
      })
      return { kind: 'done', result: { turns: MAX_SESSION_TURNS, ended: 'turn cap' } }
    }

    // A SOFT pause, not a wait inside the step: nobody is notified, no attempt
    // is consumed, and the run's own lease holds the gap. See
    // INTERRUPTED_TURN_SETTLE_MS. The condition clears itself — once the clock
    // passes `notBefore` this branch is simply false — so the deferral cannot
    // become a loop.
    if (cp.notBefore !== undefined && d.now() < cp.notBefore)
      return { kind: 'retry', after: cp.notBefore - d.now(), reason: `letting the interrupted turn settle before turn ${turn}` }

    // AUTHORITY, ON EVERY TURN, BEFORE ANYTHING ELSE. A person archiving the
    // ticket, archiving its board, or revoking the agent's grant stops the
    // session mid-flight — and for a resumed run this is the FIRST thing that
    // happens, because the world may have changed entirely while it was parked.
    const state = await d.sessionState(taskId, agentModel)
    if (!state || state.stop) {
      await say(d, taskId, 'talaria', `work session ended after ${turnsSoFar(turn)} — ${state ? state.stop : 'ticket gone'}`, {
        dedupe: ctx.attempt > cp.stageAttempt,
      })
      return { kind: 'done', result: { turns: turn - 1, ended: state?.stop ?? 'ticket gone' } }
    }

    // ── The interrupted turn ─────────────────────────────────────────────────
    // `stageAttempt` was `ctx.attempt` when this stage was written. A higher one
    // now means a driver was reclaimed while this exact turn was owed — so the
    // prompt may already be with the agent, and re-sending it would put two
    // turns of one session against one ticket. Retire it instead: the ticket
    // gets a line saying so, the next turn waits for anything in flight to
    // settle, and the turn cap counts it, because it cost a turn either way.
    if (ctx.attempt > cp.stageAttempt) {
      console.warn(
        `${LOG} ${taskId} (${agentModel}): turn ${turn} was owed when a driver died at attempt ${cp.stageAttempt} — not re-sending it. ` +
          `The model may have been called and the agent may have used its tools; a second prompt would be a second agent on this ticket.`,
      )
      return {
        kind: 'next',
        checkpoint: { stage: 'record', turn, stageAttempt: ctx.attempt, said: turn === 1 ? 'dispatch' : 'turn', reply: null },
        phase: `turn ${turn} interrupted`,
      }
    }

    const kind: TurnKind = turn === 1 ? 'dispatch' : /\b(DONE|BLOCKED)\b/i.test(cp.lastTail) ? 'reconcile' : 'turn'
    const prompt =
      kind === 'dispatch'
        ? await dispatchPrompt(d, state.task, agentModel, input.boardName)
        : kind === 'reconcile'
          ? reconcilePrompt(state.task.status)
          : continuePrompt(turn, state.task.status)

    // The last gate before the money: the driver aborts this signal when the
    // step blows its budget or the lease moves, and a step that called anyway
    // would be the doubled side effect the runtime exists to prevent.
    if (ctx.signal.aborted) throw new Error('the run was interrupted before turn ' + turn)
    ctx.log(`turn ${turn} of ${MAX_SESSION_TURNS} — waiting on ${agentModel}`)

    let reply: { text: string; findings: Finding[] }
    try {
      reply = await d.turn({ agentModel, taskId, prompt, signal: ctx.signal })
    } catch (e) {
      if (ctx.signal.aborted) throw e
      // A turn that produced nothing usable ends the session with a logged
      // failure instead of driving eleven more turns off a blank — the policy
      // lives in `workSessionHarness.onFailure: 'throw'` and this is where it
      // lands. It goes through a checkpoint rather than throwing here so the
      // ticket's failure line is written by a step with nothing else in it: a
      // throw from HERE would re-enter this step on the next driver and re-send
      // the prompt.
      return { kind: 'next', checkpoint: { stage: 'failed', turn, stageAttempt: ctx.attempt, error: errLine(e) }, phase: `turn ${turn} failed` }
    }

    // NOTHING BETWEEN THE MODEL AND THE CHECKPOINT. Every use of this reply —
    // the activity line, the DONE/BLOCKED test — happens in the next step, off
    // the persisted copy. The window in which a crash costs a second billed turn
    // is this return and one database write, and it cannot be made smaller from
    // here.
    return {
      kind: 'next',
      checkpoint: {
        stage: 'record',
        turn,
        stageAttempt: ctx.attempt,
        said: kind,
        reply: {
          head: reply.text.slice(0, 300),
          tail: reply.text.slice(-200),
          checks: [...new Set(reply.findings.map((f) => f.check))],
        },
      },
      phase: `turn ${turn} answered`,
    }
  } catch (e) {
    // An abort is the DRIVER's outcome, not the session's: it has already
    // rejected its own race and decided whether this was a lost lease or a blown
    // budget. Re-filing it as a failed session would put a "work session failed"
    // line on a ticket whose session is alive on another instance.
    if (ctx.signal.aborted) throw e
    // Anything else — a board read that threw, a workflow lookup that died — is
    // the session failing, and the ticket is owed the sentence. One more step,
    // so the line is written by something that cannot repeat a model call.
    if (cp?.stage === 'failed') throw e
    console.error(`${LOG} ${taskId} (${agentModel}) step threw:`, errText(e))
    return {
      kind: 'next',
      checkpoint: { stage: 'failed', turn: cp?.turn ?? 1, stageAttempt: ctx.attempt, error: errLine(e) },
      phase: 'session failed',
    }
  }
}

// ── The definition ───────────────────────────────────────────────────────────

export const workSessionRun = registerRun(
  defineRun<WorkSessionInput, WorkSessionCheckpoint>({
    kind: WORK_SESSION_KIND,
    label: 'Agent work session',
    step: (ctx) => workSessionStep(ctx),

    /** THE BOARD'S EDITORS, which is who the ticket routes every other decision
     *  to. Never the run's owner (there isn't one) and never the agent: a
     *  session that could name its own audience would be an agent deciding who
     *  supervises it. `nobody` when the input cannot name a board, which is a
     *  real stall to report rather than a reason to widen. */
    audience: (run: RunRow): Authority => {
      const boardId = (run.input as Partial<WorkSessionInput> | null)?.boardId
      return typeof boardId === 'string' && boardId ? { by: 'board', boardId } : { by: 'nobody' }
    },

    /** ELEVEN MINUTES, because one step is one turn and one turn is one call to
     *  `workSessionHarness`, whose `holdMs` is ten: an agent restarting under a
     *  config propagation refuses connections for tens of seconds and a fleet
     *  re-render mid-session must not kill the session. This is also the lease
     *  TTL, so a session whose instance died is reclaimable about eleven minutes
     *  later — which is the right answer rather than an unfortunate one. Coming
     *  back sooner would mean reclaiming turns that are genuinely still running. */
    maxStepMs: 660_000,

    /** FIVE, against the default three, and the reason is duration. `attempt`
     *  counts drivers that DIED holding this run, and a work session is the
     *  longest-lived run in the product — up to twelve model turns of up to ten
     *  minutes each — so it is the one kind that routinely spans more than one
     *  deploy. Three would file a healthy session as an error for the crime of a
     *  busy release day. The count is still bounded, and it is self-limiting in
     *  a way no other kind's is: every reclaim retires the turn it interrupted,
     *  so a session that keeps killing drivers spends its turn budget doing it. */
    maxAttempts: 5,
  }),
)
