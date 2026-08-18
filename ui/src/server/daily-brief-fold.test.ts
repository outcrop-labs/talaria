import { describe, expect, it } from 'vitest'
import { foldEntries } from './daily-brief-fold'
import type { BriefEntry } from './daily-brief-types'

// THE FOLD IS THE CHANGE DETECTOR, not just a renderer. `sweepBrief` diffs the
// live sources against this function's output, so every bug here is a bug in
// what gets appended — a line the fold loses is a line the next sweep re-adds
// as new, and a resolution the fold misses is a line that never comes back.
//
// It is also the only piece of the feature with no database, no clock and no
// model in it, which is why the assertions live here rather than behind a
// harness fixture.

let seq = 0
const entry = (over: Partial<BriefEntry>): BriefEntry => ({
  id: `e${++seq}`,
  seq,
  batch: null,
  kind: 'item',
  section: 'action',
  sourceKey: null,
  sourceType: null,
  sourceId: null,
  sourceHref: null,
  fingerprint: null,
  supersedes: null,
  priority: 'p1',
  statusLabel: null,
  badge: null,
  title: 'A thing',
  body: '',
  evidence: [],
  createdAt: '2026-08-17T07:00:00.000Z',
  ...over,
})

describe('foldEntries', () => {
  it('collapses a key to its newest entry and keeps the whole trail', () => {
    const first = entry({ sourceKey: 'task:1', title: 'Unblock Ledger migration', fingerprint: 'a' })
    const second = entry({ sourceKey: 'task:1', kind: 'change', title: 'Unblock Ledger migration', fingerprint: 'b', supersedes: first.id })

    const { lines } = foldEntries([first, second], 0)

    expect(lines).toHaveLength(1)
    expect(lines[0]!.current.id).toBe(second.id)
    // THE TRAIL IS THE PRODUCT, not debug data: "3 updates today" on a row is
    // the only thing that tells a reader this line has a history worth opening.
    expect(lines[0]!.history.map((h) => h.id)).toEqual([first.id, second.id])
  })

  it('marks a line resolved without dropping it from the document', () => {
    const first = entry({ sourceKey: 'task:1', fingerprint: 'a' })
    const done = entry({ sourceKey: 'task:1', kind: 'resolved', supersedes: first.id })

    const { lines } = foldEntries([first, done], 0)

    // A RESOLVED LINE STAYS. This is the append-only contract's whole visible
    // consequence: what somebody read at 08:00 is still findable at 18:00. A
    // fold that filtered these out would make the document lie by omission and
    // would also make the next sweep re-append the item as brand new.
    expect(lines).toHaveLength(1)
    expect(lines[0]!.resolved).toBe(true)
  })

  it('un-resolves a line that comes back, rather than starting a second one', () => {
    const first = entry({ sourceKey: 'task:1', fingerprint: 'a' })
    const done = entry({ sourceKey: 'task:1', kind: 'resolved', supersedes: first.id })
    const again = entry({ sourceKey: 'task:1', kind: 'change', fingerprint: 'c', supersedes: done.id })

    const { lines } = foldEntries([first, done, again], 0)

    expect(lines).toHaveLength(1)
    expect(lines[0]!.resolved).toBe(false)
    expect(lines[0]!.history).toHaveLength(3)
  })

  it('reads the log in seq order regardless of the order rows arrive in', () => {
    const first = entry({ sourceKey: 'task:1', fingerprint: 'a' })
    const done = entry({ sourceKey: 'task:1', kind: 'resolved', supersedes: first.id })

    // Postgres gives no ordering guarantee we have not asked for, and a fold
    // that trusted insertion order would report a resolved item as open the
    // first time a query came back the other way round.
    const { lines } = foldEntries([done, first], 0)

    expect(lines[0]!.current.id).toBe(done.id)
    expect(lines[0]!.resolved).toBe(true)
  })

  it('flags a line unseen when anything about it landed after the read cursor', () => {
    const first = entry({ sourceKey: 'task:1', seq: 1, fingerprint: 'a' })
    const changed = entry({ sourceKey: 'task:1', seq: 5, kind: 'change', fingerprint: 'b', supersedes: first.id })

    expect(foldEntries([first, changed], 5).lines[0]!.unseen).toBe(false)
    expect(foldEntries([first, changed], 4).lines[0]!.unseen).toBe(true)
    // Seen the change but not the original is not a state that can occur, and
    // the OR is what keeps a line the reader has never seen at all marked new.
    expect(foldEntries([first, changed], 0).lines[0]!.unseen).toBe(true)
  })

  it('ignores narrative entries when building lines, and heads a batch with its note', () => {
    const lede = entry({ kind: 'lede', title: 'Daily brief', body: 'Two things need you.' })
    const note = entry({ kind: 'note', body: 'The webhook review was signed off.', batch: 'b1' })
    const item = entry({ sourceKey: 'task:9', title: 'Reply to Dana', batch: 'b1' })

    const { lines, updates } = foldEntries([lede, note, item], 0)

    // Neither the lede nor the note has a source key, so neither becomes a row
    // in the document — they are the voice around the rows.
    expect(lines.map((l) => l.key)).toEqual(['task:9'])
    expect(updates).toHaveLength(1)
    expect(updates[0]!.note).toBe('The webhook review was signed off.')
    expect(updates[0]!.entries.map((e) => e.id)).toEqual([item.id])
  })

  it('excludes the lede from the timeline entirely', () => {
    const lede = entry({ kind: 'lede', body: 'Two things need you.' })
    const { updates } = foldEntries([lede], 0)
    // The opening read is the head of the DOCUMENT, not the first thing that
    // happened today. A timeline that led with "07:00 — your brief opened" pads
    // the one surface whose whole job is to show what actually moved.
    expect(updates).toEqual([])
  })

  it('groups by the append that wrote the rows and lists batches newest first', () => {
    const morning = entry({ sourceKey: 'task:1', seq: 2, batch: 'b1' })
    const noonA = entry({ sourceKey: 'task:2', seq: 3, batch: 'b2' })
    const noonB = entry({ sourceKey: 'task:3', seq: 4, batch: 'b2' })

    const { updates } = foldEntries([morning, noonA, noonB], 0)

    expect(updates).toHaveLength(2)
    // Newest first: a person checking back at 14:00 wants the last thing that
    // happened at the top. (The mirrored artifact reverses this deliberately —
    // a shared document is read as a narrative.)
    expect(updates[0]!.entries.map((e) => e.id)).toEqual([noonA.id, noonB.id])
    expect(updates[1]!.entries.map((e) => e.id)).toEqual([morning.id])
  })

  it('keeps two appends in the same second apart', () => {
    // THE WHOLE REASON `batch` IS STORED. Grouping used to truncate
    // `created_at` to the second, which is right until a realtime nudge and a
    // scheduler tick reach `sweepBrief` together — and then two separate
    // moments in the day render as one, on the surface whose entire job is to
    // be an honest record of when things were learned.
    const at = '2026-08-17T12:00:00.000Z'
    const first = entry({ sourceKey: 'task:1', seq: 2, batch: 'b1', createdAt: at })
    const second = entry({ sourceKey: 'task:2', seq: 3, batch: 'b2', createdAt: at })

    expect(foldEntries([first, second], 0).updates).toHaveLength(2)
  })

  it('falls back to the timestamp for rows written before the batch column', () => {
    // Briefs that already exist have `batch = null`. They still have to render
    // as a timeline rather than as one undifferentiated blob.
    const morning = entry({ sourceKey: 'task:1', seq: 2, batch: null, createdAt: '2026-08-17T09:00:00.000Z' })
    const noon = entry({ sourceKey: 'task:2', seq: 3, batch: null, createdAt: '2026-08-17T12:00:00.000Z' })

    expect(foldEntries([morning, noon], 0).updates).toHaveLength(2)
  })
})
