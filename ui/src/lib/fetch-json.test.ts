import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  HttpError,
  delJson,
  delJsonGone,
  errorMessage,
  errorSentence,
  getJson,
  getStream,
  postJson,
  postJsonOr,
  postStream,
  putJson,
  readJson,
} from '@/lib/fetch-json'

// The mutation door. The whole point of this file's subjects is the ERROR
// CONTRACT — before they existed, 134 hand-rolled stanzas each decided for
// themselves what a failed POST resolved to, and most decided "success".
// These tests pin the one decision that is allowed.

const jsonResponse = (status: number, body: unknown, headers: Record<string, string> = {}): Response =>
  new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json', ...headers } })

// ONE fetch mock for the whole file: a test that needs a particular reply
// installs it with `mockImplementationOnce` on this same instance. Reassigning
// globalThis.fetch per test once left later request assertions reading an
// array no live mock was recording into.
const fetchMock = vi.fn(async (_url: string | URL, _init?: RequestInit) => jsonResponse(200, { ok: true }))

beforeEach(() => {
  vi.stubGlobal('fetch', fetchMock)
  fetchMock.mockClear()
})
afterEach(() => vi.unstubAllGlobals())

/** The init of the most recent request — what the door put on the wire. */
const lastInit = (): RequestInit => fetchMock.mock.calls.at(-1)![1] ?? {}

describe('the mutation verbs', () => {
  it('POST/PUT encodes the body and sends the session credentials', async () => {
    await postJson('/api/x', { a: 1 })
    await putJson('/api/x', { a: 1 })
    expect(fetchMock.mock.calls.map(([, init]) => init?.method)).toEqual(['POST', 'PUT'])
    for (const [, init] of fetchMock.mock.calls) {
      expect(init?.credentials).toBe('same-origin')
      expect(init?.headers).toEqual({ 'content-type': 'application/json' })
      expect(init?.body).toBe('{"a":1}')
    }
  })

  it('DELETE carries an optional body — house deletes address the thing in the body', async () => {
    await delJson('/api/labels', { labelId: 'l1' })
    expect(lastInit().method).toBe('DELETE')
    expect(lastInit().body).toBe('{"labelId":"l1"}')
  })

  it('sends a FormData body untouched — the browser sets the multipart boundary', async () => {
    const fd = new FormData()
    fd.append('file', new Blob(['x']), 'x.txt')
    await postJson('/api/uploads', fd)
    expect(lastInit().body).toBe(fd)
    // No content-type key at all (not even `{}`): a header here would strip the
    // boundary out of the multipart body the browser builds.
    expect(lastInit().headers).toBeUndefined()
  })

  it('rejects on non-2xx with the server’s error sentence, not a status number', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(403, { error: 'only the owner can change sharing' }))
    const e = await postJson('/api/x', {}).catch((e: unknown) => e)
    expect(e).toBeInstanceOf(HttpError)
    expect((e as HttpError).status).toBe(403)
    expect((e as HttpError).message).toBe('only the owner can change sharing')
  })

  it('falls back to a status sentence when the body carries no error field', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(502, { unexpected: 'shape' }))
    await expect(postJson('/api/x', {})).rejects.toThrow('request failed (502)')
  })

  it('resolves the parsed body on success', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(200, { space: { id: 's1' } }))
    await expect(postJson<{ space: { id: string } }>('/api/x', { name: 'n' })).resolves.toEqual({ space: { id: 's1' } })
  })
})

describe('postJsonOr — statuses that are answers, not errors', () => {
  it('resolves the BODY of a listed 4xx instead of throwing', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(409, { status: 'stale', message: 'changed under you' }))
    await expect(postJsonOr('/api/focus/actions', {}, [409, 422])).resolves.toEqual({ status: 'stale', message: 'changed under you' })
  })

  it('still throws for an unlisted status', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(500, { error: 'exploded' }))
    await expect(postJsonOr('/api/focus/actions', {}, [409, 422])).rejects.toThrow('exploded')
  })

  it('readJson exposes the same carve-out directly', async () => {
    const stale = jsonResponse(409, { status: 'stale' })
    await expect(readJson(stale, [409])).resolves.toEqual({ status: 'stale' })
    await expect(readJson(jsonResponse(409, { status: 'stale' }), [422])).rejects.toBeInstanceOf(HttpError)
  })
})

describe('delJsonGone — a delete whose 404 is already gone', () => {
  it('resolves on a 200 and on a 404, and still rejects anything else', async () => {
    await expect(delJsonGone('/api/x/1')).resolves.toBeUndefined()
    expect(lastInit().method).toBe('DELETE')
    fetchMock.mockImplementationOnce(async () => jsonResponse(404, { error: 'not found' }))
    await expect(delJsonGone('/api/x/1')).resolves.toBeUndefined()
    fetchMock.mockImplementationOnce(async () => jsonResponse(403, { error: 'forbidden' }))
    await expect(delJsonGone('/api/x/1')).rejects.toMatchObject({ status: 403, message: 'forbidden' })
    fetchMock.mockImplementationOnce(async () => jsonResponse(400, { error: 'auto collections cannot be deleted' }))
    await expect(delJsonGone('/api/x/1')).rejects.toMatchObject({ status: 400 })
  })

  it('sends its body, as the secrets delete does', async () => {
    await delJsonGone('/api/x/1/secrets', { name: 'A_TOKEN' })
    expect(lastInit().body).toBe(JSON.stringify({ name: 'A_TOKEN' }))
  })
})

