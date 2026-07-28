import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { db } from '@/server/db/pg'
import { listEndpoints } from '@/server/agent-defs'
import { availableModels } from '@/server/provider-catalog'
import { gatewayPulse } from '@/server/llm-gateway'
import { containerStatus } from '@/server/fleet-docker'

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
        // ── Live pulse: what's generating right now + the last hour ─────────
        // Streaming rows persist throttled during a reply; a crashed stream
        // stays 'streaming' forever, hence the 10-minute recency clamp.
        const generating = (await sql`
          select c.agent_model as "agentModel", count(*)::int as count
          from messages m join conversations c on c.id = m.conversation_id
          where m.status = 'streaming' and m.created_at > now() - interval '10 minutes'
          group by 1
          union all
          select cm.author as "agentModel", count(*)::int as count
          from channel_messages cm
          where cm.status = 'streaming' and cm.author_type = 'agent'
            and cm.created_at > now() - interval '10 minutes'
          group by 1
        `) as unknown as Array<{ agentModel: string; count: number }>

        const lastHour = (await sql`
          select agent_model as "agentModel", count(*)::int as generations,
                 coalesce(sum(prompt_tokens + completion_tokens), 0)::bigint as tokens,
                 max(created_at) as "lastAt"
          from usage_events
          where created_at > now() - interval '1 hour'
          group by 1 order by tokens desc limit 20
        `) as unknown as Array<{ agentModel: string; generations: number; tokens: string | number; lastAt: string }>

        // Fleet container temperature: running / warming / unhealthy / down.
        const managed = (await sql`
          select department from agent_defs where enabled and managed
        `) as unknown as Array<{ department: string }>
        const states = managed.length ? await containerStatus(managed.map((m) => m.department)).catch(() => []) : []
        const fleet = { running: 0, warming: 0, unhealthy: 0, down: 0 }
        for (const s of states) {
          const c = s.managed
          if (!c || c.state !== 'running') fleet.down++
          else if (c.health === 'starting') fleet.warming++
          else if (c.health === 'unhealthy') fleet.unhealthy++
          else fleet.running++
        }

        const t = totals as { today: string | number; month: string | number; generations: number }
        return json({
          live: {
            generating,
            lastHour: lastHour.map((r) => ({ ...r, tokens: Number(r.tokens) })),
            gateway: gatewayPulse(),
            fleet,
          },
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
