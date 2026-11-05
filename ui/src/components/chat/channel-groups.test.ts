// The channel surfaces' wiring into lib/message-groups: what ChannelView and
// ThreadPanel actually pass, run through the real grouping function.
import { describe, expect, it } from 'vitest'
import type { ChannelMessage } from '@/lib/channels.svelte'
import { continuationSet, messageBlocks } from '@/lib/message-groups'
import { channelGroupFacts, threadGroupFacts } from './channel-groups'

const T0 = Date.parse('2026-11-04T15:00:00Z')
let seq = 0
const msg = (author: string, offsetMin: number, extra: Partial<ChannelMessage> = {}): ChannelMessage =>
  ({
    id: `m${++seq}`,
    seq,
    authorType: 'user',
    author,
    content: 'hi',
    status: 'complete',
    createdAt: new Date(T0 + offsetMin * 60_000).toISOString(),
    ...extra,
  }) as ChannelMessage
const noDays = { has: () => false }
const continued = (rows: ChannelMessage[], facts: typeof threadGroupFacts) =>
  [...continuationSet(messageBlocks(rows, facts, { dayStarts: noDays }))].sort((a, b) => a - b)

describe('channelGroupFacts — the main feed', () => {
  it('groups a run by author', () => {
    const rows = [msg('ada@x', 0), msg('ada@x', 1), msg('ben@x', 2), msg('ben@x', 3)]
    expect(continued(rows, channelGroupFacts)).toEqual([1, 3])
  })

  it('keys on the author type too: an agent never continues a user of the same name', () => {
    const rows = [msg('scout', 0), msg('scout', 1, { authorType: 'agent' })]
    expect(continued(rows, channelGroupFacts)).toEqual([])
  })

  it('ignores status: a streaming agent reply is a continuation while it streams and once it settles', () => {
    const agent = { authorType: 'agent' as const }
    const before = [msg('scout', 0, agent)]
    const streaming = [...before, msg('scout', 1, { ...agent, status: 'streaming', content: '' })]
    const grown = [...before, { ...streaming[1]!, content: 'Here is the first part' }]
    const settled = [...before, { ...grown[1]!, status: 'complete' as const }]
    const failed = [...before, { ...grown[1]!, status: 'error' as const }]
    expect(continued(streaming, channelGroupFacts)).toEqual([1])
    for (const rows of [grown, settled, failed]) expect(continued(rows, channelGroupFacts)).toEqual([1])
  })
})

describe('threadGroupFacts — the thread panel', () => {
  it('the root stands alone: its author replying a minute later still opens a block under the rule', () => {
    const rows = [msg('ada@x', 0), msg('ada@x', 1), msg('ada@x', 2), msg('ben@x', 3)]
    // root | reply 1 + reply 2 | Ben
    expect(messageBlocks(rows, threadGroupFacts, { dayStarts: noDays }).map((b) => [b.start, b.end])).toEqual([
      [0, 0],
      [1, 2],
      [3, 3],
    ])
  })

  it('replies after the first group like the main feed', () => {
    const rows = [msg('ada@x', 0), msg('ben@x', 1), msg('ben@x', 2), msg('ben@x', 3)]
    expect(continued(rows, threadGroupFacts)).toEqual([2, 3])
  })
})
