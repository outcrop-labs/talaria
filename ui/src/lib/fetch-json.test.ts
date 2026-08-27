import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { delJson, HttpError, postJson, postJsonOr, postStream, putJson, readJson } from '@/lib/fetch-json'

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
