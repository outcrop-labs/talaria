// The daily brief — one document per person per day, opened before their
// workday and APPENDED TO for the rest of it.
//
// WHY THIS REPLACES THE INBOX
//   The focus queue answered "what is the single next decision", which is the
//   right question at 09:00 and the wrong one every other time you open the
//   app. It had no memory: a queue that reorders itself under you cannot tell
//   you what MOVED, and the two things a person actually wants from an inbox at
//   14:00 are "what changed since I looked" and "where do I go for it". A log
//   answers both by construction; a queue answers neither at any length.
//
// THE ONE RULE: IT IS NEVER REGENERATED.
//   Not "regenerated carefully", not "regenerated only when stale" — never. The
//   lede written at 07:00 is the lede at 18:00, and every item on the page is
//   the row that was inserted the moment that thing first needed the owner.
//   Change arrives as a NEW row that supersedes an old one by id, so the
//   document grows downward and nothing a person read is ever taken away from
//   them. `server/db/pg.ts` carries the schema note; the enforcement is that
//   nothing in this file issues an UPDATE against `daily_brief_entries`, and
//   the only column it updates anywhere is `read_seq`, which describes the
//   reader rather than the content.
//
//   The regenerating version of this file is `server/briefing.ts`, which is
//   still the right design for what IT does: a short ephemeral read on the
//   current attention state, replaced wholesale whenever a fingerprint moves.
//   The two coexist on purpose. A briefing is a glance; a brief is a day.
//
// THE DIFF ENGINE WAS ALREADY WRITTEN.
//   `inbox-focus-sources.ts` computes "what needs this user" across approvals,
//   tickets, DMs and notifications, and stamps every item with a
//   `sourceFingerprint` over the fields that matter. That is exactly the change
//   detector an append-only log needs, so this file consumes those four
//   functions directly rather than reimplementing their scoping — which is the
//   half that carries the read ACL and is therefore the half you least want a
//   second copy of. It deliberately does NOT go through `listFocusQueue`: that
//   adds snoozing and a per-item brief cache, both of which are queue
//   behaviours, and snoozing an item out of a log would be a lie about the day.
import { randomUUID } from 'node:crypto'
import { db } from './db/pg'
import type { SessionUser } from './api-guard'
import { approvalItems, notificationItems, taskItems } from './inbox-focus-sources'
import { commsLines, type CommsLine } from './daily-brief-comms'
import { draftReply, releaseDrafts } from './daily-brief-delegation'
import { dedupeItems, sortItems } from './inbox-focus-policy'
import type { RawFocusItem } from './inbox-focus-types'
import { listUpcomingEvents } from './google/calendar'
import { briefConfig, briefWindow, localMoment, nextBriefAt, zoneFor, type BriefConfig } from './daily-brief-config'
import { publishUser } from './realtime'
import { registerJob } from './scheduler'
import { runHarness } from './harness/run'
import { dailyBriefLedeHarness, dailyBriefNoteHarness } from './harness/defs/briefer'
import { mirrorBriefArtifact } from './daily-brief-artifact'
import { foldEntries } from './daily-brief-fold'
import {
  BRIEF_SECTIONS,
  type BriefAssistant,
  type CommsState,
  type BriefBadge,
  type BriefEntry,
  type BriefEvidence,
  type BriefPriority,
  type BriefResponse,
  type BriefSection,
  type BriefView,
} from './daily-brief-types'

export * from './daily-brief-types'
export { foldEntries } from './daily-brief-fold'

// ── Rows ─────────────────────────────────────────────────────────────────────

interface BriefRow {
  id: string
  userId: string
  briefDate: string
  zone: string
  agentModel: string | null
  agentName: string | null
  artifactId: string | null
  lastSeq: number
  readSeq: number
  lastSweptAt: string | null
  createdAt: string
}

/** An entry as it goes IN. No id, no seq — `appendEntries` owns both, and a
 *  caller that could set either could write a row out of order. */
export interface NewEntry {
  kind: BriefEntry['kind']
  section: BriefSection
  sourceKey?: string | null
  sourceType?: string | null
  sourceId?: string | null
  sourceHref?: string | null
  fingerprint?: string | null
  supersedes?: string | null
  priority?: BriefPriority | null
  statusLabel?: string | null
  badge?: BriefBadge | null
  title?: string
  body?: string
  evidence?: BriefEvidence[]
}

const asIso = (value: string | Date | null): string | null =>
  value === null ? null : value instanceof Date ? value.toISOString() : new Date(value).toISOString()

// ── The assistant ────────────────────────────────────────────────────────────

/** The owner's personal assistant, and nothing else may write a brief.
 *
 *  Same rule (and the same reason) as the briefer harness: this document is
 *  built from the owner's private attention state — their unread DMs, their
 *  approvals, their blocked tickets — so routing it through a model an admin
 *  picked would quietly hand one person's day to somebody else's chosen brain.
 *  `PLATFORM_AGENTS.briefer` is the one entry an admin cannot assign, and this
 *  is the other half of that decision. */
