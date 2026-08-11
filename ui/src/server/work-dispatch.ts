// Work dispatch + WORK SESSIONS. When a ticket enters an agent-start column
// with agent assignees, Talaria pushes the work into the agent's own persona
// gateway — and then KEEPS THE SESSION GOING: turn after turn, the agent
// drives its tools/harness like a developer at a desk (run, read, steer,
// test, repeat), until the ticket reaches review/blocked/done, the agent
// stops progressing, or the turn cap lands. One turn is never the budget for
// real work. Matched task WORKFLOWS ride along; the plugin heartbeat remains
// the pull-side safety net.
//
// THE TURN ITSELF IS A HARNESS (harness/defs/work-session.ts). Everything in
// this file is ticket-state orchestration — who may still work this ticket, when
// the session stops, what lands in the activity trail — and every predicate in
// it is unchanged. The one thing that moved is the model call: an agent's reply
// saying a ticket is DONE now goes through `runHarness`, so `zero_tool_claim`
// finally runs on the output it was written for (audit 1.5).
//
// AND NOTHING IN THIS FILE REACHES A MODEL ANY MORE. It used to carry
// `sessionTransport` — a hand-written persona transport that existed for three
// slots `TransportRequest` did not have: the agent's own tools, the ticket the
// spend belongs to, and a ten-minute hold for an agent restarting under a config
// propagation. All three exist now, two as declarations on the harness
// (`tools: 'own'`, `holdMs`) and one as `RunContext.ledger`, so the transport is
// gone rather than reduced. That matters beyond tidiness: the shim mapped a
// request onto a `proxyChat` payload by hand and dropped `temperature` and
// `jsonMode` doing it, which is the exact class of silent divergence the harness
// layer exists to end.
import { workflowsForTask } from './workflows'
import { statusMeta } from './statuses'
import { listAllSkills, SHARED } from './agent-skills'
import { db } from './db/pg'
import { runHarness } from './harness/run'
import { workSessionHarness } from './harness/defs/work-session'
import type { Finding } from './guardrails'
import type { Task } from '@/lib/task-const'

/** Skill names this agent can actually load: the shared root + its own. */
async function agentSkillNames(agentModel: string): Promise<Set<string>> {
  const sql = await db()
  const [def] = (await sql`select slug from agent_defs where model = ${agentModel}`) as unknown as Array<{ slug: string }>
  const names = new Set<string>()
  for (const o of await listAllSkills()) {
    if (o.owner === SHARED || o.owner === def?.slug) for (const sk of o.skills) names.add(sk.name)
  }
  return names
}

/** Session guard — one live work session per ticket+agent.
 *  TODO(multi-instance): this is process-local; running several app
 *  instances (or restarting mid-session) can double-run a session. Move to
 *  a DB claim (insert … on conflict do nothing) when we scale out. */
const liveSessions = new Set<string>()

/** Turn budget: generous enough for real feature work, finite enough that a
 *  looping agent can't burn unbounded tokens. The session also ends the
 *  moment the ticket leaves the working statuses. */
const MAX_SESSION_TURNS = 12

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
 *  session ended instead of just naming a column. */
async function sessionState(
  taskId: string,
  agentModel: string,
): Promise<{ status: string; stop: string | null } | null> {
  const { getTask, agentTicketRefusal } = await import('./tasks')
  const { boardFacts } = await import('./boards')
  const t = await getTask(taskId)
  if (!t) return null
  const facts = boardFacts()
  const refusal = await agentTicketRefusal(t, agentModel, 'write', facts)
  if (refusal) return { status: t.status, stop: refusal }
  if (!t.assignees.includes(agentModel)) return { status: t.status, stop: 'no longer assigned to this agent' }
  const meta = await facts.meta(t.boardId)
  if (!meta.workingKeys.includes(t.status)) return { status: t.status, stop: `ticket moved to "${t.status}"` }
  return { status: t.status, stop: null }
}

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
const noted = (line: string, findings: Finding[]): string =>
  findings.length ? `[guard: ${[...new Set(findings.map((f) => f.check))].join(', ')}] ${line}` : line

/** Drive one ticket with one agent as a SESSION. Fire-and-forget from task
 *  mutations; re-entry for the same ticket+agent is a no-op. */
export async function dispatchTicketWork(task: Task, agentModel: string, boardName?: string): Promise<void> {
  const key = `${task.id}:${agentModel}`
  if (liveSessions.has(key)) return
  liveSessions.add(key)
  try {
    await runWorkSession(task, agentModel, boardName)
  } finally {
    liveSessions.delete(key)
  }
}

