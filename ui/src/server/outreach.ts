// Proactive outreach (#59) — agents notice things and reach out instead of
// waiting to be asked. Two halves:
//
//   • message_user — the one genuinely NEW write path: an agent starts (or
//     continues) a real chat conversation with a human, with an inbox
//     notification. Personal assistants can only reach their owner; every
//     agent↔user pair is rate-capped per day.
//   • the sweep — a throttled, opportunistic pass (same pattern as
//     maybeRagSweep / maybeSweepIdleChats) that gives each opted-in agent a
//     periodic "check-in turn" through its OWN persona gateway. The agent
//     acts through its normal MCP tools (comment / post_to_channel /
//     message_user), so everything stays attributed, board-policy-gated, and
//     guard-visible; Talaria only delivers the nudge + signals.
//
// Everything is opt-in twice over: a master switch (off by default) AND a
// per-agent `proactive` flag. outreach_events is the memory: it powers the
// daily caps, the "don't repeat yourself" context, and admin visibility.
import { db } from './db/pg'
import { getSetting, setSetting } from './audit'
import {
  createConversation,
  insertStreamingAssistant,
  nextSeq,
  touchConversation,
  updateAssistant,
} from './conversations'
import { addNotification } from './notifications'
import { describeAgent, proxyChat } from './gateway'
import { parseAgentStream } from '@/lib/sse-parse'
import { registerJob } from './scheduler'
import { estimateTokens, recordUsage } from './usage'

export interface OutreachConfig {
  /** Master switch for the periodic sweep. message_user works regardless —
   *  it's a governed tool, not a behavior. */
  enabled: boolean
  /** Minimum minutes between check-in turns for any one agent. */
  intervalMinutes: number
  /** Agent-initiated DMs allowed per agent↔user pair per day. */
  dailyDmCap: number
}

const DEFAULT_CONFIG: OutreachConfig = { enabled: false, intervalMinutes: 240, dailyDmCap: 3 }

export const getOutreachConfig = async (): Promise<OutreachConfig> => {
  const c = await getSetting<Partial<OutreachConfig>>('outreach_config', DEFAULT_CONFIG)
  return { ...DEFAULT_CONFIG, ...c }
}
export const setOutreachConfig = (c: OutreachConfig) => setSetting('outreach_config', c)

async function logEvent(
  agentModel: string,
  kind: 'sweep' | 'dm',
  data: { userId?: string; conversationId?: string; note?: string } = {},
): Promise<void> {
  const sql = await db()
  await sql`
    insert into outreach_events (agent_model, kind, user_id, conversation_id, note)
    values (${agentModel}, ${kind}, ${data.userId ?? null}, ${data.conversationId ?? null}, ${data.note?.slice(0, 500) ?? null})
  `
}

// ── message_user ─────────────────────────────────────────────────────────────

export interface MessageUserResult {
  ok: boolean
  conversationId?: string
  /** Plain-language line for the agent to act on (limits, bad target, …). */
  error?: string
}

/** An agent starts (or continues) a direct conversation with a human. The
 *  message lands as a normal assistant turn in the pair's most recent live
 *  chat conversation (or a fresh one), plus an inbox notification deep-linked
 *  to it. Personal assistants are owner-only; every pair is day-capped. */
export async function agentMessageUser(agentModel: string, to: string, message: string): Promise<MessageUserResult> {
  const sql = await db()
  const cfg = await getOutreachConfig()

  const agents = (await sql`
    select model, display_name as "displayName", owner_user_id as "ownerUserId"
    from agent_defs where model = ${agentModel} and enabled
  `) as unknown as Array<{ model: string; displayName: string; ownerUserId: string | null }>
  const agent = agents[0]
  if (!agent) return { ok: false, error: 'unknown agent' }

  // Resolve the target by email (exact) or name (exact, case-insensitive).
  const users = (await sql`
    select id, email, name from users
    where lower(email) = lower(${to}) or lower(coalesce(name, '')) = lower(${to})
    limit 2
  `) as unknown as Array<{ id: string; email: string | null; name: string | null }>
  if (users.length !== 1) {
    return { ok: false, error: users.length ? `"${to}" matches more than one person — use their email` : `no teammate matches "${to}" — use their email address` }
  }
  const target = users[0]!

  // A personal assistant belongs to one human; it never contacts anyone else.
  if (agent.ownerUserId && agent.ownerUserId !== target.id) {
    return { ok: false, error: 'as a personal assistant you can only message your owner' }
  }

  const counts = (await sql`
    select count(*)::int as count from outreach_events
    where agent_model = ${agentModel} and kind = 'dm' and user_id = ${target.id}
      and created_at > now() - interval '1 day'
  `) as unknown as Array<{ count: number }>
  if ((counts[0]?.count ?? 0) >= cfg.dailyDmCap) {
    return { ok: false, error: `daily limit reached for messaging this person (${cfg.dailyDmCap}/day) — try again tomorrow or use a ticket comment` }
  }

  // Most recent live chat between this pair, else a fresh conversation.
  const existing = (await sql`
    select id from conversations
    where user_id = ${target.id} and agent_model = ${agentModel} and kind = 'chat' and archived = false
    order by updated_at desc limit 1
  `) as unknown as Array<{ id: string }>
  const label = agent.displayName || describeAgent(agentModel).label
  const convId = existing[0]?.id ?? (await createConversation(target.id, agentModel, `${label} reached out`))

  const seq = await nextSeq(convId)
  const msgId = await insertStreamingAssistant(convId, seq)
  await updateAssistant(msgId, { content: message, reasoning: '', tools: [], status: 'complete' })
  await touchConversation(convId)

  await addNotification(target.id, {
    kind: 'agent-outreach',
    title: `${label} reached out`,
    body: message.length > 140 ? `${message.slice(0, 140)}…` : message,
    href: `/comms?a=${encodeURIComponent(agentModel)}&x=${convId}`,
  }).catch(() => {})

  await logEvent(agentModel, 'dm', { userId: target.id, conversationId: convId, note: message })
  return { ok: true, conversationId: convId }
}

