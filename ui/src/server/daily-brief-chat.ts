// Talking to your assistant about the brief in front of you.
//
// THIS WAS EPHEMERAL AND IS NOT ANY MORE, and the reason it changed is worth
// keeping. The original argument was that a person asks loose, half-formed
// questions about their own day ("is the Priya thing actually urgent?") and
// would stop if those were minuted next to the document they are about. That
// may still be true, and it was beside the point: the answers this surface
// gives END IN A NAVIGATION. You ask why a ticket is stuck, you go and look at
// the ticket, you come back — and an ephemeral thread is gone by the time you
// return, having been destroyed by the one action it existed to prompt.
//
// So the thread persists, scoped to the brief and to the line it is about
// (`brief_chat_messages`). Per brief rather than per line forever: a brief is a
// document about one day, and tomorrow's opens clean.
//
// The delta is passed SEPARATELY from the document, which is the one piece of
// prompt design this file owns. "What changed since I looked" is the question
// the surface exists for, and a model handed a full brief and asked to find the
// difference inside it answers by summarizing the brief again. `since` is
// computed from `read_seq` — the same cursor the surface uses to mark lines new
// — so the assistant's idea of "since you looked" and the page's are the same
// idea, read from the same column.
import type { SessionUser } from './api-guard'
import { db } from './db/pg'
import { briefAssistant, foldEntries, type BriefEntry } from './daily-brief'
import { briefConfig, briefWindow, zoneFor } from './daily-brief-config'
import { contentFrame } from './briefing'
import { dailyBriefChatHarness } from './harness/defs/briefer'
import { fleetStream, runHarnessStreamed } from './harness/run'

export interface BriefChatInput {
  content: string
  history: Array<{ role: 'user' | 'assistant'; content: string }>
  sourceKey: string | null
}

interface Loaded {
  briefId: string
  agentModel: string
  brief: string
  since: string | null
  focus: string | null
}

/** Everything the prompt needs, read once. Returns null when there is nothing
 *  to talk about — no assistant, or no brief today — which the caller turns
 *  into a plain error rather than a stream that says nothing. */
async function loadContext(user: SessionUser, sourceKey: string | null): Promise<Loaded | null> {
  const config = await briefConfig()
  const zone = zoneFor(user.id, config)
  const { date } = briefWindow(config, zone, new Date())
  const sql = await db()

  const rows = (await sql`
    select id, read_seq as "readSeq", agent_model as "agentModel"
    from daily_briefs where user_id = ${user.id} and brief_date = ${date}
  `) as unknown as Array<{ id: string; readSeq: number; agentModel: string | null }>
  const row = rows[0]
  if (!row) return null

  // The model comes from the BRIEF ROW, not from a fresh lookup, so the
  // assistant answering about a document is the assistant that wrote it. An
  // owner who re-pointed their assistant at lunchtime does not get this
  // morning's brief explained by a model that has never seen its prompt.
  const agentModel = row.agentModel ?? (await briefAssistant(user.id)).model
  if (!agentModel) return null

  const entries = (await sql`
    select id, seq, batch, kind, section, source_key as "sourceKey", source_type as "sourceType",
           source_id as "sourceId", source_href as "sourceHref", fingerprint, supersedes,
           priority, status_label as "statusLabel", badge, title, body, evidence,
           created_at as "createdAt"
    from daily_brief_entries where brief_id = ${row.id} order by seq asc
  `) as unknown as BriefEntry[]

  const { lines } = foldEntries(entries, row.readSeq)
  const lede = entries.find((e) => e.kind === 'lede')

  const brief = [
    lede?.body ?? '',
    ...lines.map((l) => {
      const e = l.current
      const tags = [e.statusLabel, e.badge?.label].filter(Boolean).join(' · ')
      return `- [${l.section}]${l.resolved ? ' (resolved)' : ''} ${e.title}${tags ? ` (${tags})` : ''}${e.body ? ` — ${e.body}` : ''}`
    }),
  ]
    .filter(Boolean)
    .join('\n')

  const unseen = entries.filter((e) => e.seq > row.readSeq && e.kind !== 'lede')
  const since = unseen.length
    ? unseen.map((e) => `- ${e.kind === 'resolved' ? 'resolved' : e.kind === 'change' ? 'changed' : 'new'} — ${e.title}${e.body ? ` — ${e.body}` : ''}`).join('\n')
    : null

  const focusLine = sourceKey ? lines.find((l) => l.key === sourceKey) : undefined
  const focus = focusLine
    ? `${focusLine.current.title}${focusLine.current.body ? ` — ${focusLine.current.body}` : ''}${focusLine.resolved ? ' (already resolved)' : ''}`
    : null

  return { briefId: row.id, agentModel, brief, since, focus }
}

