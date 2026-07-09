import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
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
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        maybeRefreshAutoPrices() // background; persisted rates show on the next load
        void migrateEnvKeysToCipher().catch(() => {}) // one-time: seal any config-only keys into the DB
        return json({ endpoints: await listEndpoints() })
      },
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        if (user.role !== 'admin') return json({ error: 'forbidden' }, { status: 403 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          await createEndpoint({ ...parsed.data, baseUrl: parsed.data.baseUrl ?? null, apiKeyEnv: parsed.data.apiKeyEnv ?? null, apiKey: parsed.data.apiKey ?? null })
          void logAudit({ actor: user.email ?? user.name ?? 'admin', action: 'endpoint.create', targetType: 'endpoint', targetLabel: parsed.data.name, after: { provider: parsed.data.provider, class: parsed.data.class } })
          // Price the new provider's models in the background — never block an
          // interactive save on a fetch to openrouter.ai (15s worst case offline).
          void refreshAutoPrices().catch(() => {})
          return json({ ok: true })
        } catch (e) {
          return json({ error: (e as Error).message.includes('duplicate') ? 'an endpoint with that name exists' : (e as Error).message }, { status: 400 })
        }
      },
    },
  },
})
