import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { deleteEndpoint, listEndpoints, updateEndpoint } from '@/server/agent-defs'
import { cascadeRemoval, modelUsage, type ModelUsage } from '@/server/fleet-cascade'

const Patch = z.object({
  class: z.enum(['local', 'cloud']).optional(),
  priceInPerMtok: z.number().nonnegative().nullish(),
  priceOutPerMtok: z.number().nonnegative().nullish(),
  models: z.array(z.string().min(1).max(120)).max(100).optional(),
  /** Second step of the double opt-in: cascade the removal into agent configs. */
  force: z.boolean().optional(),
})

const summarize = (usage: ModelUsage[]) =>
  usage.map((u) => ({ slug: u.slug, asMain: u.asMain, aliases: u.aliases, fallbacks: u.fallbacks }))

// PUT → edit an endpoint (class, pricing, model catalog). Removing catalog
// models that agents use returns 409 with the blast radius; retry with
// force:true to cascade (agents get new versions with the tier stripped).
// DELETE → remove the endpoint, same double-opt-in flow (?force=1).
// An agent's MAIN model is never cascaded — reassign it first.
export const Route = createFileRoute('/api/fleet/endpoints/$id')({
  server: {
    handlers: {
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Patch.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })

        let cascaded: string[] = []
        if (parsed.data.models) {
          const ep = (await listEndpoints()).find((e) => e.id === params.id)
          if (!ep) return json({ error: 'not found' }, { status: 404 })
          const removed = ep.models.filter((m) => !parsed.data.models!.includes(m))
          const usage = (await Promise.all(removed.map((m) => modelUsage(ep.name, m)))).flat()
          const mains = usage.filter((u) => u.asMain)
          if (mains.length) {
            return json(
              { error: `main model for: ${[...new Set(mains.map((m) => m.slug))].join(', ')} — reassign before removing` },
              { status: 400 },
            )
          }
          if (usage.length && !parsed.data.force) {
            return json({ needsForce: true, affected: summarize(usage) }, { status: 409 })
          }
          if (usage.length) {
            const actor = user.email ?? user.name ?? 'admin'
            for (const m of removed) cascaded.push(...(await cascadeRemoval(ep.name, m, actor)))
          }
        }
        await updateEndpoint(params.id, {
          class: parsed.data.class,
          priceInPerMtok: parsed.data.priceInPerMtok,
          priceOutPerMtok: parsed.data.priceOutPerMtok,
          models: parsed.data.models,
        })
        return json({ ok: true, cascaded: [...new Set(cascaded)] })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const force = new URL(request.url).searchParams.get('force') === '1'
        const ep = (await listEndpoints()).find((e) => e.id === params.id)
        if (!ep) return json({ ok: true })

        const usage = await modelUsage(ep.name, null)
        const mains = usage.filter((u) => u.asMain)
        if (mains.length) {
          return json(
            { error: `main model for: ${mains.map((m) => m.slug).join(', ')} — reassign before deleting` },
            { status: 400 },
          )
        }
        if (usage.length && !force) return json({ needsForce: true, affected: summarize(usage) }, { status: 409 })

        let cascaded: string[] = []
        if (usage.length) cascaded = await cascadeRemoval(ep.name, null, user.email ?? user.name ?? 'admin')
        const res = await deleteEndpoint(params.id)
        if (!res.ok) return json({ error: `still in use by: ${res.usedBy!.join(', ')}` }, { status: 400 })
        return json({ ok: true, cascaded })
      },
    },
  },
})
