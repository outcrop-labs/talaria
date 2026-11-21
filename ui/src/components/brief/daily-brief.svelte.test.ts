// @vitest-environment runes
//
// Marking a brief line handled: one request, the line struck through before
// it answers, the answer's page in the cache, the line put back when it
// fails — and the realtime echo of the click costing no GET. The firehose is
// the REAL user-events module (the app-wide invalidation map and its
// listener registry), with only its transport — the leader lock and the hub —
// stood in for, so a test frame reaches every listener the app would mount.
import { QueryClient } from '@tanstack/svelte-query'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import type { BriefEntry, BriefLine, BriefResponse, BriefView } from '@/lib/daily-brief-types'

const h = vi.hoisted(() => ({
  client: null as unknown,
  frames: [] as Array<(data: string) => void>,
  getJson: vi.fn<(url: string) => Promise<unknown>>(),
  postJson: vi.fn<(url: string, body?: unknown) => Promise<unknown>>(),
}))

vi.mock('@/lib/fetch-json', () => ({
  getJson: (url: string) => h.getJson(url),
  postJson: (url: string, body?: unknown) => h.postJson(url, body),
  errorMessage: (e: unknown) => String(e),
}))

vi.mock('@tanstack/svelte-query', async (importOriginal) => {
  const real = await importOriginal<typeof import('@tanstack/svelte-query')>()
  const createQuery = ((options: Parameters<typeof real.createQuery>[0], client?: () => QueryClient) =>
    real.createQuery(options, client ?? (() => h.client as QueryClient))) as typeof real.createQuery
  return { ...real, createQuery, useQueryClient: () => h.client as QueryClient }
})

// The firehose's transport, and nothing else of it.
vi.mock('@/lib/notify-hub', () => ({
  startHub: () => ({
    role: () => 'solo',
    state: () => 'open',
    onFrame: (fn: (data: string) => void) => {
      h.frames.push(fn)
      return () => {
        h.frames = h.frames.filter((f) => f !== fn)
      }
    },
    onOpen: () => () => {},
    onRole: () => () => {},
    onState: () => () => {},
    resumed: () => {},
    close: () => {},
  }),
}))
vi.mock('@/lib/notify-leader', () => ({
  browserLeaderEdges: () => ({ locks: null, channel: null }),
  startNotifyLeader: () => ({ close: () => {} }),
}))
vi.mock('@/lib/notify-presence', () => ({ clientId: () => 'test-tab', remintClientId: () => {} }))
vi.mock('@/lib/notify-status.svelte', () => ({ noteStreamEvent: () => {}, setHubRole: () => {}, setStreamState: () => {} }))
vi.mock('@/lib/looking', () => ({ lastInputAt: () => 0, trackInput: () => () => {} }))
vi.mock('@/lib/sse', () => ({ openStream: () => () => {}, untilSessionAnswers: async () => true }))

import { useUserEventInvalidation } from '@/lib/user-events.svelte'
import { BRIEF_KEY, briefMarkFailed, useBrief, useBriefActions, useBriefLive } from './daily-brief.svelte'

const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

const entry = (over: Partial<BriefEntry>): BriefEntry => ({
  id: 'e-1',
  seq: 2,
  batch: 'b-1',
  kind: 'item',
  section: 'action',
  sourceKey: 'task:one',
  sourceType: 'task',
  sourceId: 'one',
  sourceHref: '/boards/b/one',
  fingerprint: 'fp-one',
  supersedes: null,
  priority: 'p1',
  statusLabel: 'BLOCKED',
  badge: { label: 'DUE TODAY', tone: 'danger' },
  title: 'Unblock the first thing',
  body: 'It is blocked.',
  evidence: [{ label: 'Task', text: 'details' }],
  createdAt: '2026-11-20T09:00:00.000Z',
  ...over,
})

const line = (e: BriefEntry): BriefLine => ({ key: e.sourceKey!, section: e.section, current: e, history: [e], resolved: false, unseen: false })

function page(over: Partial<BriefView> = {}): BriefView {
  const one = entry({})
  const two = entry({ id: 'e-2', seq: 3, sourceKey: 'task:two', sourceId: 'two', title: 'Review the second thing', priority: 'p2', badge: null })
  return {
    id: 'brief-1',
    date: '2026-11-20',
    zone: 'UTC',
    openedAt: '2026-11-20T07:00:00.000Z',
    lede: 'Two things need you.',
    agent: { configured: true, model: 'm', name: 'Assistant' },
    artifactId: null,
    sections: [{ section: 'action', lines: [line(one), line(two)] }],
    updates: [],
    comms: [],
    lastSeq: 3,
    readSeq: 3,
    unseenCount: 0,
    lastSweptAt: null,
    ...over,
  }
}

