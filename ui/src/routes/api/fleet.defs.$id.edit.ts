import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { hasPerm } from '@/server/permissions'
import { addEndpointModels, addVersionIfChanged, applyConfigEdits, getAgentDef, listEndpoints, listVersions } from '@/server/agent-defs'
import { availableModels } from '@/server/provider-catalog'
import { rollAgent } from '@/server/fleet-reconcile'

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
        if (!(await hasPerm(user, 'agents.manage'))) return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const def = await getAgentDef(params.id)
        if (!def) return json({ error: 'not found' }, { status: 404 })

        const latest = (await listVersions(def.id))[0]
        if (!latest) return json({ error: 'no base version — import first' }, { status: 400 })

        // A picked model must actually route: auto-register it on its endpoint
        // when the provider's LIVE catalog serves it (no maintained lists — the
        // registration follows the choice), refuse clearly when it doesn't.
        // Without this, an unregistered model renders fine and the agent then
        // dies with a gateway 404 on its first turn — a silent-freeze chat.
        const endpoints = new Map((await listEndpoints()).map((e) => [e.name, e]))
        const targets = [parsed.data.main, ...parsed.data.aliases, ...parsed.data.fallbacks]
        for (const t of targets) {
          const ep = endpoints.get(t.endpoint)
          if (!ep) return json({ error: `endpoint "${t.endpoint}" does not exist` }, { status: 400 })
          if (ep.models.includes(t.model)) continue
          const live = await availableModels(ep).catch(() => null)
          if (live?.includes(t.model)) {
            await addEndpointModels(ep.name, [t.model])
            ep.models.push(t.model)
          } else {
            return json(
              { error: `"${t.model}" is not registered on "${t.endpoint}"${live ? ' and its live catalog does not list it' : ''} — pick it on /models first` },
              { status: 400 },
            )
          }
        }

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
            // Roll, don't restart: the new config comes up beside the old
            // container and traffic cuts over only after health — applying an
            // edit never interrupts conversations in flight.
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
