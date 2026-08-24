import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { deleteEndpoint, listEndpoints, updateEndpoint } from '@/server/agent-defs'
import { cascadeRemoval, modelUsage, type ModelUsage } from '@/server/fleet-cascade'
import { refreshAutoPrices } from '@/server/price-oracle'
import { logAudit } from '@/server/audit'

const Patch = z.object({
  class: z.enum(['local', 'cloud']).optional(),
  priceInPerMtok: z.number().nonnegative().nullish(),
  priceOutPerMtok: z.number().nonnegative().nullish(),
  models: z.array(z.string().min(1).max(120)).max(100).optional(),
  modelPrices: z
    .record(z.string().max(120), z.object({ in: z.number().nonnegative().optional(), out: z.number().nonnegative().optional() }))
    .optional(),
  /** Admin-declared effort ladders for models whose catalog publishes none
   *  (or publishes wrong ones). Levels are the provider's own spellings,
   *  sent verbatim — the picker must never rename a level into one the model
   *  rejects, so no enum here. */
  modelEfforts: z.record(z.string().max(120), z.array(z.string().min(1).max(24)).min(1).max(12)).optional(),
  /** Extra request-body defaults for the LLM gateway (deep-merged under the
   *  client body — e.g. OpenRouter provider allowlists). Admin-only, so a
   *  permissive record is acceptable here. */
  requestDefaults: z.record(z.string().max(120), z.unknown()).optional(),
  /** Raw provider API key — sealed server-side. '' clears it; omitted leaves it. */
  apiKey: z.string().max(400).nullish(),
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
export const Route = defineApi('/api/fleet/endpoints/$id', {
  PUT: async ({ request, params }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Patch)
    if (body instanceof Response) return body

    let cascade: { changed: string[]; renderError?: string } = { changed: [] }
    if (body.models) {
      const ep = (await listEndpoints()).find((e) => e.id === params.id)
      if (!ep) return json({ error: 'not found' }, { status: 404 })
      const removed = ep.models.filter((m) => !body.models!.includes(m))
      const usage = removed.length ? await modelUsage(ep.name, removed) : []
      const mains = usage.filter((u) => u.asMain)
      if (mains.length) {
        return json(
          { error: `main model for: ${[...new Set(mains.map((m) => m.slug))].join(', ')} — reassign before removing` },
          { status: 400 },
        )
      }
      if (usage.length && !body.force) {
        return json({ needsForce: true, affected: summarize(usage) }, { status: 409 })
      }
      if (usage.length) {
        // One batched cascade: one new version per agent, one render, one restart wave.
        cascade = await cascadeRemoval(ep.name, removed, user.email ?? user.name ?? 'admin')
      }
    }
    await updateEndpoint(params.id, {
      class: body.class,
      priceInPerMtok: body.priceInPerMtok,
      priceOutPerMtok: body.priceOutPerMtok,
      models: body.models,
      modelPrices: body.modelPrices,
      modelEfforts: body.modelEfforts,
      requestDefaults: body.requestDefaults,
      // Empty string = an untouched masked field round-tripping — keep the
      // stored key. Only a non-empty value rotates it.
      apiKey: body.apiKey?.trim() ? body.apiKey : undefined,
    })
    void logAudit({
      actor: actorOf(user),
      action: 'endpoint.update',
      targetType: 'endpoint',
      targetId: params.id,
      after: { ...(body.apiKey?.trim() ? { apiKeyRotated: true } : {}), ...(body.models ? { models: body.models.length } : {}) },
    })
    // New catalog models get auto-priced in the background (never block an
    // interactive save on the external catalog fetch).
    if (body.models) void refreshAutoPrices().catch(() => {})
    return json({
      ok: true,
      cascaded: cascade.changed,
      ...(cascade.renderError
        ? { error: `agents reconfigured but re-render failed (${cascade.renderError}) — fix and re-render from /agents` }
        : {}),
    })
  },
  DELETE: async ({ request, params }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
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

    let cascade: { changed: string[]; renderError?: string } = { changed: [] }
    if (usage.length) cascade = await cascadeRemoval(ep.name, null, user.email ?? user.name ?? 'admin')
    const res = await deleteEndpoint(params.id)
    if (!res.ok) return json({ error: `still in use by: ${res.usedBy!.join(', ')}` }, { status: 400 })
    void logAudit({
      actor: actorOf(user),
      action: 'endpoint.delete',
      targetType: 'endpoint',
      targetId: params.id,
      targetLabel: ep.name,
    })
    return json({
      ok: true,
      cascaded: cascade.changed,
      ...(cascade.renderError
        ? { error: `agents reconfigured but re-render failed (${cascade.renderError}) — fix and re-render from /agents` }
        : {}),
    })
  },
})
