// The daily brief's wire shapes. Split from `daily-brief.ts` for the reason
// `inbox-focus-types.ts` is split from `inbox-focus.ts`: the client imports
// these, and a client that imports the module holding `db()` drags the whole
// server graph into the browser bundle.

/** The five places a line can land in the document. Ordered as the document
 *  reads, and that order is load-bearing — `foldBrief` sorts sections by this
 *  array, so a section added in the middle moves the document, not just a
 *  switch statement somewhere. */
export const BRIEF_SECTIONS = ['action', 'schedule', 'comms', 'highlights'] as const
export type BriefSection = (typeof BRIEF_SECTIONS)[number]

/** What an appended row IS.
 *
 *  These are not statuses on a mutable row — they are the verbs of an
 *  append-only log, and each one is a row that exists forever:
 *    lede      the assistant's opening read (seq 1, exactly once per brief)
 *    item      a source appearing for the first time today
 *    change    the same source, materially different — supersedes the last
 *    resolved  the source stopped needing the owner — supersedes the last
 *    note      the assistant narrating a batch of appends */
export const BRIEF_ENTRY_KINDS = ['lede', 'item', 'change', 'resolved', 'checked', 'dismissed', 'note'] as const
export type BriefEntryKind = (typeof BRIEF_ENTRY_KINDS)[number]

export type BriefPriority = 'p0' | 'p1' | 'p2' | 'ok'

export interface BriefBadge {
  label: string
  tone: 'neutral' | 'accent' | 'success' | 'warn' | 'danger'
}

export interface BriefEvidence {
  label: string
  text: string
}

/** One appended row, verbatim. Never edited after insert. */
export interface BriefEntry {
  id: string
  seq: number
  /** Which append wrote this row. Entries sharing a batch were learned in the
   *  same sweep and are shown as one moment in the timeline. Null on rows
   *  written before the column existed. */
  batch: string | null
  kind: BriefEntryKind
  section: BriefSection
  sourceKey: string | null
  sourceType: string | null
  sourceId: string | null
  /** Where in Talaria this line points. THE POINT OF THE SURFACE — a brief
   *  that names a blocked ticket and cannot open it is a newsletter. */
  sourceHref: string | null
  fingerprint: string | null
  supersedes: string | null
  priority: BriefPriority | null
  statusLabel: string | null
  badge: BriefBadge | null
  title: string
  body: string
  evidence: BriefEvidence[]
  createdAt: string
}

/** A source folded to its CURRENT state — the newest entry for a key, plus
 *  the trail of every earlier entry for it.
 *
 *  `history` is why the fold exists rather than a `select distinct on`. The
 *  document shows where a thing stands; the thread underneath shows how it got
 *  there, and both are read off the same log. */
export interface BriefLine {
  key: string
  section: BriefSection
  /** The newest entry for this key — what the row renders as right now. */
  current: BriefEntry
  /** Every entry for this key, oldest first, `current` last. */
  history: BriefEntry[]
  /** Superseded by a 'resolved' entry: struck through, kept on the page. */
  resolved: boolean
  /** Changed since the reader's `read_seq`. Drives the "new" affordance. */
  unseen: boolean
}

/** An append event, as the day's timeline shows it: one sweep's worth of
 *  entries under the assistant's note about them. */
export interface BriefUpdate {
  seq: number
  at: string
  /** The assistant's line about this batch, when it wrote one. */
  note: string | null
  entries: BriefEntry[]
}

/** The parts of a conversation line that are LIVE rather than historical.
 *
 *  Deliberately not in the log. An entry records what was true when it was
 *  written — that a draft existed, that a thread was delegated — and those are
 *  exactly the facts that must not be acted on from history: approving a draft
 *  the owner discarded an hour ago, or showing a "revoke" control for a grant
 *  that is already revoked. The log stays append-only; this is read fresh on
 *  every request and keyed back onto the lines by `sourceKey`. */