export interface BriefChatMessage {
  role: 'user' | 'assistant'
  content: string
  createdAt: string
}

/** The saved thread for one line (or for the day, with `sourceKey: null`). */
export async function briefChatHistory(user: SessionUser, sourceKey: string | null): Promise<BriefChatMessage[]> {
  const config = await briefConfig()
  const { date } = briefWindow(config, zoneFor(user.id, config), new Date())
  const sql = await db()
  return (await sql`
    select m.role, m.content, m.created_at as "createdAt"
    from brief_chat_messages m
    join daily_briefs b on b.id = m.brief_id
    where b.user_id = ${user.id} and b.brief_date = ${date}
      and m.source_key is not distinct from ${sourceKey}
    order by m.seq asc
  `) as unknown as BriefChatMessage[]
}

/** Append one turn. Sequenced per thread with a subquery rather than a counter
 *  held in this process, so two tabs answering at once cannot collide on a seq —
 *  the unique index is the backstop that turns any remaining race into a failed
 *  insert rather than two messages claiming the same position. */
async function appendTurn(
  briefId: string,
  userId: string,
  sourceKey: string | null,
  role: 'user' | 'assistant',
  content: string,
): Promise<void> {
  const sql = await db()
  await sql`
    insert into brief_chat_messages (brief_id, user_id, source_key, seq, role, content)
    values (
      ${briefId}, ${userId}, ${sourceKey},
      coalesce((select max(seq) from brief_chat_messages
                where brief_id = ${briefId} and source_key is not distinct from ${sourceKey}), 0) + 1,
      ${role}, ${content}
    )
  `
}

/** Stream the assistant's answer. Same wire arrangement as `briefingChat` in
 *  server/briefing.ts, and the same reasons behind it: the run is what closes
 *  the stream (so the guard pass and the telemetry row happen on the tail of
 *  the response rather than being skipped), `handOver` may fire only once
 *  because `new Response(body)` may be constructed only once, and a run that
 *  streamed and then failed its contract does not retroactively turn a
 *  delivered answer into an error. */
export async function briefChat(user: SessionUser, input: BriefChatInput): Promise<Response> {
  const loaded = await loadContext(user, input.sourceKey)
  if (!loaded) throw new Error('there is no brief to talk about yet')

  let deliver!: (response: Response) => void
  let fail!: (error: Error) => void
  const streamed = new Promise<Response>((resolve, reject) => {
    deliver = resolve
    fail = reject
  })

  const encoder = new TextEncoder()
  const wire = new TransformStream<Uint8Array, Uint8Array>()
  const writer = wire.writable.getWriter()
  let delivered = false
  const handOver = (): void => {
    if (delivered) return
    delivered = true
    deliver(new Response(wire.readable, { headers: { 'content-type': 'text/event-stream' } }))
  }

  // The question is saved BEFORE the answer is attempted. A turn whose model
  // fell over still shows the person what they asked when they come back —
  // otherwise a failed reply erases their own words along with the answer.
  await appendTurn(loaded.briefId, user.id, input.sourceKey, 'user', input.content)

  void runHarnessStreamed(
    dailyBriefChatHarness,
    { brief: loaded.brief, since: loaded.since, focus: loaded.focus, history: input.history, content: input.content },
    { caller: 'briefer:daily-chat', model: loaded.agentModel },
    {
      stream: fleetStream,
      onDelta: (delta) => {
        handOver()
        void writer.write(encoder.encode(contentFrame(delta))).catch(() => {})
      },
    },
  )
    .then(async (run) => {
      if (run.value) {
        handOver()
        // `run.value`, not the accumulated deltas: the runner's value is the
        // GUARDED copy, and this harness now redacts (it persists). Saving the
        // raw stream instead would keep a credential in the table that the
        // guard had already removed from the run.
        await appendTurn(loaded.briefId, user.id, input.sourceKey, 'assistant', run.value).catch((e: unknown) =>
          console.error('[brief-chat] could not save the reply:', e),
        )
      } else {
        fail(new Error(run.error ?? 'the assistant produced no reply'))
      }
    })
    .catch((err: unknown) => fail(err instanceof Error ? err : new Error(String(err))))
    .finally(() => {
      void writer.close().catch(() => {})
    })

  return streamed
}