// ── the sweep ────────────────────────────────────────────────────────────────

const SWEEP_TICK_MS = 5 * 60_000 // how often a sweep is attempted

// THE DEPLOY-DAY BOUND. "Due" is computed from outreach_events, so an agent
// that has never been swept is due — and until now the sweep only ran when
// traffic happened to kick it. The first instance to run this on a real
// schedule therefore finds EVERY proactive agent due at once, and each due
// agent gets a check-in turn that may DM a human. Capping the pass turns that
// wall into a queue: at most this many agents per pass, one pass per 5
// minutes, so a fleet of thirty proactive agents drains over ~50 minutes
// instead of messaging everyone in one burst. Agents keep their place — the
// query orders by slug and the uncapped remainder is simply still due next
// pass. (The master switch is off by default, so this only bites deployments
// that have deliberately turned outreach on.)
const SWEEP_MAX_AGENTS = 3

interface SweepAgent {
  model: string
  displayName: string
  ownerUserId: string | null
}

/** One check-in turn per due agent, at most SWEEP_MAX_AGENTS per pass. "Due" =
 *  proactive + enabled + no sweep event within the configured interval. Serial
 *  on purpose — one agent turn at a time keeps the load invisible. */
export async function sweepOutreach(): Promise<{ turns: number; failed: number; deferred: number }> {
  const cfg = await getOutreachConfig()
  if (!cfg.enabled) return { turns: 0, failed: 0, deferred: 0 }
  const sql = await db()
  const due = (await sql`
    select d.model, d.display_name as "displayName", d.owner_user_id as "ownerUserId"
    from agent_defs d
    where d.enabled and d.proactive
      and not exists (
        select 1 from outreach_events e
        where e.agent_model = d.model and e.kind = 'sweep'
          and e.created_at > now() - (${cfg.intervalMinutes} || ' minutes')::interval
      )
    order by d.slug
    limit ${SWEEP_MAX_AGENTS + 1}
  `) as unknown as SweepAgent[]
  // One extra row is fetched purely to report the backlog honestly.
  const deferred = Math.max(0, due.length - SWEEP_MAX_AGENTS)

  let turns = 0
  let failed = 0
  for (const agent of due.slice(0, SWEEP_MAX_AGENTS)) {
    // Claim the slot BEFORE the turn — a crash mid-turn must not mean a
    // retry storm on the next pass.
    await logEvent(agent.model, 'sweep', { note: '(started)' })
    const note = await checkInTurn(agent).catch((e: Error) => {
      // The note lands in outreach_events for the admin view, but an agent
      // whose check-in fails every pass should also be visible in the log.
      failed++
      console.error(`[outreach] check-in turn for ${agent.model} failed:`, e.message)
      return `error: ${e.message}`
    })
    turns++
    await sql`
      update outreach_events set note = ${(note ?? '(no reply)').slice(0, 500)}
      where id = (
        select id from outreach_events
        where agent_model = ${agent.model} and kind = 'sweep'
        order by created_at desc limit 1
      )
    `
  }
  return { turns, failed, deferred }
}

// Scheduled, not kicked. This used to be `maybeOutreachSweep()` fired from GET
// /api/channels behind a module-level 5-minute timestamp — so on an instance
// nobody was reading comms on, proactive agents were never proactive.
registerJob({
  name: 'outreach-sweep',
  everyMs: SWEEP_TICK_MS, // unchanged: the same 5 minutes the kick throttled to
  // Long enough that a deploy (and a crash loop) never reaches the point of
  // messaging a human, and that the fleet's agents are registered first.
  firstRunDelayMs: 5 * 60_000,
  // SWEEP_MAX_AGENTS agent turns through the gateway, serially.
  maxRunMs: 10 * 60_000,
  run: async () => {
    const { turns, failed, deferred } = await sweepOutreach()
    if (!turns) return null
    return (
      `${turns} check-in turn(s)` +
      (failed ? `, ${failed} failed` : '') +
      (deferred ? `, ${deferred}+ agent(s) still due (capped at ${SWEEP_MAX_AGENTS}/pass)` : '')
    )
  },
})

