import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { parseBody, requireUser } from '@/server/api-guard'
import { channelRole, listChannelAgents } from '@/server/channels'
import { routedModelFor } from '@/server/fleet-agents'
import { canUseAgentModel } from '@/server/users'
import { dropDraft, latestDraftFor, saveDraftProposals, startPlanDraft, type StoredProposal } from '@/server/plan-drafts'

const Start = z.object({
  agentModel: z.string().min(1).max(200),
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

// Channel ticket drafts, as a DURABLE JOB (the plan surface's twin — one
// domain module, two conversation kinds). POST enqueues a 'plan-draft' run on
// a channel agent and answers immediately with the queued draft; GET/PATCH/
// DELETE read, persist edits to, and drop the channel's latest draft. Members
// only; the drafting agent must be in the channel and usable by the caller.
// Nothing is created here — the human reviews and creates via the boards API.
export const Route = defineApi('/api/channels/$id/plan', {
  GET: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    return json({ draft: await latestDraftFor(params.id) })
  },
  POST: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Start)
    if (body instanceof Response) return body

    const agents = await listChannelAgents(params.id)
    if (!agents.includes(body.agentModel)) {
      return json({ error: 'that agent is not in this channel' }, { status: 400 })
    }
    if (!(await canUseAgentModel(user.id, user.role, body.agentModel))) {
      return json({ error: 'you do not have access to that agent' }, { status: 403 })
    }
    const routed = (body.tier ? await routedModelFor(body.agentModel, body.tier).catch(() => null) : null) ?? body.agentModel
    try {
      const draft = await startPlanDraft({
        conversationId: params.id,
        source: 'channel',
        userId: user.id,
        agentModel: body.agentModel,
        routedModel: routed,
        tier: body.tier ?? null,
        boardId: body.boardId ?? null,
        templateId: body.templateId ?? null,
      })
      return json({ draft })
    } catch (e) {
      // Row-creation failures are internal text (docker, pg); the run's OWN
      // failures reach the client through the draft row's `error` field, not
      // this 500 — so the body is a fixed sentence, not the raw message.
      console.error('[channels/$id/plan] start failed', { channelId: params.id, agentModel: body.agentModel }, e)
      return json({ error: 'could not start the plan draft — see server logs' }, { status: 500 })
    }
  },
  PATCH: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    const body = await parseBody(request, Save)
    if (body instanceof Response) return body
    await saveDraftProposals(params.id, body.proposals as StoredProposal[])
    return json({ ok: true })
  },
  DELETE: async ({ request, params }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    if (!(await channelRole(user.id, params.id))) return json({ error: 'forbidden' }, { status: 403 })
    await dropDraft(params.id)
    return json({ ok: true })
  },
})
