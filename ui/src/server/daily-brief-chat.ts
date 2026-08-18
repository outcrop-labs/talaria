// Talking to your assistant about the brief in front of you.
//
// EPHEMERAL, DELIBERATELY, AND FOR A DIFFERENT REASON THAN USUAL. The brief
// itself is the most permanent thing this feature writes — append-only, never
// rewritten, mirrored to a shareable artifact. The conversation ABOUT it is the
// opposite: no conversation row, no messages, nothing indexed or distilled
// later. Those two decisions are the same decision. A person asks their
// assistant loose, half-formed questions about their own day ("is the Priya
// thing actually urgent?"), and the moment those are minuted next to the
// document they are about, they stop being asked.
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

  return { agentModel, brief, since, focus }
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
    .then((run) => {
      if (run.value) handOver()
      else fail(new Error(run.error ?? 'the assistant produced no reply'))
    })
    .catch((err: unknown) => fail(err instanceof Error ? err : new Error(String(err))))
    .finally(() => {
      void writer.close().catch(() => {})
    })

  return streamed
}