/** The page the server answers after checking off task:one. */
function checkedPage(): BriefView {
  const p = page({ lastSeq: 4, unseenCount: 1 })
  const closing = entry({
    id: 'e-3',
    seq: 4,
    batch: 'b-2',
    kind: 'checked',
    supersedes: 'e-1',
    priority: 'ok',
    statusLabel: 'CHECKED OFF',
    badge: null,
    body: '',
    evidence: [],
    createdAt: '2026-11-20T10:00:00.000Z',
  })
  const [one, two] = p.sections[0]!.lines
  p.sections[0]!.lines = [two!, { ...one!, current: closing, history: [...one!.history, closing], resolved: true, unseen: true }]
  return p
}

/** The page after both lines were checked off, task:two last. */
function bothCheckedPage(): BriefView {
  const p = checkedPage()
  p.lastSeq = 5
  const lines = p.sections[0]!.lines
  const two = lines.find((l) => l.key === 'task:two')!
  const closing = entry({
    ...two.current,
    id: 'e-4',
    seq: 5,
    batch: 'b-3',
    kind: 'checked',
    supersedes: two.current.id,
    priority: 'ok',
    statusLabel: 'CHECKED OFF',
    badge: null,
    body: '',
    evidence: [],
  })
  p.sections[0]!.lines = lines.map((l) => (l.key === 'task:two' ? { ...l, current: closing, history: [...l.history, closing], resolved: true, unseen: true } : l))
  return p
}

const cached = (): BriefView => (h.client as QueryClient).getQueryData<BriefResponse>(BRIEF_KEY) as BriefView
const lineOf = (p: BriefView, key: string): BriefLine => p.sections.flatMap((s) => s.lines).find((l) => l.key === key)!
const send = (event: unknown) => {
  for (const fn of h.frames) fn(JSON.stringify(event))
}
const gets = () => h.getJson.mock.calls.filter(([url]) => String(url).startsWith('/api/brief')).length

let stop: (() => void) | null = null

/** The surface as Home mounts it: the brief query (observed, so an
 *  invalidation would fetch), its live listener, the app-wide map, and the
 *  actions. Primed with `start` so mounting reads nothing. */
function mount(start: BriefView) {
  const qc = h.client as QueryClient
  qc.setQueryData(BRIEF_KEY, start)
  let actions!: ReturnType<typeof useBriefActions>
  stop = $effect.root(() => {
    useUserEventInvalidation()
    useBrief()
    useBriefLive()
    actions = useBriefActions()
  })
  return actions
}

/** A POST that answers when told to. */
function deferredPost() {
  let resolve!: (v: unknown) => void
  let reject!: (e: unknown) => void
  const answer = new Promise((res, rej) => {
    resolve = res
    reject = rej
  })
  h.postJson.mockImplementationOnce(() => answer)
  return { resolve, reject }
}

beforeEach(() => {
  // user-events opens its runtime only where a page exists.
  vi.stubGlobal('window', { addEventListener: () => {} })
  vi.stubGlobal('document', { addEventListener: () => {}, visibilityState: 'visible', hasFocus: () => true })
  h.client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } })
  h.getJson.mockReset()
  h.postJson.mockReset()
  h.getJson.mockImplementation(async () => checkedPage())
})

afterEach(() => {
  stop?.()
  stop = null
  ;(h.client as QueryClient).clear()
  vi.unstubAllGlobals()
})

test('a mark is one request: the POST, and no GET — not even for its own realtime echo', async () => {
  const actions = mount(page())
  await settle()
  const post = deferredPost()
  const done = actions.markItem('task:one', 'check')
  // The click's own append publishes before its answer arrives.
  send({ type: 'brief', briefId: 'brief-1', seq: 4 })
  await settle()
  post.resolve({ ok: true, brief: checkedPage() })
  expect(await done).toBe(true)
  await settle()

  expect(h.postJson).toHaveBeenCalledTimes(1)
  expect(h.postJson.mock.calls[0]![0]).toBe('/api/brief/item')
  expect(h.postJson.mock.calls[0]![1]).toMatchObject({ sourceKey: 'task:one', action: 'check' })
  expect(gets()).toBe(0)
})

test('the line is struck through before the POST answers, as the row the server will write', async () => {
  const actions = mount(page())
  await settle()
  const post = deferredPost()
  const done = actions.markItem('task:one', 'check')

  const marked = lineOf(cached(), 'task:one')
  expect(marked.resolved).toBe(true)
  expect(marked.current).toMatchObject({
    kind: 'checked',
    priority: 'ok',
    statusLabel: 'CHECKED OFF',
    badge: null,
    body: '',
    evidence: [],
    fingerprint: 'fp-one',
    supersedes: 'e-1',
    title: 'Unblock the first thing',
  })
  // Resolved lines sink, as the engine sorts them.
  expect(cached().sections[0]!.lines.map((l) => l.key)).toEqual(['task:two', 'task:one'])
  // The page's own cursor is the server's, untouched by the guess.
  expect(cached().lastSeq).toBe(3)

  post.resolve({ ok: true, brief: checkedPage() })
  await done
})

