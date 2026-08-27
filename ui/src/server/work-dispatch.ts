// Work dispatch — THE PUSH SIDE. When a ticket enters an agent-start column
// with agent assignees, Talaria pushes the work into the agent's own persona
// gateway as a WORK SESSION: turn after turn, the agent drives its tools/harness
// like a developer at a desk (run, read, steer, test, repeat), until the ticket
// reaches review/blocked/done, the agent stops progressing, or the turn cap
// lands. One turn is never the budget for real work. Matched task WORKFLOWS ride
// along; the plugin heartbeat remains the pull-side safety net.
//
// THE SESSION ITSELF IS A DURABLE RUN (runs/defs/work-session.ts). What used to
// be a `for` loop inside a fire-and-forget promise, guarded by a process-local
// `Set`, is a run: the loop index is a checkpoint, the guard is a Redis lease,
// and a driver that dies is reclaimed and re-enters at the turn it left. The
// TODO that stood on line 51 of this file for the whole of its life —
//
//   const liveSessions = new Set<string>()
//   TODO(multi-instance): this is process-local; running several app instances
//   (or restarting mid-session) can double-run a session.
//
// — is gone in both of its halves. Two instances cannot start one session
// because the run id is DERIVED (see `sessionRunId`), and a restart no longer
// drops the work: the session resumes from its last persisted turn instead of
// vanishing with the ticket still reading "work pushed to <agent>".
//
// THE TURN ITSELF IS A HARNESS (harness/defs/work-session.ts) and nothing in
// this file reaches a model any more. What is left here is what always belonged
// here: the one push-side choke point, and the claim that turns a dispatch into
// exactly one session.
import { enqueue, pgRunStore } from './runs/run'
import { isTerminal, type RunRow } from './runs/define'
import { sessionRunId, workSessionRun, type WorkSessionInput } from './runs/defs/work-session'
import { errText } from './errors'
import type { Task } from '@/lib/task-const'

const LOG = '[work-dispatch]'

/** How many sessions one ticket+agent pair may ever have.
 *
 *  Not a rate limit — it is the bound on the generation walk below, and it is a
 *  SIGNAL as much as a guard: a ticket that has been handed to the same agent
 *  twenty-five times is a ticket bouncing between a column and an agent that
 *  cannot finish it, and the honest response is to say so loudly and stop rather
 *  than to quietly start a twenty-sixth. */
const MAX_SESSIONS_PER_TICKET_AGENT = 25

/** A primary-key collision, and the only error `enqueue` can raise that means
 *  something rather than being broken: somebody else claimed this exact session
 *  in the time between our read and our insert. postgres.js surfaces the SQLSTATE
 *  on the error; 23505 is `unique_violation`. */
const isDuplicateKey = (e: unknown): boolean => typeof e === 'object' && e !== null && (e as { code?: unknown }).code === '23505'

export interface DispatchDeps {
  getRun: (id: string) => Promise<RunRow | null>
  startRun: (id: string, input: WorkSessionInput) => Promise<void>
}

const REAL_DEPS: DispatchDeps = {
  getRun: (id) => pgRunStore.get(id),
  startRun: async (id, input) => {
    await enqueue(
      workSessionRun,
      input,
      {
        id,
        // NULL OWNER, deliberately. A work session belongs to a TICKET, not to
        // the person whose click happened to move the column — `maybeDispatchTicket`
        // is not told who acted, and inventing an owner from the ticket's
        // assignees would put somebody else's agent session in a person's "my
        // runs" strip. The subject is the ticket and the audience is its board.
        ownerUserId: null,
        subjectType: 'task',
        subjectId: input.taskId,
        phase: `queued for ${input.agentModel}`,
      },
    )
  },
}

/** Drive one ticket with one agent as a SESSION. Fire-and-forget from task
 *  mutations; re-entry for the same ticket+agent is a no-op.
 *
 *  IT NO LONGER WAITS FOR THE WORK. The session is a row before this function
 *  returns and a driver picks it up detached, which is the same contract the old
 *  `void dispatchTicketWork(...)` call sites already assumed — except that now
 *  the session survives them.
 *
 *  THE CLAIM, which is what `liveSessions` was:
 *    `sessionRunId(task, agent, n)` is a DERIVED uuid, so two dispatchers racing
 *    the same ticket compute the same id and the primary key refuses the second
 *    — across instances, which a `Set` never could. The walk over `n` is what
 *    keeps that claim from being permanent: generation 0 is this pair's first
 *    session ever, and once it is FINISHED a ticket that legitimately comes back
 *    to the same agent needs somewhere to put the next one. So:
 *      · a live (or parked) run at generation n → stand down, that is the session
 *      · a finished run at generation n → that session is over, look at n + 1
 *      · nothing at generation n → claim it
 *    A duplicate-key error on the claim is the race resolving itself: somebody
 *    inserted between our read and our write, and what they inserted is a live
 *    session for this exact ticket and agent, so standing down is correct. */
