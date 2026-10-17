// The transcript's compaction marker: whether to draw it, and what it says.
//
// Pulled out of the component because the copy is the delicate part. Hermes
// compacts a pinned session by replacing a run of messages with a carrier the
// api projects as a HIDDEN row — id, timestamp and role kept, content empty.
// No summary text crosses the wire, so this row may never imply one exists:
// "compacted" is the whole claim, and "folded" is what happened to the
// messages. A line reading "the agent summarized the earlier conversation"
// would be describing something no surface here can show.
import { formatShortDate } from '@/lib/format'
import type { SessionOverview } from '@/lib/conversations.svelte'

/** Draw the row only for a conversation the question arose for AND where
 *  something actually happened. `applies` alone is not enough — a pinned
 *  session that has never crossed its threshold has nothing to report, and a
 *  row saying so would be noise at the top of every thread. */
export function showCompactedRow(o: SessionOverview | null | undefined): boolean {
  return !!o && o.applies && o.compacted > 0
}

/** The pill's one line. Clauses are separated by ` · `, the same way the rest
 *  of the chat chrome joins a fact to its qualifiers.
 *
 *  `shortDate` is injectable so the sentence is testable without a clock:
 *  the default spelling (`formatShortDate`) drops the year for the current
 *  one, which would make a fixed-date assertion change meaning in January. */
export function compactedSentence(
  o: SessionOverview,
  shortDate: (iso: string) => string = formatShortDate,
): string {
  const parts = ['Earlier in this session was compacted by the agent']
  // A floor when the api read one page of a longer session: the number is
  // true as "at least", and saying so costs one word.
  if (o.compacted > 0) {
    parts.push(`${o.partial ? 'at least ' : ''}${o.compacted} ${o.compacted === 1 ? 'message' : 'messages'} folded`)
  }
  // Hermes' timestamp rides through verbatim; a spelling Date cannot read
  // drops the clause rather than printing "Invalid Date" into the pill.
  if (o.since && !Number.isNaN(Date.parse(o.since))) parts.push(`since ${shortDate(o.since)}`)
  return parts.join(' · ')
}

/** The hover text: the identifiers, kept out of the visible line. The session
 *  id is the Hermes session (which for a pinned agent IS the conversation
 *  id), so it is what someone goes looking with. Message count is Hermes'
 *  own total for the session, absent on an api that could not read it. */
export function compactedTitle(o: SessionOverview): string {
  const parts: string[] = []
  if (o.sessionId) parts.push(`session ${o.sessionId}`)
  if (o.messageCount !== null) parts.push(`${o.messageCount} messages in the session`)
  return parts.join(' · ')
}
