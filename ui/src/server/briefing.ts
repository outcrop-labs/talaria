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
import { briefableResearch } from './research'
import { briefingChatHarness, briefingHarness, type BriefingScope } from './harness/defs/briefer'
import { fleetStream, runHarness, runHarnessStreamed } from './harness/run'

// The prompts, the scope table and the model both halves run on live in
// harness/defs/briefer.ts now. This file is what it always was underneath: the
// attention state, the fingerprint that decides when to regenerate, and the
// two database rows. Re-exported so every existing importer of `BriefingScope`
// is untouched.
export type { BriefingScope }

interface AttentionState {
  /** Stable identity of the attention set — regenerate only when it changes. */
  fingerprint: string
  /** Compact human-readable lines the model summarizes from. */
  lines: string[]
  empty: boolean
}

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
      and (
        t.status in ('blocked', 'failed')
        or t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'review')
        or t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and bs.category = 'open' and not bs.agent_start)
        or (
          not exists (select 1 from board_statuses bs where bs.board_id = t.board_id)
          and t.status in ('inbox', 'quality_review')
        )
      )
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

  // ASKED THROUGH research.ts, not with a `status in (...)` of our own. That
  // column is a TERMINAL outcome since research became a durable run — a run
  // that is searching right now still reads 'queued', and one a driver gave up
  // on reads 'queued' for ever — so a raw filter here would put a dead run in
  // somebody's briefing every morning. `briefableResearch` projects from the
  // run, which is the one authority on whether it is alive.
  const runs = scope !== 'research' ? [] : await briefableResearch(userId)

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

  // The per-scope prompts, the model chain (there isn't one — see below) and
  // the empty-reply check all moved into harness/defs/briefer.ts. What is left
  // here is the attention state this function was always really about.
  //
  // `model` is passed EXPLICITLY and there is no fallback behind it. The
  // briefer is the only platform agent an admin cannot assign — it is always
  // the owner's own personal assistant, because it reads their private
  // attention state — so the harness declares an empty chain and this is the
  // one place the model comes from.
  const run = await runHarness(
    briefingHarness,
    { scope, lines: state.lines, empty: state.empty },
    { caller: `briefer:brief:${scope}`, model: assistant.model },
  )
  // Still a throw: `getBriefing` fires this detached and its `.catch` is what
  // clears the `generating` flag, so a failure that returned quietly would leave
  // the panel spinning until the grace window expired. The previous summary is
  // untouched either way — a failed regeneration shows stale, never blank.
  const text = run.value
  if (!text) throw new Error(run.error ?? 'empty briefing')

  await sql`
    update briefings
    set summary = ${text}, generating = false, generated_at = now()
    where user_id = ${userId} and scope = ${scope}
  `
}

/** ONE CONTENT DELTA, in the only SSE shape `lib/sse-parse.ts` reads.
 *
 *  The tee used to relay the persona's own frames, so this file never had to
 *  know the wire format; `runHarnessStreamed` hands over content deltas instead,
 *  and something has to put them back on a wire. Exported so `briefing.test.ts`
 *  can feed the output through the REAL `parseAgentStream` — a mismatch here is
 *  not an error anywhere, it is a panel that streams nothing and an owner who
 *  thinks their assistant did not answer.
 *
 *  Only `content` is re-emitted, which is exactly what `AssistantBriefing.svelte`
 *  consumes: it ignores tool and usage events on this surface, and the run's own
 *  guard pass is what the tool names are for. */
export const contentFrame = (delta: string): string => `data: ${JSON.stringify({ choices: [{ delta: { content: delta } }] })}\n\n`