export async function dispatchTicketWork(task: Task, agentModel: string, boardName?: string, deps: Partial<DispatchDeps> = {}): Promise<void> {
  const d: DispatchDeps = { ...REAL_DEPS, ...deps }
  const input: WorkSessionInput = {
    taskId: task.id,
    agentModel,
    boardId: task.boardId,
    ...(boardName === undefined ? {} : { boardName }),
    generation: 0,
  }
  for (let generation = 0; generation < MAX_SESSIONS_PER_TICKET_AGENT; generation++) {
    const id = sessionRunId(task.id, agentModel, generation)
    let existing: RunRow | null
    try {
      existing = await d.getRun(id)
    } catch (e) {
      // The claim is the only thing standing between a retried request and two
      // agents on one ticket, so a store we cannot read is a dispatch we do not
      // make. The heartbeat is the pull-side safety net for exactly this.
      console.error(`${LOG} ${task.id}: could not check for a live session with ${agentModel}, not dispatching:`, errText(e))
      return
    }
    if (existing && !isTerminal(existing.state)) return
    if (existing) continue
    try {
      await d.startRun(id, { ...input, generation })
      return
    } catch (e) {
      if (isDuplicateKey(e)) return
      console.error(`${LOG} ${task.id}: could not start a work session with ${agentModel}:`, errText(e))
      return
    }
  }
  console.error(
    `${LOG} ${task.id} has had ${MAX_SESSIONS_PER_TICKET_AGENT} work sessions with ${agentModel} and every one of them finished — ` +
      `not starting another. This ticket keeps coming back to an agent that cannot close it; a person should look at it.`,
  )
}

/** Dispatch to every AGENT assignee when the ticket sits in an agent-start
 *  column. `onlyAgents` narrows to newly-added assignees on updates.
 *
 *  THE PUSH-SIDE CHOKE POINT. Every caller — createTask, both of updateTask's
 *  branches, and anything added later — arrives here, and this used to check
 *  ONE thing: is the ticket's column flagged agent-start? Nothing about closed,
 *  archived, or an archived board. `updateTask` restated a fragment of the rule
 *  inline in its status branch (`!meta.doneKeys.includes(...)`) and its
 *  assignees branch inherited nothing at all, so assigning an agent to a CLOSED
 *  ticket started a live work session on it. And `doneKeys` was never the whole
 *  rule anyway: a board owner may legally create an `active + agentStart`
 *  column labelled "Cancelled" (agentStartConflict refuses only review and
 *  done), whose key is the off-board terminal `cancelled` — in doneKeys' blind
 *  spot, in the closed predicate's plain sight.
 *  So the rule is asked HERE, once, from the same `agentTicketRefusal` the patch
 *  gate, the session loop and the heartbeat use. A caller cannot forget it,
 *  because a caller never states it.
 *
 *  `pickupKeys`, not `agentStartKeys`: the raw flag is the REFUSAL set (entering
 *  any of those columns is assignment, which an agent may not do for itself),
 *  while this is a DESTINATION question — is this column somewhere work is
 *  actually picked up from? A review column carrying `agent_start` is not, and
 *  dispatching into one starts a session on a ticket the review-exit rule has
 *  already frozen. And the agent gate is now PER AGENT, because board policy is
 *  part of the question: an agent still listed as an assignee on a board that
 *  has since revoked its grant gets no fresh work session.
 *
 *  IT IS STILL ASKED AGAIN INSIDE THE SESSION, on every turn. This gate is about
 *  whether to START; a run may sit in the queue, park, or be reclaimed after a
 *  deploy, and the first thing every step does is put the same question to
 *  `agentTicketRefusal` again. */
export async function maybeDispatchTicket(task: Task, onlyAgents?: string[]): Promise<void> {
  const { agentAssignees, agentTicketRefusal } = await import('./tasks')
  const { boardFacts } = await import('./boards')
  const facts = boardFacts()
  const meta = await facts.meta(task.boardId)
  if (!meta.pickupKeys.includes(task.status)) return
  const targets = agentAssignees(task.assignees).filter((a) => !onlyAgents || onlyAgents.includes(a))
  for (const agent of targets) {
    if (await agentTicketRefusal(task, agent, 'write', facts)) continue
    void dispatchTicketWork(task, agent).catch((e) => console.error(`${LOG} ${task.id}: dispatch to ${agent} threw:`, errText(e)))
  }
}