export interface CommsState {
  sourceKey: string
  channelId: string
  /** The assistant may answer here without asking. */
  delegated: boolean
  /** A reply waiting for the owner's decision. */
  draft: { id: string; content: string; stale: boolean } | null
}

export interface BriefAssistant {
  configured: boolean
  model: string | null
  name: string | null
}

/** The whole surface, folded. */
export interface BriefView {
  id: string
  /** Local calendar date the brief belongs to (YYYY-MM-DD). */
  date: string
  zone: string
  openedAt: string
  /** The assistant's opening read. Written once, never rewritten — a brief
   *  whose lede changed at noon would be a different document. */
  lede: string
  agent: BriefAssistant
  artifactId: string | null
  /** Current state of every source, grouped by section, in document order. */
  sections: Array<{ section: BriefSection; lines: BriefLine[] }>
  /** The day as it accumulated, newest batch first. */
  updates: BriefUpdate[]
  /** Live conversation state, keyed onto `sections` lines by `sourceKey`. */
  comms: CommsState[]
  lastSeq: number
  readSeq: number
  /** Appended since the reader last looked. */
  unseenCount: number
  lastSweptAt: string | null
}

/** No document yet — and WHICH kind of not-yet, because the kinds render
 *  differently and collapsing them is how a surface lies.
 *    'pending'   the brief hour has not arrived yet; here is when it will
 *    'no-agent'  no personal assistant, so nothing can write one
 *    'writing'   the document is being written right now — either the scheduled
 *                open is mid-flight, or the reader arrived past the due hour
 *                with no brief and the read itself kicked one off (see
 *                getBrief). The surface polls fast and the append publishes,
 *                so this state lasts seconds, not a day. */
export interface BriefAbsent {
  absent: 'pending' | 'no-agent' | 'writing'
  /** ISO of the next brief opening, when that is knowable. */
  nextAt: string | null
  agent: BriefAssistant
}

export type BriefResponse = BriefView | BriefAbsent

/** The owner's three verdicts on a line (POST /api/brief/item). */
export type BriefMarkAction = 'check' | 'dismiss' | 'restore'

/** What POST /api/brief/item answers on success: the page exactly as
 *  GET /api/brief would serve it now, read after the mark. `brief` is absent
 *  only when that read failed — the mark itself landed. */
export interface BriefMarkAnswer {
  ok: true
  brief?: BriefResponse
}

/** The kinds that close a line: the source saying it is done, or the owner
 *  checking it off or dismissing it (the engine's `is_terminal`). */
const TERMINAL_KINDS: ReadonlySet<string> = new Set(['resolved', 'checked', 'dismissed'])

const PRIORITY_RANK: Readonly<Record<string, number>> = { p0: 0, p1: 1, p2: 2 }
const priorityRank = (p: string | null): number => PRIORITY_RANK[p ?? ''] ?? 3

/** The document's line order, as the engine sorts a section (view.rs):
 *  unresolved first, then priority, then where the log first put the line. */
function byDocumentOrder(a: BriefLine, b: BriefLine): number {
  return (
    Number(a.resolved) - Number(b.resolved) ||
    priorityRank(a.current.priority) - priorityRank(b.current.priority) ||
    (a.history[0]?.seq ?? 0) - (b.history[0]?.seq ?? 0)
  )
}

/** A line as the owner's verdict leaves it — the row `mark_brief_item` will
 *  append, folded on — or null when the verdict changes nothing (checking off
 *  a closed line, restoring an open one) or there is nothing to restore to.
 *
 *  THE SAME LITERAL THE ENGINE WRITES. A check-off or a dismissal is the
 *  closing row: priority `ok`, the CHECKED OFF / DISMISSED label, no badge, no
 *  body, no evidence, the fingerprint carried forward. A restore is a
 *  `change` carrying the last LIVE row in the line's history. Only the id,
 *  seq and time are the browser's stand-ins until the server's row replaces
 *  them. */
