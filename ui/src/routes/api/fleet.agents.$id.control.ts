import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { getAgentDef } from '@/server/agent-defs'
import { ownsAgent } from '@/server/personal-agent'
import { fleetRemove, fleetRestart, fleetStop, fleetUp, pruneBundledSkills, waitHealthy } from '@/server/fleet-docker'
import { renderFleet } from '@/server/fleet-render'
import { rollAgent } from '@/server/fleet-reconcile'
import { deleteAgentForever } from '@/server/fleet-create'
import { logAudit } from '@/server/audit'
import { db } from '@/server/db/pg'

const Body = z.object({
  action: z.enum(['up', 'stop', 'restart', 'roll', 'retire', 'unretire', 'delete']),
})

// POST { action } → lifecycle control for one agent (admin; owners of a
// personal assistant may up/stop/restart their own).
//   up | stop | restart   the managed service (renders first on `up`)
//   roll                  zero-downtime replacement (admin) — detached
// `up`/`unretire`/`roll` return IMMEDIATELY; the roster's polled container
// health shows the warm-up ('starting') phase instead of blocking the call.
export const Route = createFileRoute('/api/fleet/agents/$id/control')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const ownerAllowed =
          ['up', 'stop', 'restart'].includes(parsed.data.action) && (await ownsAgent(user.id, { defId: params.id }))
        if (!(await hasPerm(user, 'agents.manage')) && !ownerAllowed) return json({ error: 'forbidden' }, { status: 403 })
        const def = await getAgentDef(params.id)
        if (!def) return json({ error: 'not found' }, { status: 404 })

        const sql = await db()
        const actor = user.email ?? user.name ?? 'admin'
        // Lifecycle actions are governance-relevant — record them.
        if (['restart', 'roll', 'retire', 'unretire', 'delete'].includes(parsed.data.action)) {
          void logAudit({ actor, action: `agent.${parsed.data.action}`, targetType: 'agent', targetId: def.id, targetLabel: def.displayName })
        }
        try {
          switch (parsed.data.action) {
            case 'up': {
              if (!def.managed) return json({ error: 'not a managed agent' }, { status: 400 })
              await renderFleet()
              await fleetUp(def.department)
              // Don't block on health — the roster shows the warm-up phase.
              void waitHealthy(def.department).then((ok) => ok && pruneBundledSkills(def.department)).catch(() => {})
              return json({ ok: true, warming: true })
            }
            case 'stop':
              await fleetStop(def.department)
              return json({ ok: true })
            case 'restart': {
              // Quick bounce (brief downtime; in-flight replies drop). For a
              // no-downtime reboot use 'roll'.
              if (!def.managed) return json({ error: 'not a managed agent' }, { status: 400 })
              await fleetRestart(def.department)
              return json({ ok: true, warming: true })
            }
            case 'roll': {
              // Zero-downtime replacement — fresh container, old one drains.
              // Long (health wait + drain), so it runs detached; the roster's
              // health polling tells the story.
              if (!(await hasPerm(user, 'agents.manage'))) return json({ error: 'forbidden' }, { status: 403 })
              if (!def.managed) return json({ error: 'not a managed agent' }, { status: 400 })
              void rollAgent(def.department).catch(() => {})
              return json({ ok: true, rolling: true })
            }
            case 'retire': {
              // Spin down + drop from the fleet. Container removed; the state
              // volume and the version history stay (re-hire with 'unretire').
              await sql`update agent_defs set enabled = false, updated_at = now() where id = ${def.id}`
              await fleetRemove(def.department)
              const render = await renderFleet() // manifest drops it; bridge hot-reloads
              return json({ ok: true, render: render.agents })
            }
            case 'delete': {
              // Permanent: def + versions + secrets + rendered files + (for
              // created agents) the state volume. Admin only, retired only.
              if (!(await hasPerm(user, 'agents.manage'))) return json({ error: 'forbidden' }, { status: 403 })
              const result = await deleteAgentForever(def.id)
              return json({ ok: true, ...result })
            }
            case 'unretire': {
              // Re-hire: re-enable, re-render (manifest + compose pick it back
              // up), and start the managed container from its preserved volume.
              await sql`update agent_defs set enabled = true, updated_at = now() where id = ${def.id}`
              await renderFleet()
              if (def.managed) {
                await fleetUp(def.department)
                void waitHealthy(def.department).then((ok) => ok && pruneBundledSkills(def.department)).catch(() => {})
                return json({ ok: true, warming: true })
              }
              return json({ ok: true })
            }
          }
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 500 })
        }
      },
    },
  },
})
