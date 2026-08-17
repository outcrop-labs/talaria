// The two SSE routes, tested from src/server/ because vitest.config.ts excludes
// `src/routes/**` from COLLECTION (a dot is a path separator there, so
// `api/mcp.test.ts` is a handler, not a suite) — it does not forbid importing a
// route module, and the property under test here cannot be tested anywhere
// else:
//
//   AN UNAUTHORIZED CALLER IS REFUSED BEFORE A SUBSCRIBER EXISTS.
//
// That is two bugs in one assertion. The obvious one is the ACL. The other is
// resource exhaustion: `runEventStream` opens a DEDICATED Redis connection per
// client, and nothing disconnects a stream that was created and then dropped on
// the floor in favour of a 403 — the cleanup hangs off the request's abort
// signal and the ReadableStream's `cancel`, and neither fires for a stream no
// caller was ever handed. A rejected request is exactly the kind that gets
// retried in a loop.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ApiHandler } from '@/server/api-route'

const guard = vi.hoisted(() => ({ requireUser: vi.fn() }))
const realtime = vi.hoisted(() => ({
  mayWatchRun: vi.fn(),
  // Typed parameters, so `mock.calls[0][0]` is the topic key the route asked
  // for and not `never` — the id these routes subscribe to IS the assertion.
  runEventStream: vi.fn((_runId: string, _signal: AbortSignal) => new ReadableStream<Uint8Array>()),
  userEventStream: vi.fn((_userId: string, _signal: AbortSignal) => new ReadableStream<Uint8Array>()),
}))

vi.mock('@/server/api-guard', () => guard)
vi.mock('@/server/realtime', () => realtime)

import { Route as RunEvents } from '@/routes/api/runs.$id.events'
import { Route as MeEvents } from '@/routes/api/me.events'

const signedIn = { id: 'u1', email: 'u1@example.com', role: 'member' }
const unauthorized = () => new Response(JSON.stringify({ error: 'unauthorized' }), { status: 401 })

function call(handler: ApiHandler | undefined, url: string, params: Record<string, string>): Promise<Response> {
  expect(handler).toBeDefined()
  return Promise.resolve(handler!({ request: new Request(url), params }))
}

beforeEach(() => {
  vi.clearAllMocks()
  realtime.runEventStream.mockImplementation((_runId: string, _signal: AbortSignal) => new ReadableStream<Uint8Array>())
  realtime.userEventStream.mockImplementation((_userId: string, _signal: AbortSignal) => new ReadableStream<Uint8Array>())
})

describe('GET /api/runs/:id/events', () => {
  it('refuses an unauthenticated caller without opening a subscriber', async () => {
    guard.requireUser.mockResolvedValue(unauthorized())
    const res = await call(RunEvents.handlers.GET, 'http://t/api/runs/r1/events', { id: 'r1' })
    expect(res.status).toBe(401)
    expect(realtime.mayWatchRun).not.toHaveBeenCalled()
    expect(realtime.runEventStream).not.toHaveBeenCalled()
  })

  it('refuses a caller the run’s ACL rejects without opening a subscriber', async () => {
    guard.requireUser.mockResolvedValue(signedIn)
    realtime.mayWatchRun.mockResolvedValue({ ok: false, reason: 'not-audience' })
    const res = await call(RunEvents.handlers.GET, 'http://t/api/runs/r1/events', { id: 'r1' })
    expect(res.status).toBe(403)
    expect(realtime.runEventStream).not.toHaveBeenCalled()
    expect(realtime.mayWatchRun).toHaveBeenCalledWith('u1', 'r1')
  })

  it('answers a missing run exactly like a refusal — no existence oracle', async () => {
    guard.requireUser.mockResolvedValue(signedIn)
    realtime.mayWatchRun.mockResolvedValue({ ok: false, reason: 'missing' })
    const res = await call(RunEvents.handlers.GET, 'http://t/api/runs/nope/events', { id: 'nope' })
    expect(res.status).toBe(403)
    expect(await res.json()).toEqual({ error: 'forbidden' })
    expect(realtime.runEventStream).not.toHaveBeenCalled()
  })

  it('streams for a caller the run’s ACL admits', async () => {
    guard.requireUser.mockResolvedValue(signedIn)
    realtime.mayWatchRun.mockResolvedValue({ ok: true })
    const res = await call(RunEvents.handlers.GET, 'http://t/api/runs/r1/events', { id: 'r1' })
    expect(res.status).toBe(200)
    expect(res.headers.get('Content-Type')).toBe('text/event-stream')
    expect(res.headers.get('Cache-Control')).toBe('no-cache, no-transform')
    expect(realtime.runEventStream).toHaveBeenCalledTimes(1)
    expect(realtime.runEventStream.mock.calls[0]?.[0]).toBe('r1')
  })
})

describe('GET /api/me/events', () => {
  it('refuses an unauthenticated caller without opening a subscriber', async () => {
    guard.requireUser.mockResolvedValue(unauthorized())
    const res = await call(MeEvents.handlers.GET, 'http://t/api/me/events', {})
    expect(res.status).toBe(401)
    expect(realtime.userEventStream).not.toHaveBeenCalled()
  })

  it('subscribes the SESSION’s id, never one supplied by the request', async () => {
    guard.requireUser.mockResolvedValue(signedIn)
    // The query string is the attack: there must be no path by which a caller
    // names whose firehose they get.
    const res = await call(MeEvents.handlers.GET, 'http://t/api/me/events?userId=u2', {})
    expect(res.status).toBe(200)
    expect(res.headers.get('Content-Type')).toBe('text/event-stream')
    expect(realtime.userEventStream).toHaveBeenCalledTimes(1)
    expect(realtime.userEventStream.mock.calls[0]?.[0]).toBe('u1')
  })
})
