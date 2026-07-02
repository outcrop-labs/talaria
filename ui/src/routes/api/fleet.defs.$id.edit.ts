import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { addVersionIfChanged, applyConfigEdits, getAgentDef, listVersions } from '@/server/agent-defs'
import { fleetRestart } from '@/server/fleet-docker'
import { renderFleet } from '@/server/fleet-render'

const Target = z.object({
  endpoint: z.string().min(1).max(100),
  model: z.string().min(1).max(200),
  contextLength: z.number().int().positive().optional(),
})

const Body = z.object({
  soul: z.string().max(200_000),
  main: Target,
  aliases: z.array(Target.extend({ name: z.string().min(1).max(60) })).max(20),
  fallbacks: z.array(Target).max(10),
  note: z.string().max(300).optional(),
  /** Re-render + restart the managed container so the edit takes effect now. */
  apply: z.boolean().optional(),
})

// POST → save an edit as a NEW immutable version (and optionally apply it to
// the running managed container). Admin. This is "versioned agent internals":
// nothing shifts silently — every change is a version you can diff and revert.
export const Route = createFileRoute('/api/fleet/defs/$id/edit')({
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
          const config = await applyConfigEdits(latest.config, {
            main: parsed.data.main,
            aliases: parsed.data.aliases,
            fallbacks: parsed.data.fallbacks,
          })
          const { version, created } = await addVersionIfChanged(def.id, {
            soul: parsed.data.soul,
            config,
            note: parsed.data.note ?? 'edited in Talaria',
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
