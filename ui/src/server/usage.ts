// Token ledger — Talaria-owned usage accounting for every agent generation.
// Real counts when the gateway's final chunk reports usage; otherwise a
// chars/4 estimate flagged `estimated`.
import { db } from './db/pg'
import { routingFor } from './llm-gateway'
import { nudgeAutoPrices } from './price-oracle'

// ── What a token costs, by KIND ─────────────────────────────────────────────
// prompt/completion were never the whole bill:
//   • cache WRITES cost more than fresh input (Anthropic: 1.25x) — you pay a
//     premium to put the block in the cache.
//   • cache READS cost far less (Anthropic: 0.1x) — that is the whole point.
//   • reasoning tokens are OUTPUT tokens; providers already fold them into
//     completion_tokens, so they are recorded for visibility and NOT re-priced.
// Multipliers ride against the endpoint's INPUT rate, which is how every
// provider publishes them.
export const CACHE_WRITE_MULTIPLIER = 1.25
export const CACHE_READ_MULTIPLIER = 0.1

/** Every usage shape we see across providers, before normalisation. */
export interface RawUsage {
  prompt_tokens?: number | null
  completion_tokens?: number | null
  prompt_tokens_details?: { cached_tokens?: number | null; cache_creation_tokens?: number | null } | null
  completion_tokens_details?: { reasoning_tokens?: number | null } | null
  // Anthropic's NATIVE shape (some compat layers pass it straight through).
  input_tokens?: number | null
  output_tokens?: number | null
  cache_creation_input_tokens?: number | null
  cache_read_input_tokens?: number | null
}

/** Priced token counts: `promptTokens` is UNCACHED input only, so the four
 *  fields never overlap and each is billed at its own rate. */
export interface TokenCounts {
  promptTokens: number
  completionTokens: number
  cacheWriteTokens: number
  cacheReadTokens: number
  /** Informational — already inside completionTokens, never re-priced. */
  reasoningTokens: number
}

const n = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) ? Math.max(0, Math.round(v)) : 0)

/**
 * Normalise a provider usage object into non-overlapping, separately-priced
 * counts. The two shapes in the wild disagree about whether cached input is
 * INSIDE prompt_tokens:
 *
 *   Anthropic (native)  input_tokens EXCLUDES cache_creation_input_tokens and
 *                       cache_read_input_tokens — they are billed on top. Pricing
 *                       prompt_tokens alone UNDERSTATED the bill.
 *   OpenAI-compatible   prompt_tokens INCLUDES prompt_tokens_details.cached_tokens
 *                       (Anthropic's own compat layer does this too). Pricing all
 *                       of prompt_tokens at the input rate OVERSTATED the bill,
 *                       because cached reads are a tenth of the price.
 *
 * We detect the shape from the payload rather than the provider name, so a
 * gateway/proxy in the middle can't mislabel it.
 *
 * KNOWN GAP: a compat layer that folds cache WRITES into prompt_tokens without
 * reporting them separately is indistinguishable from plain input, so those
 * land at 1.0x instead of 1.25x — a bounded understatement on cache writes
 * only, and it corrects itself the moment the provider reports the field.
 */
export function normalizeUsage(u: RawUsage | null | undefined): TokenCounts | null {
  if (!u || typeof u !== 'object') return null
  let promptTokens = n(u.prompt_tokens ?? u.input_tokens)
  const completionTokens = n(u.completion_tokens ?? u.output_tokens)
  const reasoningTokens = Math.min(n(u.completion_tokens_details?.reasoning_tokens), completionTokens)

  let cacheWriteTokens = n(u.cache_creation_input_tokens ?? u.prompt_tokens_details?.cache_creation_tokens)
  let cacheReadTokens = n(u.cache_read_input_tokens)
  if (cacheWriteTokens === 0 && cacheReadTokens === 0) {
    // OpenAI-compatible: cached input is already counted inside prompt_tokens,
    // so bill it once — at the cache-read rate, not the input rate.
    cacheReadTokens = Math.min(n(u.prompt_tokens_details?.cached_tokens), promptTokens)
    promptTokens -= cacheReadTokens
  } else if (u.cache_read_input_tokens == null && u.cache_creation_input_tokens == null) {
    // Detail-object cache writes are folded in (see KNOWN GAP above).
    cacheWriteTokens = Math.min(cacheWriteTokens, promptTokens)
    promptTokens -= cacheWriteTokens
  }
  if (promptTokens + completionTokens + cacheWriteTokens + cacheReadTokens === 0) return null
  return { promptTokens, completionTokens, cacheWriteTokens, cacheReadTokens, reasoningTokens }
}

