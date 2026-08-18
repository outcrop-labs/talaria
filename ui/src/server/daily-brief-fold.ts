// The fold: an append-only log replayed into the document a person reads.
//
// PURE, AND IN ITS OWN FILE FOR TWO REASONS. It is the one piece of the daily
// brief with no database, no clock and no model in it, which makes it the piece
// worth testing directly — and `sweepBrief` computes its diff against the FOLD
// rather than against a snapshot it wrote earlier, so a bug here is a bug in
// change detection, not just in rendering.
//
// The second reason is duller: `daily-brief-artifact.ts` renders the same fold
// to markdown, and importing it from `daily-brief.ts` would close a cycle
// between the two.
import { isTerminal, type BriefEntry, type BriefLine, type BriefUpdate } from './daily-brief-types'

/** Replay the log into the document.
 *
 *  This is the only place the current state of a brief is computed, and it is a
 *  pure function of the rows. That is what lets `sweepBrief` diff against the
 *  FOLD rather than against a snapshot it wrote earlier: a sweep that died
 *  half-way leaves a shorter log, never a wrong one, and the next sweep sees
 *  exactly what is missing. */
export function foldEntries(entries: BriefEntry[], readSeq: number): { lines: BriefLine[]; updates: BriefUpdate[] } {
  const ordered = [...entries].sort((a, b) => a.seq - b.seq)
  const byKey = new Map<string, BriefLine>()

  for (const entry of ordered) {
    if (!entry.sourceKey) continue
    const existing = byKey.get(entry.sourceKey)
    if (existing) {
      existing.history.push(entry)
      existing.current = entry
      existing.resolved = isTerminal(entry.kind)
      existing.unseen = existing.unseen || entry.seq > readSeq
      continue
    }
    byKey.set(entry.sourceKey, {
      key: entry.sourceKey,
      section: entry.section,
      current: entry,
      history: [entry],
      resolved: isTerminal(entry.kind),
      unseen: entry.seq > readSeq,
    })
  }

  // The day, batched. Entries written by one append share a `batch`, and
  // grouping on it is what turns a flat log into "at 11:04, three things moved"
  // — the shape a person actually reads a day in.
  //
  // The fallback is for rows written before `batch` existed: truncating
  // `created_at` to the second was the original grouping, and it is right
  // except when two appends land in the same second, which is exactly why the
  // column was added. Keeping it means an existing brief still renders as a
  // timeline rather than as one undifferentiated blob.
  const batches = new Map<string, BriefEntry[]>()
  for (const entry of ordered) {
    if (entry.kind === 'lede') continue
    const bucket = entry.batch ?? `t:${entry.createdAt.slice(0, 19)}`
    const list = batches.get(bucket)
    if (list) list.push(entry)
    else batches.set(bucket, [entry])
  }

  const updates: BriefUpdate[] = [...batches.values()]
    .map((group) => {
      const note = group.find((e) => e.kind === 'note' && !e.sourceKey)
      return {
        seq: Math.max(...group.map((e) => e.seq)),
        at: group[0]!.createdAt,
        note: note?.body || note?.title || null,
        entries: group.filter((e) => e !== note),
      }
    })
    .filter((u) => u.entries.length > 0 || u.note !== null)
    .sort((a, b) => b.seq - a.seq)

  return { lines: [...byKey.values()], updates }
}

