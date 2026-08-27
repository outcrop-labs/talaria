import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { TICKET_COLORS } from '@/lib/task-const'
import { statusMeta } from '@/server/statuses'
import { parseBody, requireUser, type SessionUser } from '@/server/api-guard'
import { agentCaller } from '@/server/agent-auth'
import { boardAllowsAgent, boardRole, canEdit, invalidAssignee, listMembers } from '@/server/boards'
import { notifyMentions } from '@/server/mentions'
import { describeAgent } from '@/server/gateway'
import { deleteTask, getTask, getTaskFull, listComments, EFFORTS, HumanApprovalRequired, PRIORITIES, updateTask, type TaskActor, type TaskPatch } from '@/server/tasks'
import { resolveAttachments } from '@/server/uploads'
import { resolveRefs } from '@/server/refs'
import { indexTicket, unindexActivity } from '@/server/retrieval/sources'
import { runJudgeForTask } from '@/server/judge'

const Patch = z.object({
  title: z.string().min(1).max(300).optional(),
  description: z.string().max(20_000).nullish(),
  // `.min(1)` is not cosmetic: without it `""` was a legal patch value, and it
  // is FALSY, so every `patch.status && …` guard in updateTask/agentSafePatch
  // skipped — board validation, the review exit, the blocked exit, the
  // assignment gate — on a write that then stored an empty column. Those guards
  // now ask for PRESENCE rather than truth (server/tasks.ts), so this is the
  // outer of two layers; a blank status is refused either way.
  status: z.string().min(1).max(40).optional(), // validated against the BOARD's status set in updateTask
  priority: z.enum(PRIORITIES).optional(),
  effort: z.enum(EFFORTS).nullish(),
  assignees: z.array(z.string().max(200)).max(20).optional(),
  dueDate: z.string().datetime().nullish(),
  startDate: z.string().datetime().nullish(),
  color: z.enum(TICKET_COLORS).nullish(),
  // Same class: `[""]` minted a blank label on the board. `[]` remains legal —
  // it clears the labels, and updateTask now writes an activity line saying so.
  tags: z.array(z.string().min(1).max(40)).max(20).optional(),
  outcome: z.string().max(50_000).nullish(),
  resolution: z.string().max(50_000).nullish(),
  errorMessage: z.string().max(50_000).nullish(),
  archived: z.boolean().optional(),
  estimatedHours: z.number().min(0).max(999).nullish(),
  parentId: Uuid.nullish(),
  addTimeSpentSeconds: z.number().min(0).max(86_400 * 30).optional(),
  // Full replacement list, same contract as chat messages: upload ids +
  // knowledge/artifact refs. Omit both to leave attachments unchanged.
  attachmentIds: z.array(Uuid).max(20).optional(),
  refs: z.array(z.object({ type: z.enum(['kb-doc', 'artifact']), id: Uuid })).max(3).optional(),
})