export interface UsageInput extends Partial<TokenCounts> {
  agentModel: string
  /** 'chat'/'channel' rows are gateway-metered by Talaria; 'ticket' rows are
   *  agent-SELF-REPORTED (MCP log_usage) for work done outside Talaria's
   *  request path — by design they add to the same totals (that spend is just
   *  as real), guarded by the agent key + board policy rather than metering. */
  source: 'chat' | 'channel' | 'ticket' | 'research'
  refId?: string | null
  /** Ticket this spend belongs to (agent-reported via MCP log_usage). */
  taskId?: string | null
  /** Alias tier the request was routed to (null = the agent's main model). */
  tier?: string | null
  promptTokens: number
  completionTokens: number
  estimated: boolean
}

/** Rough token estimate when the gateway doesn't report usage. */
export const estimateTokens = (chars: number): number => Math.ceil(chars / 4)

/** Which model serves this generation: the requested TIER's model when a tier
 *  was routed, else the agent's current MAIN model.
 *
 *  The agent's stored `endpoint` is a config-time PREFERENCE, not what ran:
 *  agents call the gateway by model name and `resolveRoute` round-robins the
 *  pool serving it, so trusting the config can stamp a cloud turn `local` and
 *  price it at $0. Classify from the gateway's real pool instead and record
 *  only what's certain — one server is exact, a pool that agrees on class is
 *  priced by class, a mixed pool leaves the row unattributed rather than
 *  guessing. */
interface AgentClass {
  endpointClass: string | null
  llmModel: string
  endpoint: string | null
}
const classCache = new Map<string, { at: number; value: AgentClass | null }>()
async function classifyAgent(agentModel: string, tier: string | null): Promise<AgentClass | null> {
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
  const value = r?.model ? await classifyModel(r.model, r) : null
  classCache.set(cacheKey, { at: Date.now(), value })
  return value
}

/** The serving endpoint as the GATEWAY would pick it, narrowed to what we can
 *  honestly claim. `configured` is the agent's stored spec — the only thing
 *  left to go on for a model the gateway doesn't serve (a tier still pointed
 *  at a legacy upstream). */
async function classifyModel(
  model: string,
  configured: { class: string | null; endpoint: string | null },
): Promise<AgentClass | null> {
  const { endpoints, upstreamModel } = await routingFor(model)
  const one = endpoints.length === 1 ? endpoints[0]! : null
  if (one) return { endpointClass: one.class, llmModel: upstreamModel, endpoint: one.name }
  if (endpoints.length === 0) {
    return configured.class ? { endpointClass: configured.class, llmModel: model, endpoint: configured.endpoint } : null
  }
  // A pool: the class is certain when every member agrees (local stays $0,
  // cloud stays visible as unpriced), the endpoint never is.
  const classes = new Set(endpoints.map((e) => e.class))
  return { endpointClass: classes.size === 1 ? [...classes][0]! : null, llmModel: upstreamModel, endpoint: null }
}

export async function recordUsage(u: UsageInput): Promise<void> {
  const sql = await db()
  const cls = await classifyAgent(u.agentModel, u.tier ?? null).catch(() => null)
  await sql`
    insert into usage_events (agent_model, source, ref_id, task_id, prompt_tokens, completion_tokens,
                              cache_write_tokens, cache_read_tokens, reasoning_tokens,
                              estimated, endpoint_class, llm_model, endpoint)
    values (${u.agentModel}, ${u.source}, ${u.refId ?? null}, ${u.taskId ?? null},
            ${n(u.promptTokens)}, ${n(u.completionTokens)},
            ${n(u.cacheWriteTokens)}, ${n(u.cacheReadTokens)}, ${n(u.reasoningTokens)},
            ${u.estimated}, ${cls?.endpointClass ?? null}, ${cls?.llmModel ?? null}, ${cls?.endpoint ?? null})
  `
  // A cloud row landing without a price is the oracle's cue to look again —
  // detached, throttled, and idempotent, so the hot path never feels it.
  if (cls?.endpointClass === 'cloud' && cls.llmModel && cls.endpoint) {
    void sql`
      select 1 as ok from llm_endpoints
      where name = ${cls.endpoint}
        and (model_prices ? ${cls.llmModel} or auto_prices ? ${cls.llmModel} or price_in_per_mtok is not null)
    `
      .then((rows) => {
        if (rows.length === 0) nudgeAutoPrices()
      })
      .catch(() => {})
  }
}