async function runWorkSession(task: Task, agentModel: string, boardName?: string): Promise<void> {
  const { logActivity } = await import('./tasks')
  try {
    const meta = await statusMeta(task.boardId)
    const workflows = await workflowsForTask(task)
    // Workflows name SKILLS — Hermes loads the flow content from the skill
    // mounts it already reads; we never paste flow prose into the prompt.
    // Skills the target agent can't see are flagged, not silently named
    // (the future gap loop starts from exactly this signal).
    const available = workflows.some((w) => w.skills.length) ? await agentSkillNames(agentModel) : new Set<string>()
    const missing = workflows.flatMap((w) => w.skills.filter((sk) => !available.has(sk)))
    const workflowBlock = workflows.length
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
    // The lifecycle is auditable from the ticket: every step lands in
    // task_activity (dispatch start w/ matched workflows + skills, the
    // agent's reply or the failure, then the agent's own actions).
    await logActivity(
      task.id,
      'talaria',
      'dispatch',
      `work pushed to ${agentModel}` +
        (workflows.length ? ` with workflow ${workflows.map((w) => w.name + (w.skills.length ? ` [${w.skills.join(', ')}]` : '')).join(', ')}` : '') +
        (missing.length ? ` — skill ${missing.map((m) => `"${m}"`).join(', ')} not available to this agent` : ''),
    )
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
    const activeHint = meta.activeKey ?? meta.assignedKey
    const step2 = activeHint
      ? `comment a one-line acknowledgment, and triage_ticket to status "${activeHint}" while you work.`
      : `comment a one-line acknowledgment. Leave the status where it is — this board has no working column for you to move it to.`
    const prompt =
      `[Assigned work — no human sent this message; a ticket was assigned to you.]\n\n` +
      `Ticket ${task.ticketRef ?? task.id}: "${task.title}"${boardName ? ` (board: ${boardName})` : ''}\n` +
      `${task.description ? `\n${task.description}\n` : ''}` +
      workflowBlock +
      `\n\nThis is a WORK SESSION, not a single exchange — Talaria keeps this conversation going until the work is done. Work like a developer at a desk: act, read the result, steer, act again.\n` +
      `1. get_ticket ${task.id} for full context (comments, attachments, dependencies).\n` +
      `2. ${step2}\n` +
      `3. Do the work in as many steps as it takes — iterate with your tools and (if you have one) your workbench harness: run it, read its structured result, respond to it, verify with tests, repeat.\n` +
      `4. report_outcome when genuinely finished — a human signs off from review. If blocked, set status "blocked" and comment why. Either of those ends the session.\n` +
      `That status move in step 4 is your LAST one on this ticket. Once it is in review, or parked in blocked, only a person moves it again — triage_ticket will refuse you with a 403, and so will add_time once the ticket is closed. Don't retry it; comment instead, which stays open.\n` +
      `\nBe honest about capability: if you genuinely can't do this properly (a tool or access you're missing, an org-specific process you'd be guessing at), don't improvise — report_gap once with what a flow would need, then block. Never report a gap for work you can simply do.\n` +
      `End each reply with a short status line: what you just did and what you'll do next (or DONE / BLOCKED).`
    // ONE TURN = ONE HARNESS RUN, on the runner's own transport. The model is
    // named by the caller (it is the agent assigned to this ticket, not a
    // chain-resolved one); the tool loop and the ten-minute hold are declared on
    // the harness; and everything else is the runner's — the guard pass that
    // finally covers this output, redaction of the copy that gets persisted, and
    // the harness_runs row.
    //
    // THE LEDGER LINE IS THE ONE THING ONLY THIS FILE CAN SAY. `recordUsage`
    // writes `task_id`, and `taskUsage` sums a ticket's cost by that column
    // alone — so without `taskId` here a session's spend lands in the ledger and
    // never in the number the ticket's owner reads. `source` stays 'chat'
    // deliberately: 'ticket' rows are agent-SELF-REPORTED through MCP
    // `log_usage`, and this turn is metered by Talaria on its own request path.
    //
    // The policy — a turn that produced nothing usable ends the session with a
    // logged failure instead of driving eleven more turns off a blank — lives in
    // `workSessionHarness.onFailure: 'throw'` and nowhere else now. It used to be
    // restated here because `runHarness` RETURNED rather than threw for the
    // pre-call failures (nothing resolved, render threw, the transport died
    // mid-stream); it throws on all of them, and the throw lands in the outer
    // catch below exactly as the old `throw new Error('gateway ' + status)` did.
    // What is left is the TYPE narrowing: `HarnessResult.value` is `O | null` on
    // every result alike, so the compiler cannot see the guarantee.
    const sendTurn = async (content: string): Promise<{ text: string; findings: Finding[] }> => {
      const run = await runHarness(
        workSessionHarness,
        { prompt: content },
        { caller: `ticket:${task.id}`, model: agentModel, ledger: { source: 'chat', refId: task.id, taskId: task.id } },
      )
      if (run.value === null) throw new Error(run.error ?? `no reply from ${agentModel}`)
      return { text: run.value, findings: run.findings }
    }

    let last = await sendTurn(prompt)
    await logActivity(task.id, agentModel, 'dispatch', noted(`picked up: ${last.text.slice(0, 300) || '(no reply)'}`, last.findings))

    // The session: keep the agent working until the ticket leaves the working
    // statuses (review/blocked/done/unassigned), it declares DONE/BLOCKED, or
    // the turn cap lands. Every continuation carries the live ticket state so
    // the agent never works a stale picture.
    for (let turn = 2; turn <= MAX_SESSION_TURNS; turn++) {
      const state = await sessionState(task.id, agentModel)
      if (!state || state.stop) {
        await logActivity(
          task.id,
          'talaria',
          'dispatch',
          `work session ended after ${turn - 1} turn${turn > 2 ? 's' : ''} — ${state ? state.stop : 'ticket gone'}`,
        ).catch(() => {})
        return
      }
      // CASE-INSENSITIVE, and the fitness suite is what found it: a model that
      // ended its turn "**Status:** Blocked" plainly meant blocked, and this
      // read it as still working — so the session kept waking up on a parked
      // ticket, which is the exact bug the status line exists to prevent. The
      // convention asks for a word, not for shouting.
      if (/\b(DONE|BLOCKED)\b/i.test(last.text.slice(-200))) {
        // The agent says it's finished but the ticket disagrees — one nudge to
        // reconcile (report_outcome / set blocked), then stop pushing.
        last = await sendTurn(
          `[Work session — reconcile] You said DONE/BLOCKED but the ticket is still "${state.status}". If finished: report_outcome now. If blocked: set status "blocked" with a comment. If neither, keep working.`,
        )
        await logActivity(task.id, agentModel, 'dispatch', noted(`session reconcile: ${last.text.slice(0, 200) || '(no reply)'}`, last.findings)).catch(() => {})
        continue
      }
      last = await sendTurn(
        `[Work session — turn ${turn}/${MAX_SESSION_TURNS}] You're mid-work on this ticket (status: "${state.status}"). Continue like a developer: next step, run it, read the result, adjust. Verify before you finish — tests, your own diff, and for UI work drive it in a real browser (Playwright) and attach evidence. When genuinely done: report_outcome. If stuck: status "blocked" + comment. End with your status line.`,
      )
      await logActivity(task.id, agentModel, 'dispatch', noted(`session turn ${turn}: ${last.text.slice(0, 250) || '(no reply)'}`, last.findings)).catch(() => {})
    }
    await logActivity(task.id, 'talaria', 'dispatch', `work session hit the ${MAX_SESSION_TURNS}-turn cap — leaving the ticket to the agent/heartbeat`).catch(() => {})
  } catch (e) {
    await logActivity(task.id, 'talaria', 'dispatch', `work session with ${agentModel} failed: ${(e as Error).message.slice(0, 200)}`).catch(() => {})
  }
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
 *  has since revoked its grant gets no fresh work session. */
export async function maybeDispatchTicket(task: Task, onlyAgents?: string[]): Promise<void> {
  const { agentAssignees, agentTicketRefusal } = await import('./tasks')
  const { boardFacts } = await import('./boards')
  const facts = boardFacts()
  const meta = await facts.meta(task.boardId)
  if (!meta.pickupKeys.includes(task.status)) return
  const targets = agentAssignees(task.assignees).filter((a) => !onlyAgents || onlyAgents.includes(a))
  for (const agent of targets) {
    if (await agentTicketRefusal(task, agent, 'write', facts)) continue
    void dispatchTicketWork(task, agent)
  }
}
