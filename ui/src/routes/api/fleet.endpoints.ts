import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { createEndpoint, listEndpoints } from '@/server/agent-defs'
import { maybeRefreshAutoPrices, refreshAutoPrices } from '@/server/price-oracle'
import { migrateEnvKeysToCipher } from '@/server/provider-catalog'
import { logAudit } from '@/server/audit'

const Body = z.object({
  name: z.string().min(2).max(60),
  provider: z.string().min(2).max(40),
  baseUrl: z.string().url().max(300).nullish(),
  class: z.enum(['local', 'cloud']),
  // Provider-key-shaped names only (see provider-catalog KEY_ENV_RE) — the
  // catalog fetch sends this var's VALUE to the endpoint's base URL.
  apiKeyEnv: z
    .string()
    .regex(/^(LLM_API_KEY|[A-Z][A-Z0-9_]*_API_KEY)$/)
    .max(80)
    .nullish(),
  // Raw provider API key — sealed (secretbox) server-side, never stored or
  // returned in the clear.
  apiKey: z.string().max(400).nullish(),
  models: z.array(z.string().min(1).max(120)).max(100).optional(),
  modelPrices: z
    .record(z.string().max(120), z.object({ in: z.number().nonnegative().optional(), out: z.number().nonnegative().optional() }))
    .optional(),
})

// The model-backend registry (Models tab). GET → all endpoints. POST → add one.
export const Route = createFileRoute('/api/fleet/endpoints')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        maybeRefreshAutoPrices() // background; persisted rates show on the next load
        void migrateEnvKeysToCipher().catch(() => {}) // one-time: seal any config-only keys into the DB
        return json({ endpoints: await listEndpoints() })
      },
      POST: async ({ request }) => {
        const user = await requireAdmin(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        try {
          const id = await createEndpoint({ ...body, baseUrl: body.baseUrl ?? null, apiKeyEnv: body.apiKeyEnv ?? null, apiKey: body.apiKey ?? null })
          void logAudit({ actor: actorOf(user), action: 'endpoint.create', targetType: 'endpoint', targetLabel: body.name, after: { provider: body.provider, class: body.class } })
          // Price the new provider's models in the background — never block an
          // interactive save on a fetch to openrouter.ai (15s worst case offline).
          void refreshAutoPrices().catch(() => {})
          return json({ ok: true, id })
        } catch (e) {
          return json({ error: (e as Error).message.includes('duplicate') ? 'an endpoint with that name exists' : (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
