import { describe, expect, it } from 'vitest'
import { foldEntries } from './daily-brief-fold'
import type { BriefEntry } from './daily-brief-types'

// THE BUG THIS SUITE EXISTS FOR, held as an assertion at the level it is
// visible: a conversation the owner READ but did not ANSWER must still be an
// open line, and the only thing that closes it is somebody replying.
//
// `commsLines` itself is one SQL statement against four joined tables, so its
// correctness is a database question and is exercised by the seed script
// end-to-end. What is tested here is the part that decided the bug: how the
// fold treats a source that emits its own resolutions, which is what lets a
// line close with WHO answered instead of by vanishing.

let seq = 0
const entry = (over: Partial<BriefEntry>): BriefEntry => ({
  id: `e${++seq}`,
  seq,
  batch: null,
  kind: 'item',
  section: 'comms',
  sourceKey: 'channel:c1',
  sourceType: 'channel',
  sourceId: 'c1',
  sourceHref: '/comms/channel/c1',
  fingerprint: null,
  supersedes: null,
  priority: 'p1',
  statusLabel: null,
  badge: null,
  title: 'Priya is waiting on you',
  body: '',
  evidence: [],
  createdAt: '2026-08-17T09:00:00.000Z',
  ...over,
})

describe('a conversation line', () => {
  it('stays open when it has been read but not answered', () => {
    const arrived = entry({ statusLabel: '2 UNREAD', fingerprint: 'a' })
    // Reading it changes the label and nothing else. The old source dropped the
    // thread from its result set here, the sweep saw the key vanish, and the
    // brief announced it done — with nobody having replied to Priya.
    const read = entry({ kind: 'change', statusLabel: 'READ, NOT ANSWERED', fingerprint: 'b', supersedes: arrived.id })

    const { lines } = foldEntries([arrived, read], 0)

    expect(lines).toHaveLength(1)
    expect(lines[0]!.resolved).toBe(false)
    expect(lines[0]!.current.statusLabel).toBe('READ, NOT ANSWERED')
  })

  it('closes only on a reply, and says who sent it', () => {
    const arrived = entry({ statusLabel: '2 UNREAD', fingerprint: 'a' })
    const answered = entry({
      kind: 'resolved',
      title: 'Replied to Priya',
      statusLabel: 'YOU REPLIED',
      supersedes: arrived.id,
    })

    const { lines } = foldEntries([arrived, answered], 0)

    expect(lines[0]!.resolved).toBe(true)
    // The distinction the whole feature turns on: a delegated reply is still an
    // answer, but the page never claims the owner wrote it.
    expect(lines[0]!.current.statusLabel).toBe('YOU REPLIED')
  })

  it('distinguishes an assistant reply from the owner’s own', () => {
    const arrived = entry({ fingerprint: 'a' })
    const answered = entry({ kind: 'resolved', statusLabel: 'ASSISTANT REPLIED', supersedes: arrived.id })

    const { lines } = foldEntries([arrived, answered], 0)

    expect(lines[0]!.resolved).toBe(true)
    expect(lines[0]!.current.statusLabel).toBe('ASSISTANT REPLIED')
    // Both entries survive, so "what did my assistant send while I was out"
    // is answerable from the line rather than only from the channel.
    expect(lines[0]!.history).toHaveLength(2)
  })

  it('reopens when they write again after being answered', () => {
    const arrived = entry({ fingerprint: 'a' })
    const answered = entry({ kind: 'resolved', statusLabel: 'YOU REPLIED', supersedes: arrived.id })
    const again = entry({ kind: 'change', statusLabel: '1 UNREAD', fingerprint: 'c', supersedes: answered.id })

    const { lines } = foldEntries([arrived, answered, again], 0)

    expect(lines[0]!.resolved).toBe(false)
    expect(lines[0]!.history).toHaveLength(3)
  })

  it('carries the drafted reply as evidence on the open line', () => {
    const withDraft = entry({
      statusLabel: 'READ · DRAFT READY',
      badge: { label: 'DRAFT READY', tone: 'accent' },
      evidence: [{ label: 'Drafted reply', text: 'Jon has seen this and will come back to you today.' }],
    })

    const { lines } = foldEntries([withDraft], 0)

    expect(lines[0]!.resolved).toBe(false)
    expect(lines[0]!.current.evidence[0]?.text).toContain('come back to you')
  })
})
