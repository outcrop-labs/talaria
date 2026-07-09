import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { ownedConversation } from '@/server/conversations'
import { ensurePlanDoc, syncPlanDoc } from '@/server/plan-doc'
import { routedModelFor } from '@/server/fleet-agents'
import { canUseAgentModel } from '@/server/users'

const Body = z.object({ tier: z.string().max(60).nullish() })

// The plan's living document (a linked doc artifact). GET → find-or-create it,
// seeded from the agent's plan template when one is bound. POST → the plan's
// agent rewrites it from the conversation so far. Owner only.
export const Route = createFileRoute('/api/plan/$id/doc')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const owned = await ownedConversation(user.id, params.id)
        if (!owned || owned.kind !== 'plan') return json({ error: 'plan not found' }, { status: 404 })
        const artifact = await ensurePlanDoc(
          params.id,
          { id: user.id, label: user.name ?? user.email ?? 'someone' },
          owned.title,
          owned.agentModel,
        )
        return json({ artifact })
      },
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const owned = await ownedConversation(user.id, params.id)
        if (!owned || owned.kind !== 'plan') return json({ error: 'plan not found' }, { status: 404 })
        if (!(await canUseAgentModel(user.id, user.role, owned.agentModel))) {
          return json({ error: 'you do not have access to that agent' }, { status: 403 })
        }
        const parsed = Body.safeParse(await request.json().catch(() => ({})))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const routed =
          (parsed.data.tier ? await routedModelFor(owned.agentModel, parsed.data.tier).catch(() => null) : null) ??
          owned.agentModel
        try {
          const artifact = await syncPlanDoc(
            params.id,
            { id: user.id, label: user.name ?? user.email ?? 'someone' },
            owned.title,
            owned.agentModel,
            routed,
          )
          return json({ artifactId: artifact.id })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 502 })
        }
      },
    },
  },
})
