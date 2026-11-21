// @vitest-environment runes
import { QueryClient } from '@tanstack/svelte-query'
import { afterEach, expect, test, vi } from 'vitest'
import { startOfLocalDay, type Artifact, type ArtifactHead } from '@/lib/artifacts'
import type { BriefView } from '@/lib/daily-brief-types'
import { buildInboxCards, type InboxInput } from '@/lib/inbox-board'
import type { HomeQueues, HomeSummary, WorkItem } from '@/routes/app/home/home'

const h = vi.hoisted(() => ({ client: null as unknown }))

vi.mock('@tanstack/svelte-query', async (importOriginal) => {
  const real = await importOriginal<typeof import('@tanstack/svelte-query')>()
  const client = () => h.client as QueryClient
  const createQuery = ((options: Parameters<typeof real.createQuery>[0], c?: () => QueryClient) =>
    real.createQuery(options, c ?? client)) as typeof real.createQuery
  const createQueries = ((options: Parameters<typeof real.createQueries>[0], c?: () => QueryClient) =>
    real.createQueries(options, c ?? client)) as typeof real.createQueries
  return { ...real, createQuery, createQueries }
})

import { useInboxBoard, type InboxBoardState } from './inbox-board.svelte'

const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

const NOW = new Date(2026, 10, 20, 15, 0, 0).getTime()
const MIDNIGHT = new Date(2026, 10, 20).getTime()
const at = (ms: number): string => new Date(ms).toISOString()
const hoursAgo = (hrs: number): string => at(NOW - hrs * 3_600_000)

const item = (id: string, status: string): WorkItem => ({
  id,
  boardId: 'b1',
  board: 'Helpdesk',
  ticketRef: `HD-${id}`,
  title: `Ticket ${id}`,
  status,
  updatedAt: hoursAgo(3),
})

const QUEUES: HomeQueues = {
  triage: { count: 1, items: [item('1', 'inbox')] },
  review: { count: 1, items: [item('2', 'quality_review')] },
  blocked: { count: 1, items: [item('3', 'blocked')] },
}

/** /api/home as an admin read it: the queues inside the whole glance. */
const SUMMARY: HomeSummary = {
  org: { name: 'Org', activity: [], alerts: 2, costToday: { tokens: 10, usd: 0.01 } },
  queues: QUEUES,
  unread: 4,
  boards: 1,
}

const artifact = (id: string, updatedAt: string, over: Partial<Artifact> = {}): Artifact => ({
  id,
  kind: 'doc',
  title: `Doc ${id}`,
  icon: null,
  body: `the body of ${id}, which the board never shows`.repeat(40),
  contentType: null,
  storageRef: null,
  visibility: 'org',
  editPolicy: 'owner',
  publicSlug: null,
  official: false,
  ragRouting: 'auto',
  kbDocId: null,
  folderId: null,
  ownerUserId: 'u-1',
  googleFileId: null,
  googleFileUrl: null,
  createdBy: 'opus',
  updatedBy: null,
  createdAt: hoursAgo(48),
  updatedAt,
  ...over,
})

/** The whole list, as /api/artifacts answers it: bodies, and every day. */
const ALL: Artifact[] = [
  artifact('edited-today', hoursAgo(2)),
  artifact('made-today', hoursAgo(1), { createdAt: hoursAgo(1) }),
  artifact('at-midnight', at(MIDNIGHT)),
  artifact('a-moment-before', at(MIDNIGHT - 1)),
  artifact('yesterday', hoursAgo(30)),
  artifact('last-week', hoursAgo(24 * 7)),
  artifact('transcript', hoursAgo(1), { kind: 'run-transcript' }),
  artifact('report', hoursAgo(1)),
  artifact('brief-mirror', hoursAgo(1)),
]

/** What `?since=<start of the reader's day>&fields=heads` answers for the same rows:
 *  the server's `updated_at >= since`, every column but the body. */
const heads = (all: readonly Artifact[], now: number): ArtifactHead[] => {
  const since = Date.parse(startOfLocalDay(now))
  return all.filter((a) => Date.parse(a.updatedAt) >= since).map(({ body: _body, ...head }) => head)
}

afterEach(() => {
  vi.unstubAllGlobals()
  ;(h.client as QueryClient | null)?.clear()
})

test("the start of the reader's day is local midnight, whatever the hour", () => {
  expect(startOfLocalDay(NOW)).toBe(at(MIDNIGHT))
  expect(startOfLocalDay(MIDNIGHT)).toBe(at(MIDNIGHT))
  expect(startOfLocalDay(MIDNIGHT - 1)).toBe(at(new Date(2026, 10, 19).getTime()))
})

