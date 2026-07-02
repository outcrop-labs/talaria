// Token ledger — Talaria-owned usage accounting for every agent generation.
// Real counts when the gateway's final chunk reports usage; otherwise a
// chars/4 estimate flagged `estimated`. Dollar cost comes later (needs
// per-LLM pricing attribution — see ROADMAP).
import { db } from './db/pg'

export interface UsageInput {
  agentModel: string
  source: 'chat' | 'channel'
  refId?: string | null
  promptTokens: number
  completionTokens: number
  estimated: boolean
}

/** Rough token estimate when the gateway doesn't report usage. */
export const estimateTokens = (chars: number): number => Math.ceil(chars / 4)

export async function recordUsage(u: UsageInput): Promise<void> {
  const sql = await db()
  await sql`
    insert into usage_events (agent_model, source, ref_id, prompt_tokens, completion_tokens, estimated)
    values (${u.agentModel}, ${u.source}, ${u.refId ?? null},
            ${Math.max(0, Math.round(u.promptTokens))}, ${Math.max(0, Math.round(u.completionTokens))},
            ${u.estimated})
  `
}

export interface CostOverview {
  totals: {
    today: { prompt: number; completion: number; generations: number }
    week: { prompt: number; completion: number; generations: number }
    month: { prompt: number; completion: number; generations: number }
    estimatedShare: number // 0..1 of the month's generations that are estimates
  }
  perAgent: Array<{
    agentModel: string
    prompt: number
    completion: number
    generations: number
    lastUsed: string | null
  }>
  perDay: Array<{ day: string; prompt: number; completion: number; generations: number }>
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

  const perAgent = await sql`
    select agent_model as "agentModel",
           coalesce(sum(prompt_tokens),0)::int as prompt,
           coalesce(sum(completion_tokens),0)::int as completion,
           count(*)::int as generations,
           max(created_at) as "lastUsed"
    from usage_events where created_at > now() - interval '30 days'
    group by agent_model order by sum(prompt_tokens) + sum(completion_tokens) desc
  `
  const perDay = await sql`
    select to_char(date_trunc('day', created_at), 'YYYY-MM-DD') as day,
           coalesce(sum(prompt_tokens),0)::int as prompt,
           coalesce(sum(completion_tokens),0)::int as completion,
           count(*)::int as generations
    from usage_events where created_at > now() - interval '14 days'
    group by 1 order by 1 asc
  `

  const t = (r: unknown) => r as { prompt: number; completion: number; generations: number }
  const e = est as { est: number; all_n: number }
  return {
    totals: {
      today: t(today),
      week: t(week),
      month: t(month),
      estimatedShare: e.all_n ? e.est / e.all_n : 0,
    },
    perAgent: perAgent as unknown as CostOverview['perAgent'],
    perDay: perDay as unknown as CostOverview['perDay'],
  }
}
