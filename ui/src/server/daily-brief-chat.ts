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

/** How often to send a keep-alive comment while the run is in flight. Fifteen
 *  seconds is comfortably inside the idle timeout of every proxy in the usual
 *  path and is invisible to the client. */
const HEARTBEAT_MS = 15_000

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

/** Stream the assistant's answer.
 *
 *  The response is constructed and returned IMMEDIATELY; the run fills the
 *  stream behind it and closes it when it settles. That ordering is the whole
 *  point — see the note inside — because this harness can spend a minute or two
 *  in a tool loop before it writes a word, and a response that has not been
 *  handed over yet is a request that looks hung.
 *
 *  The stream closes when the RUN settles rather than when the persona stops
 *  talking, which adds the guard pass and two telemetry rows to the tail of the
 *  response. That is milliseconds of regex and one insert, and it is the price
 *  of the delta accumulation living inside the runner instead of in a tee here. */
export async function briefChat(user: SessionUser, input: BriefChatInput): Promise<Response> {
  const loaded = await loadContext(user, input.sourceKey)
  if (!loaded) throw new Error('there is no brief to talk about yet')

  const encoder = new TextEncoder()
  const wire = new TransformStream<Uint8Array, Uint8Array>()
  const writer = wire.writable.getWriter()

  // THE RESPONSE IS HANDED OVER NOW, BEFORE THE MODEL IS ASKED. This used to
  // wait for the first content delta, and that is what "the chat is totally
  // broken" actually was.
  //
  // This harness declares `tools: 'own'`, so a real question — "why is this
  // ticket stuck?" — spends a minute or two calling `get_ticket` and friends
  // before it writes a word. Deferring the handover meant the route's promise
  // stayed pending for that whole time: no status, no headers, no open stream.
  // The browser had nothing to render, the loader never appeared (there was no
  // response to attach it to), and the request looked hung — because it was.
  //
  // Handing over immediately gives the client a 200 and an open stream at once,
  // so the waiting mark shows while the assistant works and tokens land as they
  // are produced. It is safe now in a way it was not before: EVERY outcome
  // writes a frame (see the settle handler below), so a failure arrives as an
  // honest sentence in the thread rather than as an empty stream the panel
  // would render as the assistant saying nothing.
  const response = new Response(wire.readable, { headers: { 'content-type': 'text/event-stream' } })

  // PRIME THE STREAM. An SSE response that has been handed over but has sent no
  // bytes is, from the far end, indistinguishable from a request that hung: the
  // headers may not have flushed, nothing has arrived, and every layer in
  // between is free to assume the worst. A comment frame costs nine bytes and
  // makes the connection real at once — `parseAgentStream` reads only `event:`
  // and `data:` lines, so a comment is invisible to the client.
  void writer.write(encoder.encode(': open\n\n')).catch(() => {})

  // And keep it alive while the assistant works. This harness declares
  // `tools: 'own'`, so a real question spends a minute or two calling tools
  // before it writes a word — a silence long enough for an idle proxy to close
  // the connection underneath a person who is still waiting for their answer.
  const heartbeat = setInterval(() => {
    void writer.write(encoder.encode(': working\n\n')).catch(() => {})
  }, HEARTBEAT_MS)

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
        // Write ordering is the writer's own queue, so these need no chaining.
        void writer.write(encoder.encode(contentFrame(delta))).catch(() => {})
      },
    },
  )
    .then(async (run) => {
      // `run.value`, not the accumulated deltas: the runner's value is the
      // GUARDED copy, and this harness redacts (it persists). Saving the raw
      // stream instead would keep a credential in the table that the guard had
      // already removed from the run.
      //
      // EVERY OUTCOME WRITES AN ASSISTANT TURN, including the failures, and
      // that is the fix for the way this broke. The question is persisted
      // BEFORE the model is asked (so a dead turn still shows the person what
      // they typed), and a failure used to reject the promise — a 500, no
      // assistant row, and a saved question that would never have an answer
      // under it. Reload and the thread was a column of your own messages into
      // silence, permanently, with the retry button the only thing that looked
      // broken.
      //
      // TWO FAILURES, TWO SENTENCES, because they are different facts and the
      // person's next move differs. `answered` is the runner's own distinction:
      // a model that spoke and wrote no prose is a TOOL-ONLY TURN — this
      // harness declares `tools: 'own'`, so "add a comment saying I don't know"
      // is answered by calling `comment` and returning nothing, which is the
      // model doing exactly what it was asked. Reporting that as an outage was
      // the bug; the reply really is empty, and saying so is the honest answer.
      const text =
        run.value ??
        (run.answered
          ? '_Done — I acted on that and had nothing to add._'
          : '_I could not reach your assistant just now. Your question is saved; ask again and it will retry._')
      // The reason is LOGGED, not swallowed. The person gets a sentence they can
      // act on; an operator needs the runner's own error, and a failure that
      // reports nothing anywhere is how "the chat is broken" stays a mystery.
      if (!run.value) {
        console.error(
          `[brief-chat] no reply for ${user.id} (answered=${run.answered}, model=${loaded.agentModel}): ${run.error ?? 'no error given'}`,
        )
      }
      if (!run.value) void writer.write(encoder.encode(contentFrame(text))).catch(() => {})
      await appendTurn(loaded.briefId, user.id, input.sourceKey, 'assistant', text).catch((e: unknown) =>
        console.error('[brief-chat] could not save the reply:', e),
      )
    })
    .catch((err: unknown) => {
      // A throw from the runner itself. The stream is already open, so the only
      // way to tell the person is on the wire — rejecting here would reject a
      // promise nobody is holding any more.
      console.error('[brief-chat] the run threw:', err)
      void writer
        .write(encoder.encode(contentFrame('_Something went wrong answering that. Your question is saved; ask again to retry._')))
        .catch(() => {})
    })
    .finally(() => {
      clearInterval(heartbeat)
      void writer.close().catch(() => {})
    })

  return response
}
