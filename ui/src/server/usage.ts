// Token ledger — Talaria-owned usage accounting for every agent generation.
// Real counts when the gateway's final chunk reports usage; otherwise a
// chars/4 estimate flagged `estimated`. Dollar cost comes later (needs
// per-LLM pricing attribution — see ROADMAP).
import { db } from './db/pg'

export interface UsageInput {
  agentModel: string
  source: 'chat' | 'channel'
  refId?: string | null
  /** Alias tier the request was routed to (null = the agent's main model). */
  tier?: string | null
  promptTokens: number
  completionTokens: number
  estimated: boolean
}

/** Rough token estimate when the gateway doesn't report usage. */
export const estimateTokens = (chars: number): number => Math.ceil(chars / 4)

/** Which model class serves this generation: the requested TIER's endpoint
 *  when a tier was routed, else the agent's current MAIN endpoint. */
const classCache = new Map<string, { at: number; value: { endpointClass: string; llmModel: string } | null }>()
async function classifyAgent(
  agentModel: string,
  tier: string | null,
): Promise<{ endpointClass: string; llmModel: string } | null> {
  const cacheKey = `${agentModel}:${tier ?? ''}`
  const hit = classCache.get(cacheKey)
  if (hit && Date.now() - hit.at < 60_000) return hit.value
  const sql = await db()
  const rows = tier
    ? await sql`
        select e.class, (a->>'model') as model
        from agent_defs d
        join agent_versions v on v.agent_id = d.id and v.version = d.current_version
        cross join lateral jsonb_array_elements(coalesce(v.config->'aliases','[]'::jsonb)) a
        left join llm_endpoints e on e.name = (a->>'endpoint')
        where d.model = ${agentModel} and a->>'name' = ${tier}
      `
    : await sql`
        select e.class, (v.config->'main'->>'model') as model
        from agent_defs d
        join agent_versions v on v.agent_id = d.id and v.version = d.current_version
        left join llm_endpoints e on e.name = (v.config->'main'->>'endpoint')
        where d.model = ${agentModel}
      `
  const r = rows[0] as { class: string | null; model: string | null } | undefined
  const value = r?.class && r.model ? { endpointClass: r.class, llmModel: r.model } : null
  classCache.set(cacheKey, { at: Date.now(), value })
  return value
}

export async function recordUsage(u: UsageInput): Promise<void> {
  const sql = await db()
  const cls = await classifyAgent(u.agentModel, u.tier ?? null).catch(() => null)
  await sql`
    insert into usage_events (agent_model, source, ref_id, prompt_tokens, completion_tokens, estimated, endpoint_class, llm_model)
    values (${u.agentModel}, ${u.source}, ${u.refId ?? null},
            ${Math.max(0, Math.round(u.promptTokens))}, ${Math.max(0, Math.round(u.completionTokens))},
            ${u.estimated}, ${cls?.endpointClass ?? null}, ${cls?.llmModel ?? null})
  `
}

export interface CostOverview {
  totals: {
    today: { prompt: number; completion: number; generations: number }
    week: { prompt: number; completion: number; generations: number }
    month: { prompt: number; completion: number; generations: number }
    estimatedShare: number // 0..1 of the month's generations that are estimates
    /** 30-day local-vs-cloud token split (unattributed rows excluded). */
    split: { local: number; cloud: number }
  }
  perAgent: Array<{
    agentModel: string
    prompt: number
    completion: number
    generations: number
    lastUsed: string | null
    /** 0..1 of this agent's attributed tokens served locally. */
    localShare: number | null
  }>
  perDay: Array<{ day: string; prompt: number; completion: number; generations: number; local: number; cloud: number }>
}

export async function costOverview(): Promise<CostOverview> {
  const sql = await db()
  const window = (interval: string) =>
    sql.unsafe(
      `select coalesce(sum(prompt_tokens),0)::int as prompt,
              coalesce(sum(completion_tokens),0)::int as completion,
              count(*)::int as generations
       from usage_events where created_at > now() - interval '${interval}'`,
    )
  const [today] = await window('1 day')
  const [week] = await window('7 days')
  const [month] = await window('30 days')
  const [est] = await sql`
    select count(*) filter (where estimated)::int as est, count(*)::int as all_n
    from usage_events where created_at > now() - interval '30 days'
  `

  const [split] = await sql`
    select coalesce(sum(prompt_tokens + completion_tokens) filter (where endpoint_class = 'local'), 0)::bigint as local,
           coalesce(sum(prompt_tokens + completion_tokens) filter (where endpoint_class = 'cloud'), 0)::bigint as cloud
    from usage_events where created_at > now() - interval '30 days'
  `

  const perAgent = await sql`
    select agent_model as "agentModel",
           coalesce(sum(prompt_tokens),0)::int as prompt,
           coalesce(sum(completion_tokens),0)::int as completion,
           count(*)::int as generations,
           max(created_at) as "lastUsed",
           case when count(*) filter (where endpoint_class is not null) = 0 then null
                else sum(prompt_tokens + completion_tokens) filter (where endpoint_class = 'local')::float
                     / nullif(sum(prompt_tokens + completion_tokens) filter (where endpoint_class is not null), 0)
           end as "localShare"
    from usage_events where created_at > now() - interval '30 days'
    group by agent_model order by sum(prompt_tokens) + sum(completion_tokens) desc
  `
  const perDay = await sql`
    select to_char(date_trunc('day', created_at), 'YYYY-MM-DD') as day,
           coalesce(sum(prompt_tokens),0)::int as prompt,
           coalesce(sum(completion_tokens),0)::int as completion,
           count(*)::int as generations,
           coalesce(sum(prompt_tokens + completion_tokens) filter (where endpoint_class = 'local'), 0)::int as local,
           coalesce(sum(prompt_tokens + completion_tokens) filter (where endpoint_class = 'cloud'), 0)::int as cloud
    from usage_events where created_at > now() - interval '14 days'
    group by 1 order by 1 asc
  `

  const t = (r: unknown) => r as { prompt: number; completion: number; generations: number }
  const e = est as { est: number; all_n: number }
  const s = split as { local: string | number; cloud: string | number }
  return {
    totals: {
      today: t(today),
      week: t(week),
      month: t(month),
      estimatedShare: e.all_n ? e.est / e.all_n : 0,
      split: { local: Number(s.local), cloud: Number(s.cloud) },
    },
    perAgent: perAgent as unknown as CostOverview['perAgent'],
    perDay: perDay as unknown as CostOverview['perDay'],
  }
}
