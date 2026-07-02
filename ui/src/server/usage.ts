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

/** Which model serves this generation: the requested TIER's endpoint when a
 *  tier was routed, else the agent's current MAIN endpoint. */
const classCache = new Map<
  string,
  { at: number; value: { endpointClass: string; llmModel: string; endpoint: string | null } | null }
>()
async function classifyAgent(
  agentModel: string,
  tier: string | null,
): Promise<{ endpointClass: string; llmModel: string; endpoint: string | null } | null> {
  const cacheKey = `${agentModel}:${tier ?? ''}`
  const hit = classCache.get(cacheKey)
  if (hit && Date.now() - hit.at < 60_000) return hit.value
  const sql = await db()
  const rows = tier
    ? await sql`
        select e.class, (a->>'model') as model, (a->>'endpoint') as endpoint
        from agent_defs d
        join agent_versions v on v.agent_id = d.id and v.version = d.current_version
        cross join lateral jsonb_array_elements(coalesce(v.config->'aliases','[]'::jsonb)) a
        left join llm_endpoints e on e.name = (a->>'endpoint')
        where d.model = ${agentModel} and a->>'name' = ${tier}
      `
    : await sql`
        select e.class, (v.config->'main'->>'model') as model, (v.config->'main'->>'endpoint') as endpoint
        from agent_defs d
        join agent_versions v on v.agent_id = d.id and v.version = d.current_version
        left join llm_endpoints e on e.name = (v.config->'main'->>'endpoint')
        where d.model = ${agentModel}
      `
  const r = rows[0] as { class: string | null; model: string | null; endpoint: string | null } | undefined
  const value = r?.class && r.model ? { endpointClass: r.class, llmModel: r.model, endpoint: r.endpoint } : null
  classCache.set(cacheKey, { at: Date.now(), value })
  return value
}

export async function recordUsage(u: UsageInput): Promise<void> {
  const sql = await db()
  const cls = await classifyAgent(u.agentModel, u.tier ?? null).catch(() => null)
  await sql`
    insert into usage_events (agent_model, source, ref_id, prompt_tokens, completion_tokens, estimated, endpoint_class, llm_model, endpoint)
    values (${u.agentModel}, ${u.source}, ${u.refId ?? null},
            ${Math.max(0, Math.round(u.promptTokens))}, ${Math.max(0, Math.round(u.completionTokens))},
            ${u.estimated}, ${cls?.endpointClass ?? null}, ${cls?.llmModel ?? null}, ${cls?.endpoint ?? null})
  `
}

export interface CostOverview {
  totals: {
    today: { prompt: number; completion: number; generations: number; cost: number }
    week: { prompt: number; completion: number; generations: number; cost: number }
    month: { prompt: number; completion: number; generations: number; cost: number }
    estimatedShare: number // 0..1 of the month's generations that are estimates
    /** 30-day local-vs-cloud token split; `other` = unattributed rows, so the
     *  three always sum to the 30-day total. */
    split: { local: number; cloud: number; other: number }
    /** 30-day cloud tokens with no price configured — shown, never silent $0. */
    unpricedCloudTokens: number
  }
  /** 30-day tokens + $ per serving model (class + model), largest first. */
  perModel: Array<{
    llmModel: string | null
    endpointClass: 'local' | 'cloud' | null
    tokens: number
    cost: number | null
  }>
  perAgent: Array<{
    agentModel: string
    prompt: number
    completion: number
    generations: number
    lastUsed: string | null
    cost: number
    /** 0..1 of this agent's attributed tokens served locally. */
    localShare: number | null
  }>
  perDay: Array<{ day: string; prompt: number; completion: number; generations: number; local: number; cloud: number }>
}