export async function briefAssistant(userId: string): Promise<BriefAssistant> {
  const sql = await db()
  const rows = (await sql`
    select model, display_name as "displayName" from agent_defs
    where owner_user_id = ${userId} and enabled limit 1
  `) as unknown as Array<{ model: string; displayName: string }>
  const row = rows[0]
  return { configured: !!row, model: row?.model ?? null, name: row?.displayName ?? null }
}

// ── Sources → candidate entries ──────────────────────────────────────────────

/** Where a source lands in the document. Section is a property of the SOURCE,
 *  decided once here, so a thing cannot migrate between sections on a later
 *  sweep and appear to be two different items. */
function sectionFor(item: RawFocusItem): BriefSection {
  if (item.sourceType === 'approval' || item.sourceType === 'task') return 'action'
  if (item.sourceType === 'channel') return 'comms'
  const kind = String(item.metadata.kind ?? '')
  return ['mention', 'dm', 'agent-outreach'].includes(kind) ? 'comms' : 'highlights'
}

function badgeFor(item: RawFocusItem): BriefBadge | null {
  if (item.dueAt) {
    const due = new Date(item.dueAt).getTime()
    if (due <= Date.now()) return { label: 'OVERDUE', tone: 'danger' }
    if (due <= Date.now() + 86_400_000) return { label: 'DUE TODAY', tone: 'danger' }
  }
  if (item.priority === 'p0') return { label: 'BLOCKING', tone: 'danger' }
  return null
}

const candidateFrom = (item: RawFocusItem): NewEntry => ({
  kind: 'item',
  section: sectionFor(item),
  sourceKey: item.key,
  sourceType: item.sourceType,
  sourceId: item.sourceId,
  sourceHref: item.sourceHref,
  fingerprint: item.sourceFingerprint,
  priority: item.priority,
  statusLabel: item.statusLabel,
  badge: badgeFor(item),
  title: item.question,
  body: item.recommendation,
  evidence: item.evidence,
})

/** A conversation, as a brief line.
 *
 *  Built here rather than in `daily-brief-comms.ts` so that file stays a pure
 *  read model — it answers "who is waiting", and this decides how that reads on
 *  the page. The `resolved` kind is the interesting part: this source can say
 *  "this became answered, and here is who answered it" instead of merely
 *  vanishing from the live set, which is what lets the line say YOU REPLIED or
 *  ASSISTANT REPLIED rather than a generic DONE. */
function commsCandidate(line: CommsLine): NewEntry {
  const answered = line.answeredBy !== null
  return {
    kind: answered ? 'resolved' : 'item',
    section: 'comms',
    sourceKey: line.key,
    sourceType: 'channel',
    sourceId: line.channelId,
    sourceHref: `/comms/channel/${line.channelId}`,
    fingerprint: line.sourceFingerprint,
    priority: line.priority,
    statusLabel: line.statusLabel,
    badge: line.badge,
    title: answered ? `Replied to ${line.peer}` : `${line.peer} is waiting on you`,
    body: answered ? '' : line.excerpt,
    evidence: line.draft && !line.draft.stale ? [{ label: 'Drafted reply', text: line.draft.content }] : [],
  }
}

/** Today's calendar, as brief entries.
 *
 *  OPTIONAL AND SAID SO. Most installs have no Google connection and the
 *  section simply does not appear; an install that HAS one and could not read
 *  it gets a 'note' entry saying that, because "your schedule is empty" and "I
 *  could not see your schedule" are different sentences and printing the first
 *  for the second is the exact failure `docs/UI-CONVENTIONS.md` calls
 *  empty-≠-broken. `null` here means "no connection, nothing to say". */
async function calendarEntries(userId: string, zone: string): Promise<NewEntry[] | null> {
  const events = await listUpcomingEvents(userId, Date.now(), 12).catch((e: unknown) => {
    const message = e instanceof Error ? e.message : String(e)
    // A missing/expired connection is the ordinary state, not an incident.
    if (/not connected|no google|unauthor|401|409/i.test(message)) return null
    return message
  })
  if (events === null) return null
  if (typeof events === 'string') {
    // A LINE, NOT A NARRATION, and the distinction cost a bug. Written as
    // `kind: 'note'` with no source key this landed in the day's timeline and
    // NOWHERE IN THE DOCUMENT — so the one reader who needed to know their
    // schedule was unreadable saw a schedule section that simply did not
    // appear, which is "empty" standing in for "broken" on the surface whose
    // whole contract is that those are different renderings.
    //
    // With a key it behaves like everything else: it sits in the schedule
    // section while the calendar is unreadable, it does not re-append on every
    // sweep (the fingerprint is stable for the same error), and it RESOLVES on
    // its own the moment a later sweep can read the calendar — because that
    // sweep returns real events and this key is no longer live.
    return [
      {
        kind: 'item',
        section: 'schedule',
        sourceKey: 'calendar:unreadable',
        sourceType: 'calendar',
        sourceId: 'unreadable',
        sourceHref: null,
        fingerprint: `unreadable:${events.slice(0, 120)}`,
        priority: 'p2',
        statusLabel: 'CALENDAR',
        badge: { label: 'UNREADABLE', tone: 'warn' },
        title: 'Your calendar could not be read this morning',
        body: `Google Calendar returned: ${events.slice(0, 300)}. Nothing else in this brief is affected — but today's schedule is missing from it.`,
      },
    ]
  }
  const today = localMoment(zone, new Date()).date
  return events
    .filter((e) => (e.start ? localMoment(zone, new Date(e.start)).date === today : false))
    .map((e) => ({
      kind: 'item' as const,
      section: 'schedule' as const,
      sourceKey: `calendar:${e.id}`,
      sourceType: 'calendar',
      sourceId: e.id,
      sourceHref: e.htmlLink,
      // Start, end and title only. Attendee lists churn (a tentative RSVP
      // flipping) and would append a "changed" row for something nobody asked
      // to be told about.
      fingerprint: `${e.start ?? ''}|${e.end ?? ''}|${e.summary}|${e.location ?? ''}`,
      statusLabel: e.allDay ? 'ALL DAY' : formatSlot(e.start, e.end, zone),
      title: e.summary,
      body: e.location ? `Location: ${e.location}` : '',
      evidence: e.attendees.length ? [{ label: 'With', text: e.attendees.slice(0, 8).join(', ') }] : [],
    }))
}

