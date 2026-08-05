import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requirePerm } from '@/server/api-guard'
import { createAgent } from '@/server/fleet-create'
import { writeSkill } from '@/server/agent-skills'
import { logAudit } from '@/server/audit'
import { fleetUp, waitHealthy } from '@/server/fleet-docker'
import { renderFleet } from '@/server/fleet-render'

const Body = z.object({
  slug: z.string().min(2).max(30),
  department: z.string().min(2).max(40),
  displayName: z.string().min(1).max(60),
  role: z.string().max(80).nullish(),
  /** Clone this agent's config; omit for the platform defaults. */
  templateId: z.string().uuid().optional(),
  /** Override the starter-soul scaffold (e.g. an AI-designed soul). */
  soul: z.string().max(200_000).optional(),
  /** Starter skills written after creation (e.g. AI-designed playbooks). */
  skills: z
    .array(z.object({ name: z.string().regex(/^[a-z0-9][a-z0-9._-]*$/), content: z.string().max(100_000) }))
    .max(5)
    .optional(),
  start: z.boolean().optional(),
})

// POST → create a new agent from a template (an existing agent's definition):
// fresh gateway key, re-stamped config, soul (scaffold or supplied), optional
// starter skills, v1. Optionally render + start it immediately. Admin.
export const Route = defineApi('/api/fleet/create', {
  POST: async ({ request }) => {
    const user = await requirePerm(request, 'agents.manage')
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    try {
      const actor = user.email ?? user.name ?? 'admin'
      const { def, keyCreated } = await createAgent({
        ...body,
        createdBy: actor,
      })
      for (const s of body.skills ?? []) {
        await writeSkill(def.slug, s.name, s.content, actor).catch(() => {})
      }
      void logAudit({ actor: actorOf(user), action: 'agent.create', targetType: 'agent', targetId: def.id, targetLabel: def.displayName, after: { slug: def.slug, department: def.department } })
      const render = await renderFleet()
      let healthy: boolean | undefined
      if (body.start) {
        await fleetUp(def.department)
        healthy = await waitHealthy(def.department)
      }
      return json({ ok: true, def, keyCreated, healthy, warnings: render.warnings })
    } catch (e) {
      return json({ error: (e as Error).message }, { status: 400 })
    }
  },
})