export function markedLine(line: BriefLine, action: BriefMarkAction, at: string, seq: number): BriefLine | null {
  const current = line.current
  const stand = { id: `pending:${line.key}:${seq}`, seq, batch: null, supersedes: current.id, createdAt: at }
  let entry: BriefEntry
  if (action === 'restore') {
    if (!line.resolved) return null
    const live = [...line.history].reverse().find((e) => !TERMINAL_KINDS.has(e.kind))
    if (!live) return null
    entry = { ...live, ...stand, kind: 'change', sourceKey: live.sourceKey ?? '' }
  } else {
    if (line.resolved) return null
    entry = {
      ...current,
      ...stand,
      kind: action === 'check' ? 'checked' : 'dismissed',
      priority: 'ok',
      statusLabel: action === 'check' ? 'CHECKED OFF' : 'DISMISSED',
      badge: null,
      body: '',
      evidence: [],
    }
  }
  return { ...line, current: entry, history: [...line.history, entry], resolved: action !== 'restore', unseen: true }
}

/** The page with one line marked (see `markedLine`), its section re-sorted
 *  the way the engine sorts it. The same object back when nothing changes —
 *  an absence, a key not on the page, a verdict that is already true. */
export function withMark(page: BriefResponse, key: string, action: BriefMarkAction, at: string): BriefResponse {
  if ('absent' in page) return page
  let changed = false
  const sections = page.sections.map((s) => {
    const i = s.lines.findIndex((l) => l.key === key)
    if (i < 0) return s
    const marked = markedLine(s.lines[i]!, action, at, page.lastSeq + 1)
    if (!marked) return s
    changed = true
    const lines = s.lines.map((l, j) => (j === i ? marked : l)).sort(byDocumentOrder)
    return { ...s, lines }
  })
  return changed ? { ...page, sections } : page
}

/** The page with one line put back as `from` had it — the undo of an
 *  optimistic mark whose POST failed. Line-level, so a second mark still in
 *  flight on another line keeps its own state, and everything else stays as
 *  current as `page` is. `page` unchanged when the two are different
 *  documents or either lacks the line: the caller refetches either way. */
export function withLineFrom(page: BriefResponse, from: BriefResponse | undefined, key: string): BriefResponse {
  if (!from || 'absent' in page || 'absent' in from || page.id !== from.id) return page
  const original = from.sections.flatMap((s) => s.lines).find((l) => l.key === key)
  if (!original) return page
  let changed = false
  const sections = page.sections.map((s) => {
    const i = s.lines.findIndex((l) => l.key === key)
    if (i < 0) return s
    changed = true
    return { ...s, lines: s.lines.map((l, j) => (j === i ? original : l)).sort(byDocumentOrder) }
  })
  return changed ? { ...page, sections } : page
}

/** A realtime 'brief' event, as the brief's listeners read it. */
export interface BriefEventSignal {
  briefId: string
  seq: number
  /** The stale nudge, not an append: see `briefEventIsNews`. */
  stale?: boolean
}

/** Whether a realtime 'brief' event says something the cached page has not
 *  seen. An APPEND's event carries the brief's id and the seq it reached; a
 *  page of that brief already at or past that seq has it — the echo of a
 *  read that swept, or of the reader's own check-off whose answer carried the
 *  page — and refetching it would be a GET for nothing.
 *
 *  THE STALE NUDGE IS ALWAYS NEWS. `mark_brief_stale` (a notification landed,
 *  a thread moved, an approval was decided) clears the sweep throttle and
 *  publishes the brief's UNCHANGED seq: the page's lastSeq equals it by
 *  construction, and the re-read is the point — the read sweeps on the way
 *  through, which is how a new message's line appears and a replied one's
 *  closes. */
export function briefEventIsNews(cached: BriefResponse | undefined, event: BriefEventSignal): boolean {
  if (event.stale === true) return true
  if (!cached || 'absent' in cached) return true
  return cached.id !== event.briefId || cached.lastSeq < event.seq
}

// Re-exported, not defined: the one definition lives in `@/lib/brief-absent`
// so the browser can reach it without importing a value from `@/server/`.
export { isBriefAbsent } from '@/lib/brief-absent'