function formatSlot(start: string | null, end: string | null, zone: string): string {
  if (!start) return ''
  const fmt = (iso: string): string =>
    new Intl.DateTimeFormat('en-US', { timeZone: zone, hour: 'numeric', minute: '2-digit' }).format(new Date(iso))
  try {
    return end ? `${fmt(start)} – ${fmt(end)}` : fmt(start)
  } catch {
    return ''
  }
}

/** Everything that needs this person right now, as candidate entries.
 *
 *  The four source functions run in parallel and a FAILURE OF ONE IS NOT A
 *  FAILURE OF THE PASS — but it is also not silence. A source that threw is
 *  reported to the caller, which declines to append RESOLUTIONS for anything
 *  in that source's namespace: an approvals query that 500s must never be read
 *  as "all your approvals resolved", which is the single most damaging thing
 *  this file could write, because a resolution is permanent. */
async function snapshot(
  user: SessionUser,
  zone: string,
  assistantModel: string | null,
): Promise<{ candidates: NewEntry[]; failedTypes: Set<string>; calendar: NewEntry[] | null; comms: CommsLine[] }> {
  const failedTypes = new Set<string>()
  const guard = async (type: string, run: () => Promise<RawFocusItem[]>): Promise<RawFocusItem[]> =>
    run().catch((e: unknown) => {
      console.error(`[daily-brief] ${type} source failed for ${user.id}:`, e)
      failedTypes.add(type)
      return []
    })

  const [approvals, tasks, comms, notifications, calendar] = await Promise.all([
    guard('approval', () => approvalItems(user)),
    guard('task', () => taskItems(user.id)),
    // NOT `channelItems`. That selects DMs with UNREAD messages, so opening one
    // drops it out of the set and the sweep resolves the line — the brief
    // telling you a question was handled because you glanced at it. See the
    // header of daily-brief-comms.ts.
    commsLines(user.id, assistantModel).catch((e: unknown) => {
      console.error(`[daily-brief] comms source failed for ${user.id}:`, e)
      failedTypes.add('channel')
      return [] as CommsLine[]
    }),
    guard('notification', () => notificationItems(user.id)),
    calendarEntries(user.id, zone).catch(() => null),
  ])

  // `dedupeItems` drops a notification that merely points at a ticket or a
  // channel already in the set. It can no longer see the conversations (they
  // are their own source now), so the channel hrefs are re-applied here —
  // without it, "Priya mentioned you" and "Priya is waiting on you" both land
  // on the page as separate lines about the same nudge.
  const commsHrefs = comms.map((c) => `/comms/channel/${c.channelId}`)
  const deduped = dedupeItems([...approvals, ...tasks, ...notifications]).filter(
    (item) => !(item.sourceType === 'notification' && commsHrefs.some((href) => item.sourceHref.startsWith(href))),
  )
  return {
    candidates: [...sortItems(deduped).map(candidateFrom), ...comms.map(commsCandidate)],
    failedTypes,
    calendar,
    comms,
  }
}

// ── The fold ─────────────────────────────────────────────────────────────────
//
// `foldEntries` lives in daily-brief-fold.ts — pure, testable, and shared with
// the artifact mirror.

