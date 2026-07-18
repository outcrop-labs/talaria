import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { agentName, checkAgentKey } from '@/server/agent-auth'
import { boardAllowsAgent, boardRole, canEdit } from '@/server/boards'
import { deleteTask, getTask, getTaskFull, listComments, EFFORTS, PRIORITIES, TASK_STATUSES, updateTask, type TaskPatch } from '@/server/tasks'
import { resolveAttachments } from '@/server/uploads'
import { resolveRefs } from '@/server/refs'
import { indexTicket, unindexActivity } from '@/server/retrieval/sources'
import { runJudgeForTask } from '@/server/judge'

const AllStatuses = [...TASK_STATUSES, 'failed', 'cancelled'] as const
const Patch = z.object({
  title: z.string().min(1).max(300).optional(),
  description: z.string().max(20_000).nullish(),
  status: z.enum(AllStatuses).optional(),
  priority: z.enum(PRIORITIES).optional(),
  effort: z.enum(EFFORTS).nullish(),
  assignees: z.array(z.string().max(200)).max(20).optional(),
  dueDate: z.string().datetime().nullish(),
  tags: z.array(z.string().max(40)).max(20).optional(),
  outcome: z.string().max(50_000).nullish(),
  resolution: z.string().max(50_000).nullish(),
  errorMessage: z.string().max(50_000).nullish(),
  archived: z.boolean().optional(),
  addTimeSpentSeconds: z.number().min(0).max(86_400 * 30).optional(),
  // Full replacement list, same contract as chat messages: upload ids +
  // knowledge/artifact refs. Omit both to leave attachments unchanged.
  attachmentIds: z.array(z.string().uuid()).max(20).optional(),
  refs: z.array(z.object({ type: z.enum(['kb-doc', 'artifact']), id: z.string().uuid() })).max(3).optional(),
})

export const Route = createFileRoute('/api/tasks/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const full = await getTaskFull(params.id)
        if (!full) return json({ error: 'not found' }, { status: 404 })
        if (checkAgentKey(request)) {
          const agent = agentName(request)
          if (!agent) return json({ error: 'x-agent-name required' }, { status: 400 })
          if (!(await boardAllowsAgent(full.task.boardId, agent))) return json({ error: 'forbidden' }, { status: 403 })
          return json(full)
        }
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (!(await boardRole(user.id, full.task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
        return json(full)
      },
      PUT: async ({ request, params }) => {
        const task = await getTask(params.id)
        if (!task) return json({ error: 'not found' }, { status: 404 })

        const agent = checkAgentKey(request)
        let actor = 'agent'
        let sessionUser: Awaited<ReturnType<typeof getSessionUser>> = null
        if (agent) {
          // A named agent must pass the board's agent policy; unnamed callers
          // (legacy plugin heartbeat/report) keep the old fleet-wide access.
          const name = agentName(request)
          if (name) {
            if (!(await boardAllowsAgent(task.boardId, name))) {
              return json({ error: `agent "${name}" is not allowed on this board` }, { status: 403 })
            }
            actor = name
          }
        } else {
          const user = await getSessionUser(request)
          if (!user || !canEdit(await boardRole(user.id, task.boardId))) return json({ error: 'forbidden' }, { status: 403 })
          actor = user.email ?? user.name ?? 'user'
          sessionUser = user
        }

        const parsed = Patch.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        // Human-in-the-loop guardrails for agents: they may triage (priority,
        // effort, labels, description, status → in_progress/blocked/quality_review)
        // but cannot assign work or sign off. Assignment + done stay human.
        if (agent) {
          if (parsed.data.status === 'assigned') return json({ error: 'agents cannot assign tickets' }, { status: 403 })
          if (parsed.data.status === 'done') parsed.data.status = 'quality_review'
          parsed.data.assignees = undefined
        }
        for (const a of parsed.data.assignees ?? []) {
          if (!(await boardAllowsAgent(task.boardId, a))) {
            return json({ error: `agent "${a}" is not allowed on this board` }, { status: 400 })
          }
        }
        const { attachmentIds, refs, ...patch } = parsed.data
        if (attachmentIds !== undefined || refs !== undefined) {
          // Resolve to canonical chips server-side (never trust client metadata).
          // Refs are ACL-checked against the attacher, so agent callers (no
          // session) can attach uploads but not knowledge/artifact refs.
          const uploads = await resolveAttachments(attachmentIds ?? [])
          const chips = sessionUser ? await resolveRefs(sessionUser, refs ?? []) : []
          ;(patch as TaskPatch).attachments = [...uploads, ...chips]
        }
        const updated = await updateTask(params.id, patch, actor)
        // Keep the activity brain fresh when the ticket's text changed.
        if (updated && (parsed.data.title !== undefined || parsed.data.description !== undefined)) {
          void indexTicket(updated).catch(() => {})
        }
        // Reliability gate: when work lands in quality_review, run the QA judge
        // (advisory) in the background so the human reviewer gets a verdict.
        if (updated && updated.status === 'quality_review' && task.status !== 'quality_review') {
          void runJudgeForTask(params.id).catch(() => {})
        }
        return json({ task: updated })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
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
    },
  },
})