export const Route = defineApi('/api/tasks/$id', {
  GET: async ({ request, params }) => {
    const full = await getTaskFull(params.id)
    if (!full) return json({ error: 'not found' }, { status: 404 })
    const caller = await agentCaller(request)
    if (caller instanceof Response) return caller
    if (caller) {
      // The CALLER, not its model — board policy's elevated bypass is only
      // for an identity that was proven, never merely asserted.
      if (!(await boardAllowsAgent(full.task.boardId, caller))) return json({ error: 'forbidden' }, { status: 403 })
      const { workflowsForTask } = await import('@/server/workflows')
      return json({ ...full, workflows: await workflowsForTask(full.task) })
    }
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    if (!(await boardRole(user.id, full.task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
    return json(full)
  },
  PUT: async ({ request, params }) => {
    const task = await getTask(params.id)
    if (!task) return json({ error: 'not found' }, { status: 404 })

    const agent = await agentCaller(request)
    if (agent instanceof Response) return agent
    let actor: TaskActor
    let sessionUser: SessionUser | null = null
    if (agent) {
      // Identity comes from the credential, so board policy is
      // unconditional — there is no longer an unnamed caller to wave
      // through (the old `if (name)` made this whole gate opt-out). Pass the
      // caller so the elevated-assistant bypass sees `legacy`.
      if (!(await boardAllowsAgent(task.boardId, agent))) {
        return json({ error: `agent "${agent.model}" is not allowed on this board` }, { status: 403 })
      }
      actor = { kind: 'agent', id: agent.model }
    } else {
      const gate = await requireUser(request)
      if (gate instanceof Response) return gate
      const user = gate
      if (!canEdit(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
      actor = { kind: 'human', id: user.email ?? user.name ?? 'user' }
      sessionUser = user
    }

    const parsed = await parseBody(request, Patch)
    if (parsed instanceof Response) return parsed
    // Human-in-the-loop guardrails (assignment, sign-off, archival) belong
    // to the ACTOR, not this route: updateTask enforces them for every
    // caller, so nothing agent-specific happens here.
    //
    // Mixed assignees: humans as `user:<uuid>` (board members), agents by
    // model id (board agent policy).
    const bad = await invalidAssignee(task.boardId, parsed.assignees ?? [])
    if (bad) return json({ error: bad }, { status: 400 })
    const { attachmentIds, refs, ...patch } = parsed
    if (attachmentIds !== undefined || refs !== undefined) {
      // Resolve to canonical chips server-side (never trust client metadata).
      // Refs are ACL-checked against the attacher, so agent callers (no
      // session) can attach uploads but not knowledge/artifact refs.
      const uploads = await resolveAttachments(attachmentIds ?? [])
      const chips = sessionUser ? await resolveRefs(sessionUser, refs ?? []) : []
      ;(patch as TaskPatch).attachments = [...uploads, ...chips]
    }
    let updated
    try {
      updated = await updateTask(params.id, patch as TaskPatch, actor)
    } catch (e) {
      if (e instanceof HumanApprovalRequired) return json({ error: e.message }, { status: 403 })
      return json({ error: (e as Error).message }, { status: 400 })
    }
    // Keep the activity brain fresh when the ticket's text changed.
    if (updated && (parsed.title !== undefined || parsed.description !== undefined)) {
      void indexTicket(updated).catch(() => {})
    }
    // A description that gains an @mention notifies board members — same
    // contract as comments. Only on actual change, never on other patches.
    if (updated && parsed.description !== undefined && parsed.description !== task.description) {
      void (async () => {
        const members = await listMembers(task.boardId)
        await notifyMentions(
          members,
          sessionUser?.id ?? '',
          sessionUser ? (sessionUser.name ?? actor.id) : describeAgent(actor.id).label,
          parsed.description ?? '',
          updated.ticketRef ?? 'a ticket',
          `/boards/${task.boardId}/${task.id}`,
        )
      })().catch(() => {})
    }
    // Reliability gate: when work lands in ANY review-category column, run
    // the QA judge (advisory) in the background so the human reviewer gets
    // a verdict. Custom review columns count — category is the contract.
    if (updated && updated.status !== task.status) {
      const meta2 = await statusMeta(task.boardId)
      if (meta2.reviewKeys.includes(updated.status) && !meta2.reviewKeys.includes(task.status)) {
        void runJudgeForTask(params.id).catch(() => {})
      }
    }
    return json({ task: updated })
  },
  DELETE: async ({ request, params }) => {
    const gate = await requireUser(request)
    if (gate instanceof Response) return gate
    const user = gate
    const task = await getTask(params.id)
    if (!task) return json({ error: 'not found' }, { status: 404 })
    if (!canEdit(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
    // Drop the ticket + its comments from the activity brain before deleting.
    const comments = await listComments(params.id).catch(() => [])
    await deleteTask(params.id)
    void unindexActivity('ticket', params.id).catch(() => {})
    for (const c of comments) void unindexActivity('comment', c.id).catch(() => {})
    return json({ ok: true })
  },
})