const NOTHING = 'NOTHING_TO_SURFACE'

/** The signals + rules for one agent's check-in, sent through its own persona
 *  gateway so any action it takes uses its normal, governed MCP tools. */
async function checkInTurn(agent: SweepAgent): Promise<string> {
  const sql = await db()

  // Signals: the agent's own assigned work with staleness, anything it sent
  // to quality review that's still waiting, and its recent outreach (so it
  // doesn't repeat itself).
  const work = (await sql`
    select t.id, t.title, t.status, b.name as board,
           extract(epoch from now() - t.updated_at)::int / 3600 as "idleHours"
    from tasks t join boards b on b.id = t.board_id
    where t.assignees @> ${sql.json([agent.model])}::jsonb
      and (
        t.status = 'blocked'
        or t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category in ('open', 'active', 'review') and bs.key <> (
          select bs2.key from board_statuses bs2 where bs2.board_id = t.board_id and bs2.category = 'open' and not bs2.agent_start order by bs2.position limit 1
        ))
        or (
          not exists (select 1 from board_statuses bs where bs.board_id = t.board_id)
          and t.status in ('assigned', 'in_progress', 'quality_review')
        )
      )
      and t.archived_at is null
    order by t.updated_at asc limit 15
  `) as unknown as Array<{ id: string; title: string; status: string; board: string; idleHours: number }>

  const recent = (await sql`
    select kind, note, created_at as "createdAt" from outreach_events
    where agent_model = ${agent.model} and note is not null and note not in ('(started)', ${NOTHING})
      and created_at > now() - interval '7 days'
    order by created_at desc limit 10
  `) as unknown as Array<{ kind: string; note: string; createdAt: string }>

  const workLines = work.length
    ? work.map((t) => `- [${t.status}] "${t.title}" (board ${t.board}, ticket ${t.id}, idle ${t.idleHours}h)`).join('\n')
    : '(no assigned tickets)'
  const recentLines = recent.length
    ? recent.map((r) => `- ${r.kind}: ${r.note}`).join('\n')
    : '(none)'

  const prompt =
    `[Automated periodic check-in — no human sent this message.]\n\n` +
    `This is your chance to be proactive: look at your current work and surface anything a teammate genuinely needs to know. ` +
    `Act through your talaria tools — \`comment\` on a ticket, \`post_to_channel\`, or \`message_user\` to reach someone directly. ` +
    `Then reply with ONE short line saying what you did and why.\n\n` +
    `Your assigned tickets:\n${workLines}\n\n` +
    `Your recent outreach (do NOT repeat any of this):\n${recentLines}\n\n` +
    `Rules:\n` +
    `- At most 2 actions. Zero is the right number most of the time.\n` +
    `- Only surface things that are real and current: work stuck or blocked with a reason a human should hear, something you noticed that needs a decision, a promise about to slip.\n` +
    `- Never invent tickets, findings, or urgency. Never nag about the same thing twice.\n` +
    `- If nothing genuinely warrants outreach, do nothing and reply exactly: ${NOTHING}`

  const messages = [{ role: 'user', content: prompt }]
  const upstream = await proxyChat({ model: agent.model, messages }, { waitMs: 30_000 })
  if (!upstream.ok || !upstream.body) throw new Error(`gateway error ${upstream.status}`)

  let text = ''
  let usage: { promptTokens: number; completionTokens: number } | null = null
  for await (const ev of parseAgentStream(upstream.body)) {
    if (ev.type === 'content') text += ev.text
    else if (ev.type === 'usage') usage = ev
  }
  void recordUsage({
    agentModel: agent.model,
    source: 'chat',
    refId: null,
    tier: null,
    promptTokens: usage?.promptTokens ?? estimateTokens(prompt.length),
    completionTokens: usage?.completionTokens ?? estimateTokens(text.length),
    estimated: !usage,
  })
  return text.trim() || NOTHING
}

// ── admin visibility ─────────────────────────────────────────────────────────

export interface OutreachEventRow {
  agentModel: string
  kind: string
  note: string | null
  createdAt: string
}

export async function recentOutreachEvents(limit = 30): Promise<OutreachEventRow[]> {
  const sql = await db()
  return (await sql`
    select agent_model as "agentModel", kind, note, created_at as "createdAt"
    from outreach_events order by created_at desc limit ${limit}
  `) as unknown as OutreachEventRow[]
}