function view(row: BriefRow, entries: BriefEntry[], agent: BriefAssistant, comms: CommsState[]): BriefView {
  const { lines, updates } = foldEntries(entries, row.readSeq)
  const lede = entries.find((e) => e.kind === 'lede')
  // Priority first, then the order the log put them in. A resolved line sinks
  // but is NEVER dropped: the thing a person read this morning has to still be
  // findable this afternoon, struck through, which is the whole argument for an
  // append-only document over a queue.
  const rank: Record<string, number> = { p0: 0, p1: 1, p2: 2, ok: 3 }
  const sections = BRIEF_SECTIONS.map((section) => ({
    section,
    lines: lines
      .filter((l) => l.section === section)
      .sort((a, b) => {
        if (a.resolved !== b.resolved) return a.resolved ? 1 : -1
        const p = (rank[a.current.priority ?? 'ok'] ?? 3) - (rank[b.current.priority ?? 'ok'] ?? 3)
        return p || a.history[0]!.seq - b.history[0]!.seq
      }),
  })).filter((s) => s.lines.length > 0)

  return {
    id: row.id,
    date: row.briefDate,
    zone: row.zone,
    openedAt: asIso(row.createdAt)!,
    lede: lede?.body ?? '',
    agent,
    artifactId: row.artifactId,
    sections,
    updates,
    comms,
    lastSeq: row.lastSeq,
    readSeq: row.readSeq,
    unseenCount: entries.filter((e) => e.seq > row.readSeq && e.kind !== 'lede').length,
    lastSweptAt: asIso(row.lastSweptAt),
  }
}

// ── Reads ────────────────────────────────────────────────────────────────────

async function loadRow(userId: string, date: string): Promise<BriefRow | null> {
  const sql = await db()
  const rows = (await sql`
    select id, user_id as "userId", to_char(brief_date, 'YYYY-MM-DD') as "briefDate", zone,
           agent_model as "agentModel", agent_name as "agentName", artifact_id as "artifactId",
           last_seq as "lastSeq", read_seq as "readSeq",
           last_swept_at as "lastSweptAt", created_at as "createdAt"
    from daily_briefs where user_id = ${userId} and brief_date = ${date}
  `) as unknown as BriefRow[]
  return rows[0] ?? null
}

async function loadEntries(briefId: string): Promise<BriefEntry[]> {
  const sql = await db()
  return (await sql`
    select id, seq, batch, kind, section, source_key as "sourceKey", source_type as "sourceType",
           source_id as "sourceId", source_href as "sourceHref", fingerprint, supersedes,
           priority, status_label as "statusLabel", badge, title, body, evidence,
           created_at as "createdAt"
    from daily_brief_entries where brief_id = ${briefId} order by seq asc
  `) as unknown as BriefEntry[]
}

/** The surface's one read. Returns the day's document, or WHICH KIND of nothing
 *  — the three absences render differently and collapsing them into one empty
 *  state is how a surface tells a person "you are all clear" over a brief that
 *  simply has not been written yet. */
export async function getBrief(user: SessionUser, at = new Date()): Promise<BriefResponse> {
  const config = await briefConfig()
  const zone = zoneFor(user.id, config)
  const { due, date } = briefWindow(config, zone, at)
  const [row, agent] = await Promise.all([loadRow(user.id, date), briefAssistant(user.id)])

  if (row) {
    // Two reads, and the second is not the log. `commsLines` is asked again at
    // request time because a draft's approvability and a grant's existence are
    // present-tense facts — the log can only say what was true when it was
    // written, and acting on that would offer to send a draft the owner already
    // discarded. A failure here costs the controls, never the document.
    const [entries, live] = await Promise.all([
      loadEntries(row.id),
      commsLines(user.id, row.agentModel).catch((e: unknown) => {
        console.error(`[daily-brief] live comms state failed for ${user.id}:`, e)
        return [] as CommsLine[]
      }),
    ])
    const comms: CommsState[] = live.map((line) => ({
      sourceKey: line.key,
      channelId: line.channelId,
      delegated: line.delegated,
      draft: line.draft && !line.draft.stale ? { id: line.draft.id, content: line.draft.content, stale: false } : line.draft,
    }))
    return view(row, entries, agent, comms)
  }
  if (!agent.configured) return { absent: 'no-agent', nextAt: null, agent }
  return { absent: due ? 'none' : 'pending', nextAt: nextBriefAt(config, zone, at).toISOString(), agent }
}

/** Move the reader's cursor. The only update this feature performs on a brief,
 *  and it describes the READER, not the document. Monotonic — `greatest` — so a
 *  second tab that loaded an older page cannot walk the cursor backwards and
 *  re-flag everything as new. */
export async function markBriefRead(userId: string, briefId: string, seq: number): Promise<void> {
  const sql = await db()
  await sql`
    update daily_briefs set read_seq = greatest(read_seq, ${seq})
    where id = ${briefId} and user_id = ${userId}
  `
}

// ── The single writer ────────────────────────────────────────────────────────

/** Append rows. THE ONLY WRITE PATH INTO A BRIEF'S CONTENT.
 *
 *  Seq is claimed by `update ... returning`, not by `max(seq) + 1`, and the
 *  difference matters: two sweeps can overlap (a scheduler tick and a realtime
 *  nudge land together), and a read-then-write would hand both the same number.
 *  The row update is atomic, so the loser gets the next block. The unique index
 *  on (brief_id, seq) is the backstop that turns any remaining race into a
 *  failed insert rather than a duplicated line. */