describe('postStream — the streaming door', () => {
  it('returns an ok response with its body untouched', async () => {
    const body = new ReadableStream<Uint8Array>()
    fetchMock.mockImplementationOnce(async () => new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } }))
    const r = await postStream('/api/chat', { m: 1 })
    expect(r.ok).toBe(true)
    expect(r.body).not.toBeNull() // the Response wraps the stream; what matters is it is THERE
    expect(lastInit().headers).toEqual({ 'content-type': 'application/json' })
  })

  it('throws with the server’s sentence on a non-2xx', async () => {
    fetchMock.mockImplementationOnce(async () => jsonResponse(429, { error: 'too many turns' }))
    const e = await postStream('/api/chat', {}).catch((e: unknown) => e)
    expect(e).toBeInstanceOf(HttpError)
    expect((e as HttpError).status).toBe(429)
    expect((e as HttpError).message).toBe('too many turns')
  })

  it('throws when the body is missing — a caller reading it would crash otherwise', async () => {
    fetchMock.mockImplementationOnce(async () => new Response(null, { status: 200 }))
    await expect(postStream('/api/chat', {})).rejects.toBeInstanceOf(HttpError)
  })
})

describe('the read deadline', () => {
  // A wedged connection used to park a read for ever — no fetch timeout
  // exists, so the query above never errored and never retried, and the
  // surface hung on its skeleton until the page was reloaded (the agents
  // roster reported exactly this). Reads now carry a deadline; mutations and
  // streams deliberately do not.
  it('every read carries an abort signal — the deadline is on the wire', async () => {
    await getJson('/api/fleet/defs')
    expect(lastInit().signal).toBeInstanceOf(AbortSignal)
  })

  it('a caller’s signal rides along with the deadline — aborting it fails the read', async () => {
    const caller = new AbortController()
    fetchMock.mockImplementationOnce(
      (_url: string | URL, init?: RequestInit) =>
        new Promise<Response>((_resolve, reject) => {
          init?.signal?.addEventListener('abort', () => reject(init.signal!.reason), { once: true })
        }),
    )
    // A caller that gives up while the request is in flight — TanStack
    // aborts exactly like this on invalidation.
    setTimeout(() => caller.abort(new Error('caller gave up')), 0)
    await expect(getJson('/api/fleet/defs', { signal: caller.signal })).rejects.toThrow('caller gave up')
  })

  it('mutations and streams carry no automatic deadline — long posts are a caller decision', async () => {
    await postJson('/api/fleet/create', { slug: 'x' })
    await postStream('/api/chat', { m: 1 })
    for (const [, init] of fetchMock.mock.calls) {
      expect(init?.signal).toBeUndefined()
    }
  })
})

// While the api restarts (every deploy), the app's proxy is still up and
// answers every /api/* path with 502 `{"error":{"message":"upstream
// unreachable"}}` (rust-proxy.ts) — the OpenAI shape, not the house string.
// Every door read only `data.error`, passed the OBJECT to `new Error()`, and
// every page loaded in that window said "Could not reach your session —
// [object Object]"; the hire modal said "[object Object]" (ui-2, fleet-4).
describe('the error sentence — both envelopes the product speaks', () => {
  const PROXY_DOWN = { error: { message: 'upstream unreachable' } }

  it('every door reads the proxy’s { error: { message } } as its sentence, never "[object Object]"', async () => {
    const doors: Array<[string, () => Promise<unknown>]> = [
      ['getJson (readJson)', () => getJson('/api/auth/session')],
      ['postJson (sendJson → readJson)', () => postJson('/api/fleet/create', { slug: 'x' })],
      ['postStream', () => postStream('/api/chat', { m: 1 })],
      ['getStream', () => getStream('/api/work/watch')],
    ]
    for (const [door, call] of doors) {
      fetchMock.mockImplementationOnce(async () => jsonResponse(502, PROXY_DOWN))
      const e = (await call().catch((x: unknown) => x)) as HttpError
      expect(e, door).toBeInstanceOf(HttpError)
      expect(e.status, door).toBe(502)
      expect(e.message, door).toBe('upstream unreachable')
      expect(errorMessage(e), door).toBe('Could not reach the server.')
    }
  })

  it('the house string still wins, and the OpenAI shape from the gateway passes its own sentence', () => {
    expect(errorSentence({ error: 'only the owner can change sharing' }, 403)).toBe('only the owner can change sharing')
    expect(errorSentence({ error: { message: 'rate limit exceeded for this key', type: 'rate_limit_exceeded' } }, 429)).toBe(
      'rate limit exceeded for this key',
    )
  })

  it('anything that is not a non-empty string is no sentence: the status line instead', () => {
    for (const body of [null, 'plain text', [], {}, { error: '' }, { error: '  ' }, { error: 42 }, { error: {} }, { error: { message: { nested: 1 } } }, { error: { message: '' } }, { error: null }]) {
      expect(errorSentence(body, 502), JSON.stringify(body)).toBe('request failed (502)')
    }
  })

  it('an HTML error page (the edge’s own 502/524) still reads as the status line', async () => {
    fetchMock.mockImplementationOnce(async () => new Response('<html>Bad gateway</html>', { status: 524, headers: { 'content-type': 'text/html' } }))
    await expect(getJson('/api/home')).rejects.toThrow('request failed (524)')
  })
})
