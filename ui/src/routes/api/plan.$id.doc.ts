import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { accessibleConversation } from '@/server/conversations'
import { ensurePlanDoc, syncPlanDoc } from '@/server/plan-doc'
import { routedModelFor } from '@/server/fleet-agents'
import { canUseAgentModel } from '@/server/users'

const Body = z.object({ tier: z.string().max(60).nullish() })

// The plan's living document (a linked doc artifact). GET → find-or-create it,
// seeded from the agent's plan template when one is bound. POST → the plan's
// agent rewrites it from the conversation so far. Owner or plan collaborator;
// the document is always OWNED by the plan's owner regardless of who touched
// it first.
export const Route = createFileRoute('/api/plan/$id/doc')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const conv = await accessibleConversation(user.id, params.id)
        if (!conv || conv.kind !== 'plan') return json({ error: 'plan not found' }, { status: 404 })
        const artifact = await ensurePlanDoc(
          params.id,
          { id: conv.ownerUserId, label: user.name ?? user.email ?? 'someone' },
          conv.title,
          conv.agentModel,
          conv.planTemplateId,
        )
        return json({ artifact })
      },
      POST: async ({ request, params }) => {
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const conv = await accessibleConversation(user.id, params.id)
        if (!conv || conv.kind !== 'plan') return json({ error: 'plan not found' }, { status: 404 })
        if (!(await canUseAgentModel(user.id, user.role, conv.agentModel))) {
          return json({ error: 'you do not have access to that agent' }, { status: 403 })
        }
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        const routed =
          (body.tier ? await routedModelFor(conv.agentModel, body.tier).catch(() => null) : null) ??
          conv.agentModel
        try {
          const artifact = await syncPlanDoc(
            params.id,
            { id: conv.ownerUserId, label: user.name ?? user.email ?? 'someone' },
            conv.title,
            conv.agentModel,
            routed,
            conv.planTemplateId,
          )
          return json({ artifactId: artifact.id })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 502 })
        }
      },
    },
  },
})