async function appendEntries(briefId: string, userId: string, rows: NewEntry[]): Promise<BriefEntry[]> {
  if (rows.length === 0) return []
  const sql = await db()

  const claimed = (await sql`
    update daily_briefs set last_seq = last_seq + ${rows.length}
    where id = ${briefId} returning last_seq as "lastSeq"
  `) as unknown as Array<{ lastSeq: number }>
  const end = claimed[0]?.lastSeq
  if (end === undefined) throw new Error('brief disappeared while appending')
  const start = end - rows.length
  // One id for this whole append. Minted here rather than defaulted in the
  // schema because the batch IS this call — a per-row default would give every
  // row its own and the timeline would be a flat list again.
  const batch = randomUUID()

  const inserted: BriefEntry[] = []
  for (const [i, row] of rows.entries()) {
    const out = (await sql`
      insert into daily_brief_entries
        (brief_id, seq, batch, kind, section, source_key, source_type, source_id, source_href,
         fingerprint, supersedes, priority, status_label, badge, title, body, evidence)
      values (${briefId}, ${start + i + 1}, ${batch}, ${row.kind}, ${row.section}, ${row.sourceKey ?? null},
              ${row.sourceType ?? null}, ${row.sourceId ?? null}, ${row.sourceHref ?? null},
              ${row.fingerprint ?? null}, ${row.supersedes ?? null}, ${row.priority ?? null},
              ${row.statusLabel ?? null}, ${row.badge ? sql.json(row.badge as never) : null},
              ${row.title ?? ''}, ${row.body ?? ''}, ${sql.json((row.evidence ?? []) as never)})
      returning id, seq, batch, kind, section, source_key as "sourceKey", source_type as "sourceType",
                source_id as "sourceId", source_href as "sourceHref", fingerprint, supersedes,
                priority, status_label as "statusLabel", badge, title, body, evidence,
                created_at as "createdAt"
    `) as unknown as BriefEntry[]
    if (out[0]) inserted.push(out[0])
  }

  // The mirror and the nudge are both DERIVED, so neither may take the append
  // down with it. A brief whose artifact failed to re-render is a brief with a
  // stale share link; a brief that failed to append is a brief that lost a day.
  void mirrorBriefArtifact(briefId, userId).catch((e: unknown) =>
    console.error(`[daily-brief] artifact mirror failed for ${briefId}:`, e),
  )
  publishUser(userId, { type: 'brief', briefId, seq: end })
  return inserted
}

// ── Opening ──────────────────────────────────────────────────────────────────

/** Open today's brief. Idempotent: the unique key on (user_id, brief_date) is
 *  what makes a double-firing tick a no-op rather than a second document, and
 *  the `returning` tells us which of the two we are. */
export async function openBrief(user: SessionUser, at = new Date()): Promise<{ opened: boolean; briefId: string | null }> {
  const config = await briefConfig()
  const zone = zoneFor(user.id, config)
  const { date } = briefWindow(config, zone, at)
  const agent = await briefAssistant(user.id)
  // No assistant, no brief. NOT a degraded brief written by some other model —
  // see `briefAssistant`. The surface renders the setup card instead.
  if (!agent.configured) return { opened: false, briefId: null }

  const sql = await db()
  const created = (await sql`
    insert into daily_briefs (user_id, brief_date, zone, agent_model, agent_name)
    values (${user.id}, ${date}, ${zone}, ${agent.model}, ${agent.name})
    on conflict (user_id, brief_date) do nothing
    returning id
  `) as unknown as Array<{ id: string }>
  const briefId = created[0]?.id
  if (!briefId) return { opened: false, briefId: (await loadRow(user.id, date))?.id ?? null }

  const { candidates, calendar } = await snapshot(user, zone, agent.model)
  const items = [...(calendar ?? []), ...candidates]

  // The lede is written BEFORE the items are appended and inserted FIRST, so
  // seq 1 is always the opening read. It is also the only model call on this
  // path that is allowed to fail quietly: a brief with no lede is a brief; a
  // brief with no items is an empty page.
  const lede = await writeLede(agent, items, date, zone).catch(() => null)
  await appendEntries(briefId, user.id, [
    {
      kind: 'lede',
      section: 'action',
      title: `Daily brief — ${date}`,
      body: lede ?? fallbackLede(items),
    },
    ...items,
  ])
  return { opened: true, briefId }
}

/** What the brief says when the model could not be reached.
 *
 *  A COUNT, NOT AN APOLOGY. The items are already on the page and they are the
 *  real content; the lede is synthesis on top. A failed model call must not
 *  turn into "your assistant is unavailable" at the head of a document that is
 *  otherwise complete and correct. */
function fallbackLede(items: NewEntry[]): string {
  const n = (section: BriefSection): number => items.filter((i) => i.section === section && i.kind === 'item').length
  const parts = [
    n('action') ? `${n('action')} thing${n('action') === 1 ? '' : 's'} need you` : '',
    n('comms') ? `${n('comms')} conversation${n('comms') === 1 ? '' : 's'} waiting` : '',
    n('schedule') ? `${n('schedule')} on your calendar` : '',
  ].filter(Boolean)
  return parts.length ? `${parts.join(', ')}.` : 'Nothing is waiting on you this morning.'
}

