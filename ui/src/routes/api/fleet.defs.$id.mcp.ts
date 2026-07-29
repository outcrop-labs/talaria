import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requirePerm } from '@/server/api-guard'
import { addVersionIfChanged, getAgentDef, listVersions } from '@/server/agent-defs'
import { applyMcpEdits } from '@/server/agent-mcp'
import { rollAgent } from '@/server/fleet-reconcile'
import { logAudit } from '@/server/audit'

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
        const user = await requirePerm(request, 'agents.manage')
        if (user instanceof Response) return user
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        const def = await getAgentDef(params.id)
        if (!def) return json({ error: 'not found' }, { status: 404 })
        const latest = (await listVersions(def.id))[0]
        if (!latest) return json({ error: 'no base version — import first' }, { status: 400 })

        try {
          const config = applyMcpEdits(latest.config, def.slug, body)
          const added = body.add.map((a) => a.name).join(', ')
          const removed = body.remove.join(', ')
          const { version, created } = await addVersionIfChanged(def.id, {
            soul: latest.soul,
            config,
            note: `mcp: ${[added && `+${added}`, removed && `-${removed}`].filter(Boolean).join(' ') || 'no-op'}`,
            createdBy: user.email ?? user.name ?? 'admin',
          })
          if (created) {
            void logAudit({
              actor: actorOf(user),
              action: 'agent.mcp_edit',
              targetType: 'agent',
              targetId: def.id,
              targetLabel: def.displayName,
              after: { added: body.add.map((a) => a.name), removed: body.remove },
            })
          }
          let applied = false
          if (created && body.apply && def.managed) {
            // Roll, don't restart — see fleet.defs.$id.edit.
            const roll = await rollAgent(def.department)
            if (!roll.ok) return json({ ok: true, version, created, applied: false, warning: roll.error })
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