export interface CostOverview {
  totals: {
    today: { prompt: number; completion: number; cache: number; generations: number; cost: number }
    week: { prompt: number; completion: number; cache: number; generations: number; cost: number }
    month: { prompt: number; completion: number; cache: number; generations: number; cost: number }
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
//
// Every INPUT kind is priced off the input rate at its own multiplier: fresh
// prompt at 1x, cache writes at 1.25x, cache reads at 0.1x. Reasoning tokens
// are already inside completion_tokens, so they are never added again.
const PRICED = `
  select u.*,
    case
      when u.endpoint_class = 'local' then 0
      when u.endpoint_class = 'cloud' then
        ((u.prompt_tokens
            + u.cache_write_tokens * ${CACHE_WRITE_MULTIPLIER}
            + u.cache_read_tokens * ${CACHE_READ_MULTIPLIER})
           * coalesce((e.model_prices->u.llm_model->>'in')::numeric,
                      (e.auto_prices->u.llm_model->>'in')::numeric, e.price_in_per_mtok)
         + u.completion_tokens * coalesce((e.model_prices->u.llm_model->>'out')::numeric,
                                          (e.auto_prices->u.llm_model->>'out')::numeric, e.price_out_per_mtok)) / 1e6
      else null
    end as cost
  from usage_events u
  left join llm_endpoints e on e.name = u.endpoint
`

/** Every token on a row, whatever its kind — the denominator for volume views. */
const ALL_TOKENS = 'prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens'

export interface TaskUsage {
  promptTokens: number
  completionTokens: number
  cacheWriteTokens: number
  cacheReadTokens: number
  cost: number
  /** Tokens with no $ figure: unpriced cloud AND unattributed rows (agent name
   *  didn't resolve to a def). Cost is understated when > 0. */
  unpricedTokens: number
  perModel: Array<{ llmModel: string | null; tokens: number; cost: number | null }>
}

/** Token spend reported against one ticket (agents via MCP log_usage). */
export async function taskUsage(taskId: string): Promise<TaskUsage> {
  const sql = await db()
  const [totals] = await sql.unsafe(
    `with priced as (${PRICED})
     select coalesce(sum(prompt_tokens),0)::int as prompt,
            coalesce(sum(completion_tokens),0)::int as completion,
            coalesce(sum(cache_write_tokens),0)::int as cache_write,
            coalesce(sum(cache_read_tokens),0)::int as cache_read,
            coalesce(sum(cost),0)::float as cost,
            coalesce(sum(${ALL_TOKENS}) filter (where cost is null), 0)::bigint as unpriced
     from priced where task_id = $1`,
    [taskId],
  )
  const perModel = await sql.unsafe(
    `with priced as (${PRICED})
     select llm_model as "llmModel",
            coalesce(sum(${ALL_TOKENS}),0)::bigint as tokens,
            sum(cost)::float as cost
     from priced where task_id = $1
     group by llm_model order by tokens desc`,
    [taskId],
  )
  const t = totals as unknown as {
    prompt: number
    completion: number
    cache_write: number
    cache_read: number
    cost: number
    unpriced: string | number
  }
  return {
    promptTokens: t.prompt,
    completionTokens: t.completion,
    cacheWriteTokens: t.cache_write,
    cacheReadTokens: t.cache_read,
    cost: Number(t.cost),
    unpricedTokens: Number(t.unpriced),
    perModel: (perModel as unknown as TaskUsage['perModel']).map((m) => ({
      ...m,
      tokens: Number(m.tokens),
      cost: m.cost === null ? null : Number(m.cost),
    })),
  }
}

export async function costOverview(): Promise<CostOverview> {
  const sql = await db()
  const window = (interval: string) =>
    sql.unsafe(
      `with priced as (${PRICED})
       select coalesce(sum(prompt_tokens),0)::int as prompt,
              coalesce(sum(completion_tokens),0)::int as completion,
              coalesce(sum(cache_write_tokens + cache_read_tokens),0)::int as cache,
              count(*)::int as generations,
              coalesce(sum(cost), 0)::float as cost
       from priced where created_at > now() - interval '${interval}'`,
    )
  // Independent aggregates — fan out concurrently (alerts + /cost call this on
  // every load; nine sequential round-trips added up).
  const [[today], [week], [month], [est], [split], [unpriced], perModel, perAgent, perDay] = await Promise.all([
    window('1 day'),
    window('7 days'),
    window('30 days'),
    sql`
      select count(*) filter (where estimated)::int as est, count(*)::int as all_n
      from usage_events where created_at > now() - interval '30 days'
    `,
    // NOTE: the token sum is INLINED here — a tagged sql template interpolates
    // a string as a bind parameter, and sum($1) is sum(unknown) to Postgres.
    // ALL_TOKENS is only for the sql.unsafe strings.
    sql`
      select coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class = 'local'), 0)::bigint as local,
             coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class = 'cloud'), 0)::bigint as cloud,
             coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class is null), 0)::bigint as other
      from usage_events where created_at > now() - interval '30 days'
    `,
    // Cloud tokens we can't price (no per-model or endpoint rate) — shown, not $0.
    sql.unsafe(
      `with priced as (${PRICED})
       select coalesce(sum(${ALL_TOKENS}), 0)::bigint as tokens
       from priced where created_at > now() - interval '30 days'
         and endpoint_class = 'cloud' and cost is null`,
    ),
    sql.unsafe(
      `with priced as (${PRICED})
       select llm_model as "llmModel", endpoint_class as "endpointClass",
              coalesce(sum(${ALL_TOKENS}), 0)::bigint as tokens,
              sum(cost)::float as cost
       from priced where created_at > now() - interval '30 days'
       group by llm_model, endpoint_class
       order by (endpoint_class = 'local') desc nulls last, tokens desc`,
    ),
    sql.unsafe(
      `with priced as (${PRICED})
       select agent_model as "agentModel",
              coalesce(sum(prompt_tokens),0)::int as prompt,
              coalesce(sum(completion_tokens),0)::int as completion,
              count(*)::int as generations,
              max(created_at) as "lastUsed",
              coalesce(sum(cost), 0)::float as cost,
              case when count(*) filter (where endpoint_class is not null) = 0 then null
                   else sum(${ALL_TOKENS}) filter (where endpoint_class = 'local')::float
                        / nullif(sum(${ALL_TOKENS}) filter (where endpoint_class is not null), 0)
              end as "localShare"
       from priced where created_at > now() - interval '30 days'
       group by agent_model order by sum(${ALL_TOKENS}) desc`,
    ),
    sql`
      select to_char(date_trunc('day', created_at), 'YYYY-MM-DD') as day,
             coalesce(sum(prompt_tokens),0)::int as prompt,
             coalesce(sum(completion_tokens),0)::int as completion,
             count(*)::int as generations,
             coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class = 'local'), 0)::int as local,
             coalesce(sum(prompt_tokens + completion_tokens + cache_write_tokens + cache_read_tokens) filter (where endpoint_class = 'cloud'), 0)::int as cloud
      from usage_events where created_at > now() - interval '14 days'
      group by 1 order by 1 asc
    `,
  ])

  const t = (r: unknown) => {
    const x = r as { prompt: number; completion: number; cache: number; generations: number; cost: number | null }
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

// ── Rolling-window spend (the budget check's read side) ─────────────────────

export interface SpendWindow {
  tokens: number
  /** Priced spend in USD. Unpriced cloud rows contribute 0, so a $ ceiling is
   *  never tripped by a model whose rate isn't configured — `unpricedTokens`
   *  says how much of the window that covers, and a token ceiling bounds it. */
  cost: number
  unpricedTokens: number
}

/**
 * Billable spend over a rolling window, optionally for one agent/caller
 * (`agent_model` — a fleet model id for persona rows, `api:<key>` for gateway
 * rows). Called before every budgeted gateway call; llm-gateway caches it.
 */
export async function spendSince(windowHours: number, agentModel?: string | null): Promise<SpendWindow> {
  const hours = Math.max(1, Math.min(24 * 365, Math.round(windowHours)))
  const sql = await db()
  const rows = await sql.unsafe(
    `with priced as (${PRICED})
     select coalesce(sum(${ALL_TOKENS}), 0)::bigint as tokens,
            coalesce(sum(cost), 0)::float as cost,
            coalesce(sum(${ALL_TOKENS}) filter (where cost is null), 0)::bigint as unpriced
     from priced
     where created_at > now() - interval '${hours} hours'
       and ($1::text is null or agent_model = $1)`,
    [agentModel ?? null],
  )
  const r = rows[0] as unknown as { tokens: string | number; cost: number; unpriced: string | number } | undefined
  if (!r) return { tokens: 0, cost: 0, unpricedTokens: 0 }
  return { tokens: Number(r.tokens), cost: Number(r.cost), unpricedTokens: Number(r.unpriced) }
}
