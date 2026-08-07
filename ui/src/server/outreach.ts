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
// "GUARD-VISIBLE" was half true until agent-writes.ts existed, and the half that
// was false is the half a person reads: the check-in's REPORT line came back
// through `runHarness` and was scanned and redacted, while the DM the agent sent
// during that same turn went straight into a human's chat and inbox. Every one
// of those three tools now goes through `guardAgentWrite` at its write path —
// `message_user`'s is `agentMessageUser` below.
//
// Everything is opt-in twice over: a master switch (off by default) AND a
// per-agent `proactive` flag. outreach_events is the memory: it powers the
// daily caps, the "don't repeat yourself" context, and admin visibility.
import { db } from './db/pg'
import { getSetting, setSetting } from './audit'
import { guardAgentWrite } from './agent-writes'
import { needsRedaction, redactSecrets } from './guardrails'
import {
  createConversation,
  insertStreamingAssistant,
  nextSeq,
  setMessageGuard,
  touchConversation,
  updateAssistant,
} from './conversations'
import { addNotification } from './notifications'
import { describeAgent } from './gateway'
import { NOTHING_TO_SURFACE as NOTHING, outreachCheckInHarness } from './harness/defs/outreach'
import { runHarness } from './harness/run'
import { registerJob } from './scheduler'

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

  // THE GUARD ON THE DM ITSELF, and the asymmetry it ends: the check-in turn's
  // REPORT line goes through `runHarness` and is scanned and redacted, while the
  // DM the agent actually sent during that same turn — the thing a person
  // reads — used to be persisted and notified untouched. `mcp message_user`
  // arrives here as a tool ARGUMENT; nothing else on this path ever looked at
  // it. See agent-writes.ts for why a credential is redacted rather than
  // blocked: refusing a DM silently is how a teammate never hears about the
  // thing that needed them.
  const guarded = await guardAgentWrite('direct-message', { agent: agentModel }, message)
  const body = guarded.text

  const seq = await nextSeq(convId)
  const msgId = await insertStreamingAssistant(convId, seq)
  await updateAssistant(msgId, { content: body, reasoning: '', tools: [], status: 'complete' })
  // annotate/strict pin the findings onto the message as metadata the UI renders
  // as a caveat — chat-persist.ts's exact precedent, and safe for the same
  // reason it is there: `setMessageGuard` writes a column no transcript builder
  // reads, so the snippet never travels back into a model's context.
  if (guarded.findings.length && (guarded.mode === 'annotate' || guarded.mode === 'strict')) {
    await setMessageGuard(msgId, guarded.findings).catch(() => {})
  }
  await touchConversation(convId)

  await addNotification(target.id, {
    kind: 'agent-outreach',
    title: `${label} reached out`,
    body: body.length > 140 ? `${body.slice(0, 140)}…` : body,
    href: `/comms?a=${encodeURIComponent(agentModel)}&x=${convId}`,
  }).catch(() => {})

  // THE EVENT NOTE IS NOT JUST AUDIT — `checkInTurn` reads recent notes back
  // into the agent's next check-in prompt ("don't repeat yourself"), so this row
  // is a path from a flagged DM into a model's context, which guardrails.ts
  // forbids in every mode and not only in strict. Hence the second scrub: strict
  // already redacted `body`, and observe/annotate deliberately did not — but
  // what goes into the note has to be clean either way. `redactSecrets` on
  // already-redacted text is a no-op.
  const note = needsRedaction(guarded.findings) ? redactSecrets(body).text : body
  await logEvent(agentModel, 'dm', { userId: target.id, conversationId: convId, note })
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

  // The prompt, the rules block and the NOTHING_TO_SURFACE contract live in
  // harness/defs/outreach.ts now; this function's job is the two signal queries
  // above. The model is the AGENT'S OWN — there is no platform-agent slot for
  // outreach and there should not be one, so the harness declares no chain and
  // takes the model from here.
  //
  // NO TRANSPORT, and that is the whole of what changed. `personaTurnWithOwnTools`
  // lived here to keep the agent's MCP tools live and to meter the turn; the
  // harness declares `tools: 'own'` and `holdMs` now, and `meterPersonaTurn`
  // writes the same ledger row the shim did (`source: 'chat'`, no refId — a
  // check-in belongs to no conversation). Nothing is attributed here because
  // there is nothing to attribute it to.
  const run = await runHarness(outreachCheckInHarness, { work, recent }, { caller: `outreach:${agent.model}`, model: agent.model })
  // A silent turn is not a failure — the harness's declared fallback IS the
  // NOTHING token, so `text.trim() || NOTHING` is preserved without this file
  // restating it. A null value means the turn never completed (the gateway was
  // down, the stream died), which is what `sweepOutreach` already catches and
  // records as `error: …` on the event row.
  if (run.value === null) throw new Error(run.error ?? 'the check-in turn produced no reply')
  return run.value
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
