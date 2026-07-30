// Work dispatch — the missing link between "a human assigned a ticket" and
// "the agent actually starts". When a ticket sits in an agent-start column
// with agent assignees, Talaria PUSHES the work into each agent's own persona
// gateway (the same governed path outreach uses): the agent acknowledges,
// works through its normal talaria MCP tools, and reports to review. Matched
// task WORKFLOWS ride along — the flow/instructions for this kind of work.
// The plugin heartbeat remains the pull-side safety net.
import { proxyChat } from './gateway'
import { parseAgentStream } from '@/lib/sse-parse'
import { estimateTokens, recordUsage } from './usage'
import { workflowsForTask } from './workflows'
import { listStatuses, statusMeta } from './statuses'
import type { Task } from '@/lib/task-const'

/** Push one ticket to one agent. Fire-and-forget from task mutations. */
export async function dispatchTicketWork(task: Task, agentModel: string, boardName?: string): Promise<void> {
  const { logActivity } = await import('./tasks')
  try {
    const statuses = await listStatuses(task.boardId)
    const workflows = await workflowsForTask(task)
    const workflowBlock = workflows.length
      ? `\n\nHOW THIS KIND OF WORK IS DONE HERE (workflow${workflows.length > 1 ? 's' : ''}):\n` +
        workflows
          .map((h) => {
            const kits = h.toolkits.length
              ? `\nExpected toolkits: ${h.toolkits.map((t) => t.server + (t.tools?.length ? ` (${t.tools.join(', ')})` : '')).join('; ')}`
              : ''
            return `── ${h.name} ──\n${h.instructions}${kits}`
          })
          .join('\n\n')
      : ''
    // The lifecycle is auditable from the ticket: every step lands in
    // task_activity (dispatch start w/ matched workflows, the agent's reply
    // or the failure, then the agent's own status/comment actions).
    await logActivity(
      task.id,
      'talaria',
      'dispatch',
      `work pushed to ${agentModel}${workflows.length ? ` with workflow ${workflows.map((h) => h.name).join(', ')}` : ''}`,
    )
    const activeHint = statuses.find((s) => s.category === 'active')?.key
    const prompt =
      `[Assigned work — no human sent this message; a ticket was assigned to you.]\n\n` +
      `Ticket ${task.ticketRef ?? task.id}: "${task.title}"${boardName ? ` (board: ${boardName})` : ''}\n` +
      `${task.description ? `\n${task.description}\n` : ''}` +
      workflowBlock +
      `\n\nWork it now, through your talaria tools:\n` +
      `1. get_ticket ${task.id} for full context (comments, attachments, dependencies).\n` +
      `2. comment a one-line acknowledgment, and triage_ticket to status "${activeHint ?? 'in_progress'}" while you work.\n` +
      `3. Do the work. If you are blocked, set status "blocked" and comment why.\n` +
      `4. report_outcome when finished — a human signs off from review.\n` +
      `Reply here with ONE short line: what you did or what blocks you.`
    const upstream = await proxyChat({ model: agentModel, messages: [{ role: 'user', content: prompt }] }, { waitMs: 120_000 })
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
      promptTokens: usage?.promptTokens ?? estimateTokens(prompt.length),
      completionTokens: usage?.completionTokens ?? estimateTokens(text.length),
      estimated: !usage,
    }).catch(() => {})
    await logActivity(task.id, agentModel, 'dispatch', `picked up: ${text.trim().slice(0, 300) || '(no reply)'}`)
  } catch (e) {
    await logActivity(task.id, 'talaria', 'dispatch', `dispatch to ${agentModel} failed: ${(e as Error).message.slice(0, 200)}`).catch(() => {})
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
