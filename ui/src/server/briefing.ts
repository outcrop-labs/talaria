// Console briefings — the personal assistant's short read on what needs the
// user's attention, per scope, regenerated ONLY when the attention state
// actually changes. Deliberately EPHEMERAL: the summary lives in the
// briefings row (replaced on regen) and follow-up chat streams straight
// through the assistant with the briefing as context — nothing lands in
// conversations, nothing gets indexed or distilled later.
import { createHash } from 'node:crypto'
import { db } from './db/pg'
import { listChannels } from './channels'
import { listPending } from './google/pending-actions'
import { proxyChat } from './gateway'
import { parseAgentStream } from '@/lib/sse-parse'
import { estimateTokens, recordUsage } from './usage'

interface AttentionState {
  /** Stable identity of the attention set — regenerate only when it changes. */
  fingerprint: string
  /** Compact human-readable lines the model summarizes from. */
  lines: string[]
  empty: boolean
}

export type BriefingScope = 'inbox' | 'boards' | 'comms' | 'plans' | 'research'

async function attentionState(userId: string, isAdmin: boolean, scope: BriefingScope): Promise<AttentionState> {
  const sql = await db()

  const NOTIF_KINDS: Record<BriefingScope, string[] | null> = {
    inbox: null, // all unread
    boards: [],
    comms: ['mention', 'dm', 'agent-outreach'],
    plans: ['plan-share'],
    research: ['research'],
  }
  const kinds = NOTIF_KINDS[scope]
  const notifs = kinds !== null && kinds.length === 0 ? [] : ((await sql`
    select id, kind, title from notifications
    where user_id = ${userId} and read_at is null
      ${kinds ? sql`and kind = any(${kinds})` : sql``}
    order by created_at desc limit 20
  `) as unknown as Array<{ id: string; kind: string; title: string }>)

  const queue = !['inbox', 'boards'].includes(scope) ? [] : ((await sql`
    select t.id, t.title, t.status, b.name as board
    from tasks t
    join boards b on b.id = t.board_id
    left join board_members m on m.board_id = b.id and m.user_id = ${userId}
    left join team_members tm on tm.team_id = b.team_id and tm.user_id = ${userId}
    where (m.user_id is not null or tm.user_id is not null)
      and b.archived_at is null and t.archived_at is null
      and t.status in ('inbox', 'quality_review', 'blocked', 'failed')
    order by t.updated_at desc limit 30
  `) as unknown as Array<{ id: string; title: string; status: string; board: string }>)

  const approvals = scope === 'inbox' ? await listPending(userId, isAdmin).catch(() => []) : []
  const channels = ['inbox', 'comms'].includes(scope) ? await listChannels(userId).catch(() => []) : []
  const unread = channels.filter((c) => (c.unreadCount ?? 0) > 0)

  const plans = scope !== 'plans' ? [] : ((await sql`
    select c.id, c.title, date_trunc('minute', c.updated_at) as bucket,
           exists(select 1 from messages m where m.conversation_id = c.id and m.role = 'assistant'
                    and m.status = 'streaming' and m.created_at > now() - interval '10 minutes') as working,
           coalesce((select m.status = 'error' from messages m where m.conversation_id = c.id and m.role = 'assistant'
                     order by m.seq desc limit 1), false) as failed
    from conversations c
    where c.kind = 'plan' and c.archived = false
      and (c.user_id = ${userId} or exists(select 1 from conversation_members cm where cm.conversation_id = c.id and cm.user_id = ${userId}))
      and c.updated_at > now() - interval '48 hours'
    order by c.updated_at desc limit 15
  `) as unknown as Array<{ id: string; title: string | null; bucket: string; working: boolean; failed: boolean }>)

  const runs = scope !== 'research' ? [] : ((await sql`
    select id, coalesce(title, question) as question, status from research_runs
    where (owner_user_id = ${userId}
        or exists(select 1 from research_members rm where rm.run_id = research_runs.id and rm.user_id = ${userId}))
      and (status in ('queued', 'running')
        or (status = 'error' and created_at > now() - interval '7 days'))
    order by created_at desc limit 10
  `) as unknown as Array<{ id: string; question: string; status: string }>)

  // Fingerprint on IDs + counts (not text): stable across renames, changed by
  // anything appearing, resolving, or accumulating.
  const fingerprint = createHash('sha256')
    .update(
      JSON.stringify({
        n: notifs.map((n) => n.id),
        q: queue.map((t) => `${t.id}:${t.status}`),
        a: approvals.map((a) => a.id),
        u: unread.map((c) => `${c.id}:${c.unreadCount}`),
        p: plans.map((p) => `${p.id}:${p.bucket}:${p.working}:${p.failed}`),
        r: runs.map((r) => `${r.id}:${r.status}`),
      }),
    )
    .digest('hex')

  const statusWord: Record<string, string> = { inbox: 'needs triage', quality_review: 'awaiting your review', blocked: 'blocked', failed: 'FAILED' }
  const lines = [
    ...notifs.map((n) => `notification (${n.kind}): ${n.title}`),
    ...queue.map((t) => `ticket ${statusWord[t.status] ?? t.status}: "${t.title}" on ${t.board}`),
    ...approvals.map((a) => `pending approval: ${a.summary ?? a.kind}`),
    ...unread.map((c) => `${c.unreadCount} unread in ${c.kind === 'dm' ? `DM with ${c.peer?.name ?? c.peer?.email ?? 'someone'}` : `#${c.name}`}`),
    ...plans.map((p) => `plan ${p.failed ? 'FAILED — its last turn errored, needs a re-run' : p.working ? 'BEING WORKED RIGHT NOW' : 'active in the last 48h'}: "${p.title ?? 'Untitled'}"`),
    ...runs.map((r) => `research ${r.status === 'error' ? 'FAILED — needs a re-run' : r.status}: "${r.question}"`),
  ]
  return { fingerprint, lines, empty: lines.length === 0 }
}

export interface BriefingView {
  /** No personal assistant — the panel shows the setup card instead. */
  none?: boolean
  summary: string
  agentModel: string | null
  agentName: string | null
  generating: boolean
  generatedAt: string | null
}

const GENERATING_GRACE_MS = 2 * 60_000

/** Current briefing, regenerating in the background when the attention set
 *  changed. Always returns fast — the client polls while `generating`. */
export async function getBriefing(userId: string, isAdmin: boolean, scope: BriefingScope = 'inbox'): Promise<BriefingView> {
  const sql = await db()
  const assistants = (await sql`
    select model, display_name as "displayName" from agent_defs
    where owner_user_id = ${userId} and enabled limit 1
  `) as unknown as Array<{ model: string; displayName: string }>
  const assistant = assistants[0]
  if (!assistant) return { none: true, summary: '', agentModel: null, agentName: null, generating: false, generatedAt: null }

  const state = await attentionState(userId, isAdmin, scope)
  const rows = (await sql`
    select fingerprint, summary, generating, generated_at as "generatedAt"
    from briefings where user_id = ${userId} and scope = ${scope}
  `) as unknown as Array<{ fingerprint: string; summary: string; generating: boolean; generatedAt: string }>
  const row = rows[0]

  const view = (generating: boolean): BriefingView => ({
    summary: row?.summary ?? '',
    agentModel: assistant.model,
    agentName: assistant.displayName,
    generating,
    generatedAt: row?.generatedAt ?? null,
  })

  if (row && row.fingerprint === state.fingerprint && !row.generating) return view(false)
  if (row?.generating && Date.now() - new Date(row.generatedAt).getTime() < GENERATING_GRACE_MS) return view(true)

  // Claim, then generate DETACHED — the request returns the stale summary now.
  await sql`
    insert into briefings (user_id, scope, fingerprint, generating, generated_at)
    values (${userId}, ${scope}, ${state.fingerprint}, true, now())
    on conflict (user_id, scope) do update set fingerprint = ${state.fingerprint}, generating = true, generated_at = now()
  `
  void generateBriefing(userId, assistant, state, scope).catch(async () => {
    const s = await db()
    await s`update briefings set generating = false where user_id = ${userId} and scope = ${scope}`.catch(() => {})
  })
  return view(true)
}

async function generateBriefing(
  userId: string,
  assistant: { model: string; displayName: string },
  state: AttentionState,
  scope: BriefingScope,
): Promise<void> {
  const sql = await db()

  // Each console view gets its OWN prompt: what the view is, which verbs
  // matter there, and what an empty state means — not one template with a
  // swapped noun.
  const SCOPE_PROMPT: Record<BriefingScope, { intro: string; guidance: string; empty: string }> = {
    inbox: {
      intro: 'You are briefing your owner as they open their INBOX — the cross-workspace view of everything unreviewed: notifications, work queues, approvals, unread rooms.',
      guidance: 'Lead with whatever is most time-sensitive regardless of area. Recommend the single next action when one is obvious ("approve", "triage", "read").',
      empty: 'Nothing is waiting on them anywhere. One short line saying they are all clear.',
    },
    boards: {
      intro: 'You are briefing your owner on the BOARDS view — ticket work waiting on a human: triage (assign or park), review (sign off agent work), and unblocking.',
      guidance: 'Think like a delivery lead: sizes of each queue, what is aging, what one triage or review pass would clear. Suggest where to start.',
      empty: 'No tickets are waiting on them — queues are clear. One short line.',
    },
    comms: {
      intro: 'You are briefing your owner on their COMMS — unread channels, relays, DMs, and mentions.',
      guidance: 'Who is waiting on a reply matters most: name people and rooms, distinguish a direct question from ambient chatter. Suggest which thread to open first.',
      empty: 'No unread conversations, nobody waiting on a reply. One short line.',
    },
    plans: {
      intro: 'You are briefing your owner on their PLANS — living planning docs, some being actively worked by agents right now, some newly shared with them.',
      guidance: 'Call out plans in motion RIGHT NOW first, then fresh shares they have not looked at. Frame as "what moved since you last looked".',
      empty: 'No plan activity since they last looked. One short line.',
    },
    research: {
      intro: 'You are briefing your owner on RESEARCH — runs in flight and finished reports they have not read yet.',
      guidance: 'Ready-and-unread reports first (that is the payoff), then what is still running. Mention what each ready report answers, not just its title.',
      empty: 'No research running and nothing unread. One short line.',
    },
  }
  const spec = SCOPE_PROMPT[scope]
  const prompt = state.empty
    ? `[Automated ${scope} briefing — no human sent this.]\n${spec.intro}\n${spec.empty} No tools, no preamble.`
    : `[Automated ${scope} briefing — no human sent this.]\n${spec.intro}\n` +
      `${spec.guidance}\n` +
      `Ground every line in the data below ONLY. Rules: at most 5 bullets, one short line each, most urgent first, lead word bolded. No preamble, no sign-off, no tools, no invented items. Group similar items ("3 research briefs ready") instead of listing each.\n\n` +
      state.lines.map((l) => `- ${l}`).join('\n')

  const upstream = await proxyChat({ model: assistant.model, messages: [{ role: 'user', content: prompt }] }, { waitMs: 30_000 })
  if (!upstream.ok || !upstream.body) throw new Error(`gateway ${upstream.status}`)
  let text = ''
  let usage: { promptTokens: number; completionTokens: number } | null = null
  for await (const ev of parseAgentStream(upstream.body)) {
    if (ev.type === 'content') text += ev.text
    else if (ev.type === 'usage') usage = ev
  }
  text = text.trim()
  if (!text) throw new Error('empty briefing')

  void recordUsage({
    agentModel: assistant.model,
    source: 'chat',
    refId: null,
    tier: null,
    promptTokens: usage?.promptTokens ?? estimateTokens(prompt.length),
    completionTokens: usage?.completionTokens ?? estimateTokens(text.length),
    estimated: !usage,
  })

  await sql`
    update briefings
    set summary = ${text}, generating = false, generated_at = now()
    where user_id = ${userId} and scope = ${scope}
  `
}

/** Ephemeral follow-up chat on a briefing: the exchange streams through the
 *  assistant with the current briefing as context and is NEVER persisted —
 *  no conversation row, no messages, nothing to distill later. */
export async function briefingChat(
  userId: string,
  scope: BriefingScope,
  history: Array<{ role: 'user' | 'assistant'; content: string }>,
  content: string,
): Promise<Response> {
  const sql = await db()
  const assistants = (await sql`
    select model from agent_defs where owner_user_id = ${userId} and enabled limit 1
  `) as unknown as Array<{ model: string }>
  if (!assistants[0]) throw new Error('no personal assistant')
  const rows = (await sql`
    select summary from briefings where user_id = ${userId} and scope = ${scope}
  `) as unknown as Array<{ summary: string }>

  const messages = [
    {
      role: 'user',
      content:
        `[Ephemeral briefing chat — this thread is NOT saved. Keep replies short and direct; use tools only if the owner's question truly needs them.]\n` +
        `Your latest briefing to your owner:\n${rows[0]?.summary ?? '(none yet)'}`,
    },
    { role: 'assistant', content: 'Got it.' },
    ...history.slice(-12),
    { role: 'user', content },
  ]
  const upstream = await proxyChat({ model: assistants[0].model, messages }, { waitMs: 30_000 })
  if (!upstream.ok || !upstream.body) throw new Error(`gateway ${upstream.status}`)
  return new Response(upstream.body, { headers: { 'content-type': 'text/event-stream' } })
}