test('the answer’s page replaces the cache', async () => {
  const actions = mount(page())
  await settle()
  h.postJson.mockResolvedValueOnce({ ok: true, brief: checkedPage() })
  await actions.markItem('task:one', 'check')
  expect(cached()).toEqual(checkedPage())
})

test('a dismissal and a put-back write their own literals', async () => {
  const actions = mount(page())
  await settle()
  const post = deferredPost()
  const done = actions.markItem('task:two', 'dismiss')
  expect(lineOf(cached(), 'task:two').current).toMatchObject({ kind: 'dismissed', statusLabel: 'DISMISSED', priority: 'ok' })
  post.resolve({ ok: true, brief: checkedPage() })
  await done

  // Put back: the last LIVE row of the line's history, as a change.
  const back = deferredPost()
  const restoring = actions.markItem('task:one', 'restore')
  const restored = lineOf(cached(), 'task:one')
  expect(restored.resolved).toBe(false)
  expect(restored.current).toMatchObject({
    kind: 'change',
    priority: 'p1',
    statusLabel: 'BLOCKED',
    badge: { label: 'DUE TODAY', tone: 'danger' },
    body: 'It is blocked.',
    supersedes: 'e-3',
  })
  back.resolve({ ok: true, brief: page({ lastSeq: 5 }) })
  await restoring
})

test('a failed POST puts the line back as it was, refetches, and says so', async () => {
  const start = page()
  const actions = mount(start)
  await settle()
  const post = deferredPost()
  h.getJson.mockImplementation(() => new Promise(() => {}))
  const done = actions.markItem('task:one', 'check')
  expect(lineOf(cached(), 'task:one').resolved).toBe(true)
  post.reject(new Error('Could not reach the server.'))

  expect(await done).toBe(false)
  expect(cached()).toEqual(start)
  expect(briefMarkFailed('task:one')).toBe(true)
  await settle()
  expect(gets()).toBe(1)

  // The next mark on that line clears the failure.
  h.postJson.mockResolvedValueOnce({ ok: true, brief: checkedPage() })
  await actions.markItem('task:one', 'check')
  expect(briefMarkFailed('task:one')).toBe(false)
})

test('an event at or below the cached page’s seq issues no GET', async () => {
  mount(page())
  await settle()
  send({ type: 'brief', briefId: 'brief-1', seq: 3 })
  send({ type: 'brief', briefId: 'brief-1', seq: 2 })
  await settle()
  expect(gets()).toBe(0)
})

test('one new event reaching both listeners issues one GET', async () => {
  mount(page())
  await settle()
  send({ type: 'brief', briefId: 'brief-1', seq: 4 })
  await settle()
  expect(gets()).toBe(1)
  await settle()
  expect(cached().lastSeq).toBe(4)
})

test('an event held during a mark that the answer did not cover still refetches', async () => {
  const actions = mount(page())
  await settle()
  const post = deferredPost()
  const done = actions.markItem('task:one', 'check')
  send({ type: 'brief', briefId: 'brief-1', seq: 9 })
  await settle()
  expect(gets()).toBe(0)
  post.resolve({ ok: true, brief: checkedPage() })
  await done
  await settle()
  expect(gets()).toBe(1)
})

test('a page read before the mark landed does not overwrite the page its answer carried', async () => {
  const actions = mount(page())
  await settle()
  h.postJson.mockResolvedValueOnce({ ok: true, brief: checkedPage() })
  await actions.markItem('task:one', 'check')
  // A read that left before the commit comes back with the old log.
  h.getJson.mockImplementationOnce(async () => page())
  await (h.client as QueryClient).refetchQueries({ queryKey: BRIEF_KEY })
  expect(lineOf(cached(), 'task:one').resolved).toBe(true)
  expect(cached().lastSeq).toBe(4)
})

test('two marks answering out of order keep the newer page', async () => {
  const actions = mount(page())
  await settle()
  const first = deferredPost()
  const second = deferredPost()
  const a = actions.markItem('task:one', 'check')
  const b = actions.markItem('task:two', 'check')
  await settle()
  // The second mark's answer has both check-offs; the first's was read before
  // the second committed, and arrives last.
  second.resolve({ ok: true, brief: bothCheckedPage() })
  await b
  first.resolve({ ok: true, brief: checkedPage() })
  await a
  expect(cached().lastSeq).toBe(5)
  expect(lineOf(cached(), 'task:one').resolved).toBe(true)
  expect(lineOf(cached(), 'task:two').resolved).toBe(true)
  expect(gets()).toBe(0)
})