async function writeLede(agent: BriefAssistant, items: NewEntry[], date: string, zone: string): Promise<string | null> {
  if (!agent.model) return null
  const run = await runHarness(
    dailyBriefLedeHarness,
    {
      date,
      zone,
      lines: items.filter((i) => i.kind === 'item').map((i) => `[${i.section}] ${i.title}${i.body ? ` — ${i.body}` : ''}`),
    },
    { caller: 'briefer:daily-open', model: agent.model },
  )
  return run.value ?? null
}

// ── Following the day ────────────────────────────────────────────────────────

export interface SweepResult {
  appended: number
  added: number
  changed: number
  resolved: number
}

/** Recompute the sources and append what moved. NEVER rewrites.
 *
 *  Three verbs and one refusal:
 *    · a key the log has never seen         → append 'item'
 *    · a key whose fingerprint moved        → append 'change', superseding
 *    · a key that is gone from the sources  → append 'resolved', superseding
 *    · a key whose SOURCE FAILED TO LOAD    → append nothing at all
 *
 *  That last one is the load-bearing branch. A resolution is permanent — there
 *  is no row to take back — so "the approvals query 500'd" must never be
 *  allowed to look like "every approval was handled". `snapshot` reports which
 *  source types failed and this pass skips resolutions for exactly those,
 *  leaving the lines standing until a pass that could actually read them says
 *  otherwise. */
export async function sweepBrief(user: SessionUser, at = new Date()): Promise<SweepResult> {
  const none: SweepResult = { appended: 0, added: 0, changed: 0, resolved: 0 }
  const config = await briefConfig()
  const zone = zoneFor(user.id, config)
  const { date } = briefWindow(config, zone, at)
  const row = await loadRow(user.id, date)
  if (!row) return none

  const [entries, first] = await Promise.all([loadEntries(row.id), snapshot(user, zone, row.agentModel)])

  // DRAFT FIRST, THEN LOOK AT THE WORLD. Drafting used to run after the diff,
  // which was wrong in a way only visible with a standing grant: the assistant
  // SENT a reply during the sweep, and because the snapshot had already been
  // taken the line still read "1 unread" for another five minutes — the brief
  // reporting a conversation as waiting that its own assistant had just
  // answered. Re-reading costs one query on the sweeps where anything was
  // drafted, and nothing at all on the ones where nothing was.
  // A grant made since the last sweep can have parked drafts waiting behind it —
  // see `releaseDrafts`. Released BEFORE drafting, so a thread whose reply has
  // just gone out is not also drafted for.
  const released = await releaseDrafts(user.id).catch((e: unknown) => {
    console.error(`[daily-brief] releasing drafts failed for ${user.id}:`, e)
    return 0
  })
  const freshDrafts = await draftPending(user, row, first.comms).catch((e: unknown) => {
    console.error(`[daily-brief] drafting replies failed for ${user.id}:`, e)
    return 0
  })
  const drafted = released + freshDrafts
  const { candidates, failedTypes, calendar } = drafted > 0 ? await snapshot(user, zone, row.agentModel) : first
  const { lines } = foldEntries(entries, row.readSeq)
  const known = new Map(lines.map((l) => [l.key, l]))
  // No `kind` filter: `calendarEntries` returns only items now (the unreadable
  // case included, as a keyed line). Filtering here would have dropped that
  // line out of `live` and made every sweep resolve it and re-add it.
  const live = [...(calendar ?? []), ...candidates]
  const liveKeys = new Set(live.map((c) => c.sourceKey).filter((k): k is string => !!k))

  const appends: NewEntry[] = []
  let added = 0
  let changed = 0
  let resolved = 0

  for (const candidate of live) {
    if (!candidate.sourceKey) continue
    const line = known.get(candidate.sourceKey)
    if (!line) {
      // A SOURCE-EMITTED RESOLUTION FOR A LINE THAT WAS NEVER HERE IS DROPPED.
      // The comms source reports answered conversations so a line can close
      // with WHO answered it rather than a generic DONE — but a thread answered
      // before the brief ever mentioned it has nothing to close, and appending
      // it would put "Replied to Sam" on a page that never asked you to.
      if (candidate.kind === 'resolved') continue
      appends.push(candidate)
      added++
      continue
    }
    // Unchanged, or back after a resolution. Both are decided by the
    // fingerprint against the LAST entry for the key, which is what makes a
    // thing that resolves and returns append a fresh row rather than nothing.
    if (line.current.fingerprint === candidate.fingerprint && line.resolved === (candidate.kind === 'resolved')) continue
    if (candidate.kind === 'resolved') {
      if (line.resolved) continue
      appends.push({ ...candidate, supersedes: line.current.id })
      resolved++
      continue
    }
    appends.push({ ...candidate, kind: 'change', supersedes: line.current.id })
    changed++
  }

  for (const line of lines) {
    if (line.resolved || liveKeys.has(line.key)) continue
    // A conversation NEVER resolves by vanishing. `commsLines` reports answered
    // threads explicitly (with who answered), so a comms key that is absent
    // from the live set means the source could not see it — an archived
    // channel, a failed read — and closing it here would be the original bug
    // wearing a different hat: a line marked done that nobody answered.
    if (line.current.sourceType === 'channel') continue
    const type = line.current.sourceType ?? ''
    // Calendar has no failure namespace of its own — a schedule that could not
    // be read comes back as `null`, not as an empty list — so a null calendar
    // means "do not resolve calendar lines either".
    if (failedTypes.has(type) || (type === 'calendar' && calendar === null)) continue
    appends.push({
      kind: 'resolved',
      section: line.section,
      sourceKey: line.key,
      sourceType: line.current.sourceType,
      sourceId: line.current.sourceId,
      sourceHref: line.current.sourceHref,
      fingerprint: null,
      supersedes: line.current.id,
      priority: 'ok',
      statusLabel: 'DONE',
      title: line.current.title,
      body: '',
    })
    resolved++
  }

  const sql = await db()
  if (appends.length === 0) {
    await sql`update daily_briefs set last_swept_at = now() where id = ${row.id}`
    return none
  }

  // ONE narration per batch, and it goes in FIRST so the fold finds it at the
  // head of the group. The model sees only what this pass appended — never the
  // whole document — because a note is about the change, and handing it the day
  // so far is how a delta line turns into a second, competing summary.
  const note = await writeNote(row, appends).catch(() => null)
  const written = await appendEntries(row.id, user.id, note ? [note, ...appends] : appends)
  await sql`update daily_briefs set last_swept_at = now() where id = ${row.id}`
  return { appended: written.length, added, changed, resolved }
}

