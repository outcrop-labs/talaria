import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { addVersionIfChanged, getAgentDef, listVersions } from '@/server/agent-defs'
import { applyMcpEdits } from '@/server/agent-mcp'
import { fleetRestart } from '@/server/fleet-docker'
import { renderFleet } from '@/server/fleet-render'

const Body = z.object({
  add: z
    .array(
      z.object({
        name: z.string().regex(/^[a-z0-9][a-z0-9_-]*$/).max(60),
        url: z.string().url().max(300),
        timeout: z.number().int().positive().max(3600).optional(),
      }),
    )
    .max(20)
    .default([]),
  remove: z.array(z.string().max(60)).max(20).default([]),
  apply: z.boolean().optional(),
})

// POST → add/remove MCP servers on an agent as a NEW config version (same
// versioned-internals contract as model edits), optionally applied live.
export const Route = createFileRoute('/api/fleet/defs/$id/mcp')({
  server: {
    handlers: {
      POST: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const def = await getAgentDef(params.id)
        if (!def) return json({ error: 'not found' }, { status: 404 })
        const latest = (await listVersions(def.id))[0]
        if (!latest) return json({ error: 'no base version — import first' }, { status: 400 })

        try {
          const config = applyMcpEdits(latest.config, def.slug, parsed.data)
          const added = parsed.data.add.map((a) => a.name).join(', ')
          const removed = parsed.data.remove.join(', ')
          const { version, created } = await addVersionIfChanged(def.id, {
            soul: latest.soul,
            config,
            note: `mcp: ${[added && `+${added}`, removed && `-${removed}`].filter(Boolean).join(' ') || 'no-op'}`,
            createdBy: user.email ?? user.name ?? 'admin',
          })
          let applied = false
          if (created && parsed.data.apply && def.managed) {
            await renderFleet()
            await fleetRestart(def.department)
            applied = true
          }
          return json({ ok: true, version, created, applied })
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