// ── The stale nudge ─────────────────────────────────────────────────────────
//
// mark_brief_stale (a notification landed, a thread moved, an approval was
// decided) clears the sweep throttle and publishes the brief's UNCHANGED seq:
// the page holds that seq already, and the re-read — which sweeps — is the
// whole point of the event.

test('a stale nudge at the cached page’s seq still refetches, once', async () => {
  mount(page())
  await settle()
  send({ type: 'brief', briefId: 'brief-1', seq: 3, stale: true })
  await settle()
  expect(gets()).toBe(1)
})

test('a stale nudge heard during a mark refetches after the answer, whatever seq the answer reached', async () => {
  const actions = mount(page())
  await settle()
  const post = deferredPost()
  const done = actions.markItem('task:one', 'check')
  // The nudge's seq is the page's; the mark's own echo follows it.
  send({ type: 'brief', briefId: 'brief-1', seq: 3, stale: true })
  send({ type: 'brief', briefId: 'brief-1', seq: 4 })
  await settle()
  expect(gets()).toBe(0)
  post.resolve({ ok: true, brief: checkedPage() })
  await done
  await settle()
  expect(gets()).toBe(1)
})

test('a click does not cancel the read a stale nudge started', async () => {
  const actions = mount(page())
  await settle()
  // The swept page the nudge's read comes back with: the check-off, and a
  // new line the sweep found.
  const swept = checkedPage()
  swept.lastSeq = 5
  const three = entry({ id: 'e-5', seq: 5, sourceKey: 'task:three', sourceId: 'three', title: 'A third thing', priority: 'p2', badge: null })
  swept.sections[0]!.lines = [line(three), ...swept.sections[0]!.lines]
  let answerRead!: (v: unknown) => void
  h.getJson.mockImplementationOnce(() => new Promise((resolve) => (answerRead = resolve)))
  send({ type: 'brief', briefId: 'brief-1', seq: 3, stale: true })
  await settle()
  expect(gets()).toBe(1)

  h.postJson.mockResolvedValueOnce({ ok: true, brief: checkedPage() })
  await actions.markItem('task:one', 'check')
  answerRead(swept)
  await settle()
  expect(gets()).toBe(1)
  expect(lineOf(cached(), 'task:three')).toBeDefined()
  expect(lineOf(cached(), 'task:one').resolved).toBe(true)
})

// ── A failure the server's page makes moot ──────────────────────────────────

test('a POST that failed after its commit landed stops saying so once the refetch shows the mark', async () => {
  const actions = mount(page())
  await settle()
  const post = deferredPost()
  // The refetch reads the log with the check-off in it.
  let answerRead!: (v: unknown) => void
  h.getJson.mockImplementationOnce(() => new Promise((resolve) => (answerRead = resolve)))
  const done = actions.markItem('task:one', 'check')
  post.reject(new Error('The proxy timed out.'))
  expect(await done).toBe(false)
  expect(briefMarkFailed('task:one')).toBe(true)
  await settle()
  expect(gets()).toBe(1)
  answerRead(checkedPage())
  await settle()
  expect(lineOf(cached(), 'task:one').resolved).toBe(true)
  expect(briefMarkFailed('task:one')).toBe(false)
})

test('a failed mark a later click on the line has overtaken is not shown as failed', async () => {
  const actions = mount(page())
  await settle()
  h.getJson.mockImplementation(() => new Promise(() => {}))
  const first = deferredPost()
  const second = deferredPost()
  const check = actions.markItem('task:one', 'check')
  const restore = actions.markItem('task:one', 'restore')
  first.reject(new Error('Could not reach the server.'))
  expect(await check).toBe(false)
  expect(briefMarkFailed('task:one')).toBe(false)
  second.resolve({ ok: true, brief: page() })
  expect(await restore).toBe(true)
  expect(briefMarkFailed('task:one')).toBe(false)
  expect(lineOf(cached(), 'task:one').resolved).toBe(false)
})

test('a check and a quick put-back in flight on one line leave stand-ins with their own ids and seqs', async () => {
  const actions = mount(page())
  await settle()
  const first = deferredPost()
  const second = deferredPost()
  const check = actions.markItem('task:one', 'check')
  const restore = actions.markItem('task:one', 'restore')
  const history = lineOf(cached(), 'task:one').history
  expect(history.map((e) => e.kind)).toEqual(['item', 'checked', 'change'])
  expect(new Set(history.map((e) => e.id)).size).toBe(3)
  expect(history.map((e) => e.seq)).toEqual([2, 4, 5])
  // The put-back copies the server's live row, not the check's stand-in.
  expect(history[2]).toMatchObject({ priority: 'p1', statusLabel: 'BLOCKED', supersedes: history[1]!.id })
  first.resolve({ ok: true, brief: checkedPage() })
  await check
  second.resolve({ ok: true, brief: page({ lastSeq: 5 }) })
  await restore
})