/** Ask the assistant to write replies for the conversations still waiting.
 *
 *  WHY DRAFTING NEEDS NO PERMISSION AND SENDING DOES. A draft is a suggestion
 *  sitting on the owner's own page; nothing has left the building, and they
 *  read it before anyone else does. That is the same posture
 *  `google/pending-actions.ts` takes with outbound mail — "reads and drafts are
 *  free". `draftReply` is what checks for a grant, and it is the only thing
 *  that can turn a draft into a sent message. */
async function draftPending(user: SessionUser, row: BriefRow, comms: CommsLine[]): Promise<number> {
  if (!row.agentModel) return 0
  // Bounded twice over: only threads where somebody is genuinely waiting, and
  // only those without a live draft already. Without both, every unanswered DM
  // would be re-drafted every five minutes — a model call per thread per sweep,
  // and a page that rewrites itself under the reader.
  const waiting = comms.filter((c) => c.answeredBy === null && (!c.draft || c.draft.stale))
  if (waiting.length === 0) return 0
  const owner = user.name ?? user.email ?? 'your owner'
  let wrote = 0
  // Sequential: each is a model call, and a person with twenty unanswered DMs
  // should not fan twenty concurrent turns at their own assistant.
  if (waiting.length > DRAFT_LIMIT) {
    // SAID OUT LOUD. A silent cap reads as "every waiting thread was handled",
    // which is the class of quiet truncation this whole surface exists to avoid.
    console.warn(
      `[daily-brief] ${waiting.length} conversations waiting for ${user.id}; drafting ${DRAFT_LIMIT} this pass, the rest on following sweeps`,
    )
  }
  for (const line of waiting.slice(0, DRAFT_LIMIT)) {
    const result = await draftReply({
      userId: user.id,
      channelId: line.channelId,
      peer: line.peer,
      awaitingSeq: line.awaitingSeq,
      agentModel: row.agentModel,
      ownerName: owner,
    }).catch((e: unknown) => {
      // One thread failing to draft leaves that thread waiting, which the line
      // already renders honestly. It must not stop the other threads.
      console.error(`[daily-brief] draft failed for channel ${line.channelId}:`, e)
      return null
    })
    if (result) wrote++
  }
  return wrote
}

/** How many conversations one sweep will draft for.
 *
 *  A ceiling rather than a queue, and it is LOGGED when it bites (see
 *  `sweepBrief`'s caller): a person who comes back from leave with forty
 *  unanswered DMs should not have their assistant spend forty model calls in
 *  one tick. The rest are drafted on following sweeps, and every one of them is
 *  still visible on the page as waiting in the meantime. */
const DRAFT_LIMIT = 8

async function writeNote(row: BriefRow, appends: NewEntry[]): Promise<NewEntry | null> {
  if (!row.agentModel) return null
  const run = await runHarness(
    dailyBriefNoteHarness,
    {
      changes: appends.map((a) => `${a.kind}: ${a.title}${a.body ? ` — ${a.body}` : ''}`),
    },
    { caller: 'briefer:daily-delta', model: row.agentModel },
  )
  if (!run.value) return null
  return { kind: 'note', section: 'action', title: '', body: run.value }
}

// ── The scheduled pass ───────────────────────────────────────────────────────

/** Users who could have a brief: everyone with an enabled personal assistant.
 *  Scoped by that rather than by `users`, because a person with no assistant
 *  has nothing that can write one and sweeping them is pure cost. */