test('the cards come out the same from the queues alone and the heads since today', () => {
  const base: Omit<InboxInput, 'queues' | 'artifacts'> = {
    now: NOW,
    brief: null,
    sessions: [],
    runs: [
      {
        id: 'r1',
        ownerUserId: 'u-1',
        requestedBy: 'u-1',
        agentModel: 'opus',
        mode: 'recon',
        question: 'What changed?',
        title: null,
        status: 'done',
        phase: null,
        awaiting: null,
        artifactId: 'report',
        conversationId: null,
        error: null,
        stats: {},
        createdAt: hoursAgo(2),
        completedAt: hoursAgo(1),
      },
    ],
    agentName: (id) => (id === 'opus' ? 'Opus' : null),
    assignees: () => null,
  }
  const before = buildInboxCards({ ...base, queues: SUMMARY.queues, artifacts: ALL })
  const answer = JSON.parse(JSON.stringify({ queues: QUEUES })) as { queues: HomeQueues }
  const narrowed = heads(ALL, NOW)
  const after = buildInboxCards({ ...base, queues: answer.queues, artifacts: narrowed })

  expect(after).toEqual(before)
  // Not vacuous: the heads drop rows, the cards carry tickets and today's documents.
  expect(narrowed.map((a) => a.id)).not.toContain('a-moment-before')
  expect(narrowed.length).toBeLessThan(ALL.length)
  expect(before.filter((c) => c.source === 'doc').map((c) => c.key).sort()).toEqual(
    ['doc:at-midnight', 'doc:brief-mirror', 'doc:edited-today', 'doc:made-today'],
  )
  expect(before.filter((c) => c.source === 'ticket')).toHaveLength(3)

  // And with the brief's own mirror named, it drops out of both the same way.
  // Asserted, not annotated: the brief's own fields are not what this test is about.
  const brief = {
    id: 'brief-1',
    date: '2026-11-20',
    zone: 'UTC',
    openedAt: hoursAgo(8),
    lede: 'A quiet day.',
    agent: { configured: true, model: 'opus', name: 'Opus' },
    artifactId: 'brief-mirror',
    sections: [],
    updates: [],
    comms: [],
    lastSeq: 1,
    readSeq: 1,
    unseenCount: 0,
    lastSweptAt: null,
  } as BriefView
  expect(buildInboxCards({ ...base, brief, queues: answer.queues, artifacts: narrowed })).toEqual(
    buildInboxCards({ ...base, brief, queues: SUMMARY.queues, artifacts: ALL }),
  )
})

test('the board reads the queues and the heads since today — never /api/home or the whole list', async () => {
  vi.useFakeTimers({ toFake: ['Date'], now: NOW })
  h.client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const asked: string[] = []
  const answers: Record<string, unknown> = {
    '/api/home/queues': { queues: QUEUES },
    [`/api/artifacts?since=${encodeURIComponent(at(MIDNIGHT))}&fields=heads`]: { artifacts: heads(ALL, NOW) },
    '/api/conversations?kind=sessions': { conversations: [] },
    '/api/research': { runs: [] },
    '/api/agents': { agents: [] },
    '/api/boards/b1/tasks': { tasks: [] },
    '/api/boards/b1/members': { members: [] },
    // Answered, so a regression shows up as a recorded read rather than a hang.
    '/api/home': SUMMARY,
    '/api/artifacts': { artifacts: ALL },
  }
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      asked.push(url)
      const body = answers[url]
      return body === undefined
        ? new Response(JSON.stringify({ error: 'not found' }), { status: 404 })
        : new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } })
    }),
  )

  const held: { board: InboxBoardState | null } = { board: null }
  const stop = $effect.root(() => {
    held.board = useInboxBoard(
      () => null,
      () => true,
    )
  })
  const board = held.board!
  for (let i = 0; i < 20 && board.pending; i++) await settle()

  expect(board.pending).toBe(false)
  expect(board.failure).toBeNull()
  expect(asked).toContain('/api/home/queues')
  expect(asked).toContain(`/api/artifacts?since=${encodeURIComponent(at(MIDNIGHT))}&fields=heads`)
  expect(asked).not.toContain('/api/home')
  expect(asked).not.toContain('/api/artifacts')
  expect(asked.filter((u) => u.startsWith('/api/artifacts')).every((u) => u.includes('fields=heads'))).toBe(true)
  expect(board.cards.filter((c) => c.source === 'ticket')).toHaveLength(3)
  expect(board.cards.filter((c) => c.source === 'doc').map((c) => c.key).sort()).toEqual(
    ['doc:at-midnight', 'doc:brief-mirror', 'doc:edited-today', 'doc:made-today', 'doc:report'],
  )
  stop()
  vi.useRealTimers()
})

