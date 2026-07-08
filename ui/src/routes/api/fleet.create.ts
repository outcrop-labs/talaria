import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
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
export const Route = createFileRoute('/api/fleet/create')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          const actor = user.email ?? user.name ?? 'admin'
          const { def, keyCreated } = await createAgent({
            ...parsed.data,
            createdBy: actor,
          })
          for (const s of parsed.data.skills ?? []) {
            await writeSkill(def.slug, s.name, s.content, actor).catch(() => {})
          }
          void logAudit({ actor, action: 'agent.create', targetType: 'agent', targetId: def.id, targetLabel: def.displayName, after: { slug: def.slug, department: def.department } })
          const render = await renderFleet()
          let healthy: boolean | undefined
          if (parsed.data.start) {
            await fleetUp(def.department)
            healthy = await waitHealthy(def.department)
          }
          return json({ ok: true, def, keyCreated, healthy, warnings: render.warnings })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