async function briefableUsers(): Promise<SessionUser[]> {
  const sql = await db()
  const rows = (await sql`
    select u.id, u.email, u.name, u.role
    from users u join agent_defs d on d.owner_user_id = u.id and d.enabled
    order by u.created_at asc
  `) as unknown as Array<{ id: string; email: string | null; name: string | null; role: 'admin' | 'member' }>
  // `sub`, `picture` and `provider` are session fields the source functions
  // never read — only `id` and `role` (which gates org-wide approvals) matter
  // here, and filling the rest with nulls is honest about that.
  return rows.map((r) => ({ id: r.id, sub: '', email: r.email, name: r.name, picture: null, provider: 'local' as never, role: r.role }))
}

export interface BriefPassResult {
  opened: number
  swept: number
  appended: number
  failed: number
}

/** One scheduler tick: open what is due, then follow what is open.
 *
 *  Sequential, not `Promise.all`. Each person costs four scoped queries and up
 *  to one model call, and a workspace-wide fan-out would hit the connection
 *  pool (max 10) and the gateway at the same instant every five minutes. The
 *  job's `maxRunMs` is what makes a pass that gets too slow VISIBLE, which is
 *  the right way for this to fail. */
export async function runBriefPass(at = new Date()): Promise<BriefPassResult> {
  const config = await briefConfig()
  const result: BriefPassResult = { opened: 0, swept: 0, appended: 0, failed: 0 }
  const users = await briefableUsers()

  for (const user of users) {
    try {
      const zone = zoneFor(user.id, config)
      const { due, date } = briefWindow(config, zone, at)
      const existing = await loadRow(user.id, date)
      if (!existing) {
        if (!due) continue
        const { opened } = await openBrief(user, at)
        if (opened) result.opened++
        continue
      }
      if (!sweepDue(existing, config, at)) continue
      const swept = await sweepBrief(user, at)
      result.swept++
      result.appended += swept.appended
    } catch (e: unknown) {
      result.failed++
      console.error(`[daily-brief] pass failed for ${user.id}:`, e)
    }
  }
  return result
}

const sweepDue = (row: BriefRow, config: BriefConfig, at: Date): boolean =>
  !row.lastSweptAt || at.getTime() - new Date(row.lastSweptAt).getTime() >= config.sweepMinutes * 60_000

/** Sweep one person NOW, on demand. Used by the surface when the reader opens
 *  it and by the realtime nudge — both want "is it current" answered before the
 *  next tick, and both are rate-limited by `sweepDue` so an open tab cannot
 *  turn into a query loop. */
export async function sweepIfDue(user: SessionUser, at = new Date()): Promise<SweepResult | null> {
  const config = await briefConfig()
  const zone = zoneFor(user.id, config)
  const { date } = briefWindow(config, zone, at)
  const row = await loadRow(user.id, date)
  if (!row || !sweepDue(row, config, at)) return null
  return sweepBrief(user, at)
}

// ── The schedule ─────────────────────────────────────────────────────────────
//
// A TALARIA JOB, NOT A HERMES CRON, and the choice is the one `agent-crons.ts`
// argues for in its own header. A `hermes cron` fires the agent's loop on the
// PERSONA gateway key, which the gateway leaves unmetered because a Talaria
// flow normally writes that turn's ledger row — a cron has no flow, so its
// spend reaches no ledger. A brief is one model call per person per morning
// plus one per sweep; across a workspace that is real money going nowhere
// visible. Driving the schedule from here dispatches through `runHarness` like
// any other turn, which meters and attributes it for free.
//
// It also fails in the right direction. A Hermes cron needs the owner's
// container to be up at 07:00 — so the person whose agent is mid-restart
// silently gets no brief, and nothing anywhere says so. This job runs
// regardless, and a failed pass is a scheduler failure an operator can see on
// /observability.
registerJob({
  name: 'daily-brief',
  // Five minutes, and the number is doing two jobs. It is the granularity of
  // the OPEN (a brief can be at most five minutes late), and it is the tick the
  // per-brief `sweepMinutes` throttle rides on, so a sweep interval below five
  // minutes has no effect without changing this too.
  everyMs: 5 * 60_000,
  // Let a deploy settle before the first pass. A crash-looping instance must
  // never reach a job that writes a document to every person in the workspace.
  firstRunDelayMs: 90_000,
  // Generous, because the pass is sequential across everyone with an assistant
  // and each person may cost a model call. Past this it is not slow, it is
  // stuck, and `unhealthyJobs()` should say so.
  maxRunMs: 10 * 60_000,
  run: async () => {
    const r = await runBriefPass()
    if (!r.opened && !r.swept && !r.failed) return null
    const line = `opened ${r.opened} brief(s), swept ${r.swept}, appended ${r.appended} entr(ies)`
    // A pass with failures is a FAILED run, not a run with a footnote. The
    // per-user error is already logged with its id; this is what makes the
    // pattern — "it has been failing for six people since Tuesday" — reach an
    // operator instead of scrolling past in a log.
    if (r.failed) throw new Error(`${line} — and FAILED for ${r.failed} user(s); see the [daily-brief] errors above`)
    return line
  },
})
