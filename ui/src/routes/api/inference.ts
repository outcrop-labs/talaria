import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { db } from '@/server/db/pg'
import { listEndpoints } from '@/server/agent-defs'
import { availableModels } from '@/server/provider-catalog'

export interface LocalBackend {
  id: string
  name: string
  baseUrl: string | null
  models: string[]
  health: { ok: boolean; latencyMs: number | null; servingNow: string[]; note: string | null }
}

// Local inference: your own hardware's backends (class=local), probed live,
// plus what they've served from the token ledger. Config lives on /models.
export const Route = createFileRoute('/api/inference')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })

        const locals = (await listEndpoints()).filter((e) => e.class === 'local')
        const backends: LocalBackend[] = await Promise.all(
          locals.map(async (ep) => {
            const started = Date.now()
            try {
              const serving = await availableModels(ep)
              return {
                id: ep.id,
                name: ep.name,
                baseUrl: ep.baseUrl,
                models: ep.models,
                health: { ok: true, latencyMs: Date.now() - started, servingNow: serving, note: null },
              }
            } catch (e) {
              return {
                id: ep.id,
                name: ep.name,
                baseUrl: ep.baseUrl,
                models: ep.models,
                health: { ok: false, latencyMs: null, servingNow: [], note: (e as Error).message },
              }
            }
          }),
        )

        const sql = await db()
        const [totals] = await sql`
          select coalesce(sum(prompt_tokens + completion_tokens) filter (where created_at > now() - interval '1 day'), 0)::bigint as today,
                 coalesce(sum(prompt_tokens + completion_tokens), 0)::bigint as month,
                 count(*)::int as generations
          from usage_events
          where endpoint_class = 'local' and created_at > now() - interval '30 days'
        `
        const perModel = await sql`
          select llm_model as "llmModel", coalesce(sum(prompt_tokens + completion_tokens), 0)::bigint as tokens
          from usage_events
          where endpoint_class = 'local' and created_at > now() - interval '30 days'
          group by llm_model order by tokens desc
        `
        const t = totals as { today: string | number; month: string | number; generations: number }
        return json({
          backends,
          usage: {
            today: Number(t.today),
            month: Number(t.month),
            generations: t.generations,
            perModel: (perModel as unknown as Array<{ llmModel: string | null; tokens: string | number }>).map((m) => ({
              llmModel: m.llmModel,
              tokens: Number(m.tokens),
            })),
          },
        })
      },
    },
  },
})
