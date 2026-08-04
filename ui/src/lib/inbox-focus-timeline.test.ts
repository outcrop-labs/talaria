import assert from 'node:assert/strict'
import test from 'node:test'
import {
  buildInboxTimeline,
  decodeInboxTimelineCursor,
  encodeInboxTimelineCursor,
  mergeInboxTimelinePages,
  normalizeInboxTimelineTimestamp,
  type InboxTimelineRecord,
} from './inbox-focus-timeline'

const records: InboxTimelineRecord[] = [
  {
    recordType: 'message',
    id: '00000000-0000-4000-8000-000000000001',
    createdAt: '2026-08-03T12:00:01.000Z',
    role: 'user',
    content: 'Summarize this decision.',
    status: 'complete',
    metadata: { focus: { key: 'task:one', question: 'Approve one?', sourceHref: '/boards/one' } },
  },
  {
    recordType: 'message',
    id: '00000000-0000-4000-8000-000000000002',
    createdAt: '2026-08-03T12:00:02.000Z',
    role: 'assistant',
    content: 'The first decision is ready.',
    status: 'complete',
    metadata: { focus: { key: 'task:one', question: 'Approve one?', sourceHref: '/boards/one' } },
  },
  {
    recordType: 'decision',
    id: '00000000-0000-4000-8000-000000000003',
    createdAt: '2026-08-03T12:00:03.000Z',
    status: 'completed',
    actionId: 'mark_read',
    instruction: null,
    proposal: null,
    outcome: { afterReadAt: '2026-08-03T12:00:03.000Z' },
    focus: { key: 'notification:two', question: 'Mark this read?', sourceHref: '/notifications' },
  },
]

test('timeline orders records and inserts one context boundary per interaction change', () => {
  const timeline = buildInboxTimeline([...records].reverse())
  assert.deepEqual(timeline.map((entry) => entry.kind), ['context', 'message', 'message', 'context', 'activity'])
  assert.equal(timeline[0]?.kind === 'context' ? timeline[0].focus.key : null, 'task:one')
  assert.equal(timeline[3]?.kind === 'context' ? timeline[3].focus.key : null, 'notification:two')
})

test('detached messages do not create a context boundary', () => {
  const timeline = buildInboxTimeline([
    {
      recordType: 'message',
      id: '00000000-0000-4000-8000-000000000004',
      createdAt: '2026-08-03T12:00:04.000Z',
      role: 'user',
      content: 'What can you help with generally?',
      status: 'complete',
      metadata: {},
    },
  ])
  assert.deepEqual(timeline.map((entry) => entry.kind), ['message'])
})

test('timeline cursors round-trip their exact ordering key', () => {
  const createdAt = normalizeInboxTimelineTimestamp(records[2]!.createdAt)
  const cursor = encodeInboxTimelineCursor(createdAt, records[2]!.id)
  assert.deepEqual(decodeInboxTimelineCursor(cursor), {
    createdAt,
    id: records[2]!.id,
  })
})

test('timeline normalizes postgres Date values before ordering and serialization', () => {
  const createdAt = new Date('2026-08-03T18:00:00.000Z')
  const timeline = buildInboxTimeline([{
    recordType: 'message',
    id: '00000000-0000-4000-8000-000000000005',
    createdAt,
    role: 'assistant',
    content: 'Persisted response',
    status: 'complete',
    metadata: {},
  }])

  assert.equal(normalizeInboxTimelineTimestamp(createdAt), '2026-08-03T18:00:00.000Z')
  assert.equal(timeline[0]?.createdAt, '2026-08-03T18:00:00.000Z')
})

test('clarification decisions stay in message history without rendering a completion card', () => {
  const timeline = buildInboxTimeline([{
    recordType: 'decision',
    id: '00000000-0000-4000-8000-000000000006',
    createdAt: '2026-08-03T18:01:00.000Z',
    status: 'completed',
    actionId: null,
    instruction: 'Summarize this decision.',
    proposal: null,
    outcome: { kind: 'clarification' },
    focus: { key: 'task:one', question: 'Approve one?', sourceHref: '/boards/one' },
  }])

  assert.deepEqual(timeline, [])
})

test('page merging removes repeated context boundaries for one uninterrupted focus', () => {
  const focus = { key: 'task:one', question: 'Approve one?', sourceHref: '/boards/one' }
  const older = buildInboxTimeline(records.slice(0, 1))
  const newer = buildInboxTimeline(records.slice(1, 2))
  const merged = mergeInboxTimelinePages([newer, older])

  assert.deepEqual(merged.map((entry) => entry.kind), ['context', 'message', 'message'])
  assert.equal(merged.filter((entry) => entry.kind === 'context' && entry.focus.key === focus.key).length, 1)
})
