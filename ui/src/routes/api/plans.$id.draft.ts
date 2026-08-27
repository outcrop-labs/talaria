import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { parseBody, requireUser } from '@/server/api-guard'
import { accessibleConversation } from '@/server/conversations'
import { routedModelFor } from '@/server/fleet-agents'
import { canUseAgentModel } from '@/server/users'
import { dropDraft, latestDraftFor, saveDraftProposals, startPlanDraft, type StoredProposal } from '@/server/plan-drafts'

const Start = z.object({
  tier: z.string().max(60).nullish(),
  /** Where the tickets will land — lets the board's default template apply. */
  boardId: Uuid.nullish(),
  /** Explicit template pick (overrides agent/board resolution). */
  templateId: Uuid.nullish(),
})

// The review walk's writes: the full batch, already normalized. Narrow on
// purpose — this is what the run produced plus the human's edits, and a shape
// drifting past this schema is a client bug, not user input.
const Save = z.object({
  proposals: z
    .array(
      z.object({
        title: z.string().max(500),
        description: z.string().max(20_000),
        priority: z.enum(['low', 'medium', 'high', 'urgent']),
        effort: z.enum(['xs', 's', 'm', 'l', 'xl']).nullable(),
        dependsOn: z.array(z.number().int()),
        tags: z.array(z.string().max(100)),
        include: z.boolean(),
      }),
    )
    .max(50),
})

// The plan's ticket drafts, as a DURABLE JOB: POST enqueues a 'plan-draft' run
// and answers immediately with the queued draft; the agent reads the
// conversation server-side, so drafting survives a closed tab and a restarted
// server alike. GET is the review's way back to the conversation's latest
// draft; PATCH persists the walk's edits; DELETE drops a consumed or
// discarded batch. Nothing is created by any of this — the human reviews and
// creates via the boards API (PlanModal).
export const Route = defineApi('/api/plans/$id/draft', {
  GET: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await accessibleConversation(user.id, params.id))) return json({ error: 'plan not found' }, { status: 404 })
    return json({ draft: await latestDraftFor(params.id) })
  },
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const agentModel = (await accessibleConversation(user.id, params.id))?.agentModel ?? null
    if (!agentModel) return json({ error: 'plan not found' }, { status: 404 })
    if (!(await canUseAgentModel(user.id, user.role, agentModel))) {
      return json({ error: 'you do not have access to that agent' }, { status: 403 })
    }
    const body = await parseBody(request, Start)
    if (body instanceof Response) return body
    const routed = (body.tier ? await routedModelFor(agentModel, body.tier).catch(() => null) : null) ?? agentModel
    try {
      const draft = await startPlanDraft({
        conversationId: params.id,
        source: 'plan',
        userId: user.id,
        agentModel,
        routedModel: routed,
        tier: body.tier ?? null,
        boardId: body.boardId ?? null,
        templateId: body.templateId ?? null,
      })
      return json({ draft })
    } catch (e) {
      // The row-creation failure is never a sentence worth showing (docker
      // text, pg internals); the run's OWN failures reach the client as the
      // draft row's `error` field, not through this 500.
      console.error('[plan/$id/draft] start failed', { conversationId: params.id, agentModel }, e)
      return json({ error: 'could not start the plan draft — see server logs' }, { status: 500 })
    }
  },
  PATCH: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await accessibleConversation(user.id, params.id))) return json({ error: 'plan not found' }, { status: 404 })
    const body = await parseBody(request, Save)
    if (body instanceof Response) return body
    await saveDraftProposals(params.id, body.proposals as StoredProposal[])
    return json({ ok: true })
  },
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await accessibleConversation(user.id, params.id))) return json({ error: 'plan not found' }, { status: 404 })
    await dropDraft(params.id)
    return json({ ok: true })
  },
})
