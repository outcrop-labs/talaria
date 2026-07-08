import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { getAgentDef } from '@/server/agent-defs'
import { ownsAgent } from '@/server/personal-agent'
import { fleetRemove, fleetStop, fleetUp, legacyControl, waitHealthy } from '@/server/fleet-docker'
import { renderFleet } from '@/server/fleet-render'
import { logAudit } from '@/server/audit'
import { db } from '@/server/db/pg'

const Body = z.object({
  action: z.enum(['migrate', 'up', 'stop', 'legacy-start', 'legacy-stop', 'retire', 'unretire']),
})

// POST { action } → lifecycle control for one agent (admin; owners of a
// personal assistant may up/stop their own).
//   migrate       flip to managed, render, stop the legacy container, start the
//                 talaria-managed one, wait for health
//   up | stop     the managed service (renders first on `up`)
//   legacy-*      the old ai-project container
export const Route = createFileRoute('/api/fleet/agents/$id/control')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const ownerAllowed =
          ['up', 'stop'].includes(parsed.data.action) && (await ownsAgent(user.id, { defId: params.id }))
        if (user.role !== 'admin' && !ownerAllowed) return json({ error: 'forbidden' }, { status: 403 })
        const def = await getAgentDef(params.id)
        if (!def) return json({ error: 'not found' }, { status: 404 })

        const sql = await db()
        const actor = user.email ?? user.name ?? 'admin'
        // Lifecycle actions are governance-relevant — record them.
        if (['migrate', 'retire', 'unretire'].includes(parsed.data.action)) {
          void logAudit({ actor, action: `agent.${parsed.data.action}`, targetType: 'agent', targetId: def.id, targetLabel: def.displayName })
        }
        try {
          switch (parsed.data.action) {
            case 'migrate': {
              if (!def.currentVersion) return json({ error: 'no version to render' }, { status: 400 })
              await sql`update agent_defs set managed = true, updated_at = now() where id = ${def.id}`
              const render = await renderFleet()
              await legacyControl(def.department, 'stop').catch(() => {}) // may not exist
              await fleetUp(def.department)
              const healthy = await waitHealthy(def.department)
              return json({ ok: true, healthy, render })
            }
            case 'up': {
              if (!def.managed) return json({ error: 'not talaria-managed — migrate first' }, { status: 400 })
              await renderFleet()
              await fleetUp(def.department)
              const healthy = await waitHealthy(def.department)
              return json({ ok: true, healthy })
            }
            case 'stop':
              await fleetStop(def.department)
              return json({ ok: true })
            case 'legacy-start':
              await legacyControl(def.department, 'start')
              return json({ ok: true })
            case 'legacy-stop':
              await legacyControl(def.department, 'stop')
              return json({ ok: true })
            case 'retire': {
              // Spin down + drop from the fleet. Container removed; the state
              // volume and the version history stay (re-hire with 'unretire').
              await sql`update agent_defs set enabled = false, updated_at = now() where id = ${def.id}`
              if (def.managed) await fleetRemove(def.department)
              else await legacyControl(def.department, 'stop').catch(() => {})
              const render = await renderFleet() // manifest drops it; bridge hot-reloads
              return json({ ok: true, render: render.agents })
            }
            case 'unretire': {
              // Re-hire: re-enable, re-render (manifest + compose pick it back
              // up), and start the managed container from its preserved volume.
              await sql`update agent_defs set enabled = true, updated_at = now() where id = ${def.id}`
              await renderFleet()
              if (def.managed) {
                await fleetUp(def.department)
                const healthy = await waitHealthy(def.department)
                return json({ ok: true, healthy })
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