/** Ephemeral follow-up chat on a briefing: the exchange streams through the
 *  assistant with the current briefing as context and is NEVER persisted —
 *  no conversation row, no messages, nothing to distill later.
 *
 *  STREAMING IS THE FEATURE, so this does not become a blocking harness call —
 *  it is `runHarnessStreamed`, the runner's streaming entry point, over
 *  `fleetStream`. The owner watches the answer arrive token by token; the runner
 *  accumulates the same deltas and does its guard pass once the reply ends. That
 *  is the arrangement guardrails.ts's header describes for a live surface: the
 *  stream already showed the owner the original, and the guard's job here is the
 *  FINDING — `guard_findings.model` is the per-model confabulation rate the
 *  fitness page reads — rather than a rewrite. There is nothing to redact
 *  because there is nothing saved, which is the same design decision this
 *  function has always carried, now stated in the harness (`briefer:chat`
 *  declares no `redact`).
 *
 *  THE TEE IS GONE. This function used to hand `runHarness` a transport of its
 *  own that called `proxyChat`, split the body with `.tee()`, sent one branch to
 *  the owner and drained the other for the guard — because the runner blocked,
 *  and because its fleet transport disarmed the tools this prompt deliberately
 *  permits. Both are declarations now (`runHarnessStreamed`, and `tools: 'own'`
 *  on the harness), so the shim is deleted rather than reduced. What is left
 *  here is the one thing the runner cannot do: turn content deltas back into the
 *  SSE frames the panel's `parseAgentStream` reads.
 *
 *  Until this path was guarded at all, whether a personal-assistant reply got a
 *  guardrail pass depended on which surface it came from: ordinary chat runs
 *  `guardChatReply`, and this panel — asking the same assistant about the same
 *  work — ran nothing (audit 1.5). */
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
  const assistant = assistants[0]
  if (!assistant) throw new Error('no personal assistant')
  const rows = (await sql`
    select summary from briefings where user_id = ${userId} and scope = ${scope}
  `) as unknown as Array<{ summary: string }>

  // Settled on the first token the persona emits, or rejected by the run when it
  // never got that far — so a caller that used to see `throw new Error('gateway
  // 502')` still does, and the route still answers a dead gateway with a 500
  // rather than with an empty stream the panel would render as a blank reply.
  let deliver!: (response: Response) => void
  let fail!: (error: Error) => void
  const streamed = new Promise<Response>((resolve, reject) => {
    deliver = resolve
    fail = reject
  })

  const encoder = new TextEncoder()
  const wire = new TransformStream<Uint8Array, Uint8Array>()
  const writer = wire.writable.getWriter()
  /** ONCE. `new Response(body)` may be constructed once per stream — a second
   *  construction over a readable that is already being read throws — and
   *  resolving a settled promise is a silent no-op, so the guard has to be here
   *  rather than left to `deliver`. */
  let delivered = false
  const handOver = (): void => {
    if (delivered) return
    delivered = true
    deliver(new Response(wire.readable, { headers: { 'content-type': 'text/event-stream' } }))
  }

  // Detached on purpose: the guard pass, the findings row and the harness_runs
  // row all happen after the owner already has their answer. `fail` is a no-op
  // once `handOver` has run, so a model that streamed a reply and then failed
  // its contract does not retroactively turn a delivered stream into an error.
  void runHarnessStreamed(
    briefingChatHarness,
    { scope, summary: rows[0]?.summary ?? null, history, content },
    { caller: `briefer:chat:${scope}`, model: assistant.model },
    {
      stream: fleetStream,
      onDelta: (delta) => {
        handOver()
        // Write ordering is the writer's own queue, so these do not need
        // chaining. Backpressure is deliberately not applied: a briefing answer
        // is a few hundred tokens, and blocking the persona stream on a slow
        // reader would stall the guard pass as well as the screen.
        void writer.write(encoder.encode(contentFrame(delta))).catch(() => {})
      },
    },
  )
    .then((run) => {
      // A run that produced a value MUST have emitted at least one delta, since
      // both come from the same content events — but resolving anyway is the
      // difference between a defensive branch and a request that hangs forever
      // if that ever stops being true.
      if (run.value) handOver()
      else fail(new Error(run.error ?? 'the briefing chat produced no reply'))
    })
    .catch((err: unknown) => fail(err instanceof Error ? err : new Error(String(err))))
    // The owner's stream closes when the RUN settles rather than when the
    // persona stops talking, which adds the guard pass and two telemetry rows to
    // the tail of the response. That is milliseconds of regex and one insert,
    // and it is the price of the delta accumulation being inside the runner
    // instead of in a tee here.
    .finally(() => {
      void writer.close().catch(() => {})
    })

  return streamed
}
