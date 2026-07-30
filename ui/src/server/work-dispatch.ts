// Work dispatch + WORK SESSIONS. When a ticket enters an agent-start column
// with agent assignees, Talaria pushes the work into the agent's own persona
// gateway — and then KEEPS THE SESSION GOING: turn after turn, the agent
// drives its tools/harness like a developer at a desk (run, read, steer,
// test, repeat), until the ticket reaches review/blocked/done, the agent
// stops progressing, or the turn cap lands. One turn is never the budget for
// real work. Matched task WORKFLOWS ride along; the plugin heartbeat remains
// the pull-side safety net.
import { proxyChat } from './gateway'
import { parseAgentStream } from '@/lib/sse-parse'
import { estimateTokens, recordUsage } from './usage'
import { workflowsForTask } from './workflows'
import { listStatuses, statusMeta } from './statuses'
import { listAllSkills, SHARED } from './agent-skills'
import { db } from './db/pg'
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

/** Session guard — one live work session per ticket+agent. */
const liveSessions = new Set<string>()

/** Turn budget: generous enough for real feature work, finite enough that a
 *  looping agent can't burn unbounded tokens. The session also ends the
 *  moment the ticket leaves the working statuses. */
const MAX_SESSION_TURNS = 12
const TURN_WAIT_MS = 600_000

/** Where the ticket is now — the session's continue/stop signal. */
async function ticketState(taskId: string, agentModel: string): Promise<{ status: string; assigned: boolean; terminal: boolean } | null> {
  const { getTask } = await import('./tasks')
  const t = await getTask(taskId)
  if (!t) return null
  const meta = await statusMeta(t.boardId)
  const working = new Set([...meta.agentStartKeys, ...(await listStatuses(t.boardId)).filter((s) => s.category === 'active').map((s) => s.key)])
  const assigned = t.assignees.includes(agentModel)
  return { status: t.status, assigned, terminal: !assigned || !working.has(t.status) }
}

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
    const statuses = await listStatuses(task.boardId)
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
    const activeHint = statuses.find((s) => s.category === 'active')?.key
    const prompt =
      `[Assigned work — no human sent this message; a ticket was assigned to you.]\n\n` +
      `Ticket ${task.ticketRef ?? task.id}: "${task.title}"${boardName ? ` (board: ${boardName})` : ''}\n` +
      `${task.description ? `\n${task.description}\n` : ''}` +
      workflowBlock +
      `\n\nThis is a WORK SESSION, not a single exchange — Talaria keeps this conversation going until the work is done. Work like a developer at a desk: act, read the result, steer, act again.\n` +
      `1. get_ticket ${task.id} for full context (comments, attachments, dependencies).\n` +
      `2. comment a one-line acknowledgment, and triage_ticket to status "${activeHint ?? 'in_progress'}" while you work.\n` +
      `3. Do the work in as many steps as it takes — iterate with your tools and (if you have one) your workbench harness: run it, read its structured result, respond to it, verify with tests, repeat.\n` +
      `4. report_outcome when genuinely finished — a human signs off from review. If blocked, set status "blocked" and comment why. Either of those ends the session.\n` +
      `\nBe honest about capability: if you genuinely can't do this properly (a tool or access you're missing, an org-specific process you'd be guessing at), don't improvise — report_gap once with what a flow would need, then block. Never report a gap for work you can simply do.\n` +
      `End each reply with a short status line: what you just did and what you'll do next (or DONE / BLOCKED).`
    const sendTurn = async (content: string): Promise<string> => {
      const upstream = await proxyChat({ model: agentModel, messages: [{ role: 'user', content }] }, { waitMs: TURN_WAIT_MS })
      if (!upstream.ok || !upstream.body) throw new Error(`gateway ${upstream.status}`)
      let text = ''
      let usage: { promptTokens: number; completionTokens: number } | null = null
      for await (const ev of parseAgentStream(upstream.body)) {
        if (ev.type === 'content') text += ev.text
        else if (ev.type === 'usage') usage = ev
      }
      void recordUsage({
        agentModel,
        source: 'chat',
        refId: task.id,
        tier: null,
        promptTokens: usage?.promptTokens ?? estimateTokens(content.length),
        completionTokens: usage?.completionTokens ?? estimateTokens(text.length),
        estimated: !usage,
      }).catch(() => {})
      return text.trim()
    }

    let reply = await sendTurn(prompt)
    await logActivity(task.id, agentModel, 'dispatch', `picked up: ${reply.slice(0, 300) || '(no reply)'}`)

    // The session: keep the agent working until the ticket leaves the working
    // statuses (review/blocked/done/unassigned), it declares DONE/BLOCKED, or
    // the turn cap lands. Every continuation carries the live ticket state so
    // the agent never works a stale picture.
    for (let turn = 2; turn <= MAX_SESSION_TURNS; turn++) {
      const state = await ticketState(task.id, agentModel)
      if (!state || state.terminal) {
        await logActivity(task.id, 'talaria', 'dispatch', `work session ended after ${turn - 1} turn${turn > 2 ? 's' : ''} (ticket ${state ? state.status : 'gone'})`).catch(() => {})
        return
      }
      if (/\b(DONE|BLOCKED)\b/.test(reply.slice(-200))) {
        // The agent says it's finished but the ticket disagrees — one nudge to
        // reconcile (report_outcome / set blocked), then stop pushing.
        reply = await sendTurn(
          `[Work session — reconcile] You said DONE/BLOCKED but the ticket is still "${state.status}". If finished: report_outcome now. If blocked: set status "blocked" with a comment. If neither, keep working.`,
        )
        await logActivity(task.id, agentModel, 'dispatch', `session reconcile: ${reply.slice(0, 200) || '(no reply)'}`).catch(() => {})
        continue
      }
      reply = await sendTurn(
        `[Work session — turn ${turn}/${MAX_SESSION_TURNS}] You're mid-work on this ticket (status: "${state.status}"). Continue like a developer: next step, run it, read the result, adjust. Verify before you finish — tests, your own diff, and for UI work drive it in a real browser (Playwright) and attach evidence. When genuinely done: report_outcome. If stuck: status "blocked" + comment. End with your status line.`,
      )
      await logActivity(task.id, agentModel, 'dispatch', `session turn ${turn}: ${reply.slice(0, 250) || '(no reply)'}`).catch(() => {})
    }
    await logActivity(task.id, 'talaria', 'dispatch', `work session hit the ${MAX_SESSION_TURNS}-turn cap — leaving the ticket to the agent/heartbeat`).catch(() => {})
  } catch (e) {
    await logActivity(task.id, 'talaria', 'dispatch', `work session with ${agentModel} failed: ${(e as Error).message.slice(0, 200)}`).catch(() => {})
  }
}

/** Dispatch to every AGENT assignee when the ticket sits in an agent-start
 *  column. `onlyAgents` narrows to newly-added assignees on updates. */
export async function maybeDispatchTicket(task: Task, onlyAgents?: string[]): Promise<void> {
  const { agentAssignees } = await import('./tasks')
  const meta = await statusMeta(task.boardId)
  if (!meta.agentStartKeys.includes(task.status)) return
  const targets = agentAssignees(task.assignees).filter((a) => !onlyAgents || onlyAgents.includes(a))
  for (const agent of targets) {
    void dispatchTicketWork(task, agent)
  }
}