// The priced view: cloud rows get $ from the user's per-model override, else
// the auto-fetched public rate (price-oracle), else the endpoint default;
// local rows are $0 (your hardware); cloud rows with no price at all get NULL
// cost so they can be surfaced as "unpriced".
const PRICED = `
  select u.*,
    case
      when u.endpoint_class = 'local' then 0
      when u.endpoint_class = 'cloud' then
        (u.prompt_tokens * coalesce((e.model_prices->u.llm_model->>'in')::numeric,
                                    (e.auto_prices->u.llm_model->>'in')::numeric, e.price_in_per_mtok)
         + u.completion_tokens * coalesce((e.model_prices->u.llm_model->>'out')::numeric,
                                          (e.auto_prices->u.llm_model->>'out')::numeric, e.price_out_per_mtok)) / 1e6
      else null
    end as cost
  from usage_events u
  left join llm_endpoints e on e.name = u.endpoint
`

export async function costOverview(): Promise<CostOverview> {
  const sql = await db()
  const window = (interval: string) =>
    sql.unsafe(
      `with priced as (${PRICED})
       select coalesce(sum(prompt_tokens),0)::int as prompt,
              coalesce(sum(completion_tokens),0)::int as completion,
              count(*)::int as generations,
              coalesce(sum(cost), 0)::float as cost
       from priced where created_at > now() - interval '${interval}'`,
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
           coalesce(sum(prompt_tokens + completion_tokens) filter (where endpoint_class = 'cloud'), 0)::bigint as cloud,
           coalesce(sum(prompt_tokens + completion_tokens) filter (where endpoint_class is null), 0)::bigint as other
    from usage_events where created_at > now() - interval '30 days'
  `
  // Cloud tokens we can't price (no per-model or endpoint rate) — shown, not $0.
  const [unpriced] = await sql.unsafe(
    `with priced as (${PRICED})
     select coalesce(sum(prompt_tokens + completion_tokens), 0)::bigint as tokens
     from priced where created_at > now() - interval '30 days'
       and endpoint_class = 'cloud' and cost is null`,
  )
  const perModel = await sql.unsafe(
    `with priced as (${PRICED})
     select llm_model as "llmModel", endpoint_class as "endpointClass",
            coalesce(sum(prompt_tokens + completion_tokens), 0)::bigint as tokens,
            sum(cost)::float as cost
     from priced where created_at > now() - interval '30 days'
     group by llm_model, endpoint_class
     order by (endpoint_class = 'local') desc nulls last, tokens desc`,
  )

  const perAgent = await sql.unsafe(
    `with priced as (${PRICED})
     select agent_model as "agentModel",
            coalesce(sum(prompt_tokens),0)::int as prompt,
            coalesce(sum(completion_tokens),0)::int as completion,
            count(*)::int as generations,
            max(created_at) as "lastUsed",
            coalesce(sum(cost), 0)::float as cost,
            case when count(*) filter (where endpoint_class is not null) = 0 then null
                 else sum(prompt_tokens + completion_tokens) filter (where endpoint_class = 'local')::float
                      / nullif(sum(prompt_tokens + completion_tokens) filter (where endpoint_class is not null), 0)
            end as "localShare"
     from priced where created_at > now() - interval '30 days'
     group by agent_model order by sum(prompt_tokens) + sum(completion_tokens) desc`,
  )
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

  const t = (r: unknown) => {
    const x = r as { prompt: number; completion: number; generations: number; cost: number | null }
    return { ...x, cost: Number(x.cost ?? 0) }
  }
  const e = est as { est: number; all_n: number }
  const s = split as { local: string | number; cloud: string | number; other: string | number }
  return {
    totals: {
      today: t(today),
      week: t(week),
      month: t(month),
      estimatedShare: e.all_n ? e.est / e.all_n : 0,
      split: { local: Number(s.local), cloud: Number(s.cloud), other: Number(s.other) },
      unpricedCloudTokens: Number((unpriced as unknown as { tokens: string | number }).tokens),
    },
    perModel: (perModel as unknown as CostOverview['perModel']).map((m) => ({
      ...m,
      tokens: Number(m.tokens),
      cost: m.cost === null ? null : Number(m.cost),
    })),
    perAgent: (perAgent as unknown as CostOverview['perAgent']).map((a) => ({ ...a, cost: Number(a.cost ?? 0) })),
    perDay: perDay as unknown as CostOverview['perDay'],
  }
}
