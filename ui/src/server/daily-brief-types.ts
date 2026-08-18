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
export const BRIEF_ENTRY_KINDS = ['lede', 'item', 'change', 'resolved', 'note'] as const
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

/** No brief today — and WHICH kind of nothing, because the three render
 *  differently and collapsing them is how a surface lies.
 *    'pending'   the brief hour has not arrived yet; here is when it will
 *    'no-agent'  no personal assistant, so nothing can write one
 *    'none'      the hour passed and no brief exists (a missed run) */
export interface BriefAbsent {
  absent: 'pending' | 'no-agent' | 'none'
  /** ISO of the next brief opening, when that is knowable. */
  nextAt: string | null
  agent: BriefAssistant
}

export type BriefResponse = BriefView | BriefAbsent

// Re-exported, not defined: the one definition lives in `@/lib/brief-absent`
// so the browser can reach it without importing a value from `@/server/`.
export { isBriefAbsent } from '@/lib/brief-absent'