test('the board leaves its skeleton when the brief settles after every other source answered', async () => {
  vi.useFakeTimers({ toFake: ['Date'], now: NOW })
  h.client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const answers: Record<string, unknown> = {
    '/api/home/queues': { queues: QUEUES },
    [`/api/artifacts?since=${encodeURIComponent(at(MIDNIGHT))}&fields=heads`]: { artifacts: heads(ALL, NOW) },
    '/api/conversations?kind=sessions': { conversations: [] },
    '/api/research': { runs: [] },
    '/api/agents': { agents: [] },
    '/api/boards/b1/tasks': { tasks: [] },
    '/api/boards/b1/members': { members: [] },
  }
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) =>
      answers[url] === undefined
        ? new Response('{}', { status: 404 })
        : new Response(JSON.stringify(answers[url]), { status: 200, headers: { 'content-type': 'application/json' } }),
    ),
  )
  let settled = $state(false)
  const seen = { pending: true, failure: null as unknown }
  const stop = $effect.root(() => {
    const board = useInboxBoard(
      () => null,
      () => settled,
    )
    // What InboxBoard and DailyBrief read on their first render: the failure
    // callout's condition, then the skeleton's — before anything has answered.
    $effect(() => {
      seen.failure = board.failure
      seen.pending = board.pending
    })
  })
  for (let i = 0; i < 20; i++) await settle()
  expect(seen.pending).toBe(true)

  // A slow brief lands last, as it does whenever its read sweeps first.
  settled = true
  for (let i = 0; i < 20 && seen.pending; i++) await settle()
  expect(seen.pending).toBe(false)
  expect(seen.failure).toBeNull()
  stop()
  vi.useRealTimers()
})

test("the board keeps its cards through local midnight while the new day's files load", async () => {
  // A Home tab left open overnight: the day is in the files key, so midnight
  // starts a new query. It must not put the board back on its skeleton.
  const NEXT = new Date(2026, 10, 21).getTime()
  vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval'], now: NEXT - 1_500 })
  h.client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const today = `/api/artifacts?since=${encodeURIComponent(at(MIDNIGHT))}&fields=heads`
  const tomorrow = `/api/artifacts?since=${encodeURIComponent(at(NEXT))}&fields=heads`
  const answers: Record<string, unknown> = {
    '/api/home/queues': { queues: QUEUES },
    [today]: { artifacts: heads(ALL, NOW) },
    '/api/conversations?kind=sessions': { conversations: [] },
    '/api/research': { runs: [] },
    '/api/agents': { agents: [] },
    '/api/boards/b1/tasks': { tasks: [] },
    '/api/boards/b1/members': { members: [] },
  }
  const json = (body: unknown) =>
    new Response(JSON.stringify(body), { status: 200, headers: { 'content-type': 'application/json' } })
  // The new day's read is held open, to look at the board while it is in flight.
  let answerTomorrow: (r: Response) => void = () => {}
  const asked: string[] = []
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      asked.push(url)
      if (url === tomorrow) return new Promise<Response>((resolve) => (answerTomorrow = resolve))
      return answers[url] === undefined ? new Response('{}', { status: 404 }) : json(answers[url])
    }),
  )
  const seen = { pending: true, failure: null as unknown, docs: [] as string[] }
  const stop = $effect.root(() => {
    const board = useInboxBoard(
      () => null,
      () => true,
    )
    $effect(() => {
      seen.failure = board.failure
      seen.pending = board.pending
      seen.docs = board.cards.filter((c) => c.source === 'doc').map((c) => c.key)
    })
  })
  for (let i = 0; i < 20 && seen.pending; i++) await settle()
  expect(seen.pending).toBe(false)
  expect(seen.docs.length).toBeGreaterThan(0)

  // The clock ticks past midnight; the new day's heads are on their way.
  vi.advanceTimersByTime(2_000)
  for (let i = 0; i < 20 && !asked.includes(tomorrow); i++) await settle()
  expect(asked).toContain(tomorrow)
  for (let i = 0; i < 5; i++) await settle()
  expect(seen.pending).toBe(false)
  expect(seen.failure).toBeNull()
  // Yesterday's heads stand in, and none of them is today's.
  expect(seen.docs).toEqual([])

  answerTomorrow(json({ artifacts: [] }))
  for (let i = 0; i < 5; i++) await settle()
  expect(seen.pending).toBe(false)
  expect(seen.failure).toBeNull()
  stop()
  vi.useRealTimers()
})
