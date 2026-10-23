// The pieces of an app's request context the host builds (app-context.ts):
// the sub-path's segments, the public origin, and ctx.api — the platform API
// as the caller. ctx.secret's rule is pinned in app-secrets.test.ts.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { HttpError } from '@/lib/fetch-json'
import { appOrigin, appPathSegments, platformApi } from './app-context'

describe('appPathSegments', () => {
  it('decodes each segment exactly once', () => {
    expect(appPathSegments('/api/apps/crm/files/100%25')).toEqual({ ok: true, segments: ['files', '100%'] })
    // A double-encoded escape stays an escape: one decode, not two.
    expect(appPathSegments('/api/apps/crm/files/a%2525b')).toEqual({ ok: true, segments: ['files', 'a%25b'] })
    expect(appPathSegments('/api/apps/crm/caf%C3%A9/a%20b')).toEqual({ ok: true, segments: ['café', 'a b'] })
  })

  it('is empty at the app root, and drops empty segments', () => {
    expect(appPathSegments('/api/apps/crm')).toEqual({ ok: true, segments: [] })
    expect(appPathSegments('/api/apps/crm/')).toEqual({ ok: true, segments: [] })
    expect(appPathSegments('/api/apps/crm//items//1/')).toEqual({ ok: true, segments: ['items', '1'] })
  })

  it('refuses an encoded slash rather than splitting or keeping it: a segment never holds "/"', () => {
    for (const p of ['/api/apps/crm/files/a%2Fb', '/api/apps/crm/files/a%2fb', '/api/apps/crm/admin%2Fsecrets']) {
      const r = appPathSegments(p)
      expect(r.ok).toBe(false)
      expect(r.ok ? '' : r.error).toContain('%2F')
    }
  })

  it('refuses a malformed escape', () => {
    expect(appPathSegments('/api/apps/crm/files/100%').ok).toBe(false)
    expect(appPathSegments('/api/apps/crm/%E0%A4%A').ok).toBe(false)
  })
})

describe('appOrigin', () => {
  const req = (url: string, headers: Record<string, string> = {}) => new Request(url, { headers })

  it('prefers AUTH_PUBLIC_URL', () => {
    expect(appOrigin(req('http://127.0.0.1:5273/api/apps/crm/x', { 'x-forwarded-host': 'evil.test' }), 'https://talaria.example.com/')).toBe(
      'https://talaria.example.com',
    )
  })

  it('then the proxy\'s forwarded proto and host, left-most of a chain', () => {
    const r = req('http://127.0.0.1:5273/api/apps/crm/x', { 'x-forwarded-proto': 'https, http', 'x-forwarded-host': 'talaria.example.com, inner:5273' })
    expect(appOrigin(r, null)).toBe('https://talaria.example.com')
  })

  it('then the request\'s own origin', () => {
    expect(appOrigin(req('http://localhost:5303/api/apps/crm/x'), null)).toBe('http://localhost:5303')
  })

  it('ignores a forwarded value that does not make an http(s) origin', () => {
    expect(appOrigin(req('http://localhost:5303/x', { 'x-forwarded-proto': 'javascript' }), null)).toBe('http://localhost:5303')
    expect(appOrigin(req('http://localhost:5303/x', { 'x-forwarded-proto': 'https', 'x-forwarded-host': 'a b/c' }), null)).toBe('http://localhost:5303')
  })
})

describe('platformApi (ctx.api)', () => {
  const BASE = () => 'http://api.test:5999'
  const calls: Array<{ url: string; init: RequestInit }> = []
  const answer = (status: number, body: string, type = 'application/json') =>
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: URL | string, init: RequestInit) => {
        calls.push({ url: String(url), init })
        return new Response(body === '' ? null : body, { status, headers: { 'content-type': type } })
      }),
    )
  const caller = (headers: Record<string, string> = {}, signal?: AbortSignal) =>
    new Request('https://talaria.example.com/api/apps/crm/sync', {
      headers: { cookie: 'talaria_session=abc', authorization: 'Bearer caller-token', 'x-other': 'no', ...headers },
      signal,
    })

  afterEach(() => {
    vi.unstubAllGlobals()
    calls.length = 0
  })

  it('calls the api with the caller\'s cookie and nothing else of theirs, and parses the JSON', async () => {
    answer(200, JSON.stringify({ boards: [{ id: 'b1' }] }))
    const api = platformApi(caller(), BASE)
    expect(await api('/api/boards?mine=1')).toEqual({ boards: [{ id: 'b1' }] })
    expect(calls).toHaveLength(1)
    expect(calls[0]!.url).toBe('http://api.test:5999/api/boards?mine=1')
    const h = new Headers(calls[0]!.init.headers)
    expect(h.get('cookie')).toBe('talaria_session=abc')
    expect(h.get('authorization')).toBeNull()
    expect(h.get('x-other')).toBeNull()
    expect(h.get('x-forwarded-proto')).toBe('https')
    expect(h.get('x-forwarded-host')).toBe('talaria.example.com')
    expect(calls[0]!.init.method).toBe('GET')
  })

  it('cannot be pointed at another identity: the app\'s own cookie and bearer headers are dropped', async () => {
    answer(200, '{}')
    await platformApi(caller(), BASE)('/api/me', { headers: { cookie: 'talaria_session=someone-else', authorization: 'Bearer app-key' } })
    const h = new Headers(calls[0]!.init.headers)
    expect(h.get('cookie')).toBe('talaria_session=abc')
    expect(h.get('authorization')).toBeNull()
  })

  it('sends an object body as JSON, and a string as is', async () => {
    answer(200, JSON.stringify({ task: { id: 't1' } }))
    const api = platformApi(caller(), BASE)
    await api('/api/boards/b1/tasks', { method: 'post', body: { title: 'Follow up' } })
    expect(calls[0]!.init.method).toBe('POST')
    expect(calls[0]!.init.body).toBe('{"title":"Follow up"}')
    expect(new Headers(calls[0]!.init.headers).get('content-type')).toBe('application/json')
    await api('/api/x', { method: 'PUT', body: 'raw', headers: { 'content-type': 'text/plain' } })
    expect(calls[1]!.init.body).toBe('raw')
    expect(new Headers(calls[1]!.init.headers).get('content-type')).toBe('text/plain')
  })

  it('throws an HttpError with the status and the server\'s sentence on non-2xx', async () => {
    answer(403, JSON.stringify({ error: 'you cannot see that board' }))
    const err = await platformApi(caller(), BASE)('/api/boards/b9/tasks').catch((e: unknown) => e)
    expect(err).toBeInstanceOf(HttpError)
    expect((err as HttpError).status).toBe(403)
    expect((err as HttpError).message).toBe('you cannot see that board')
    // The proxy's own envelope, and a body with no sentence at all.
    answer(502, JSON.stringify({ error: { message: 'upstream unreachable' } }))
    expect(((await platformApi(caller(), BASE)('/api/x').catch((e: unknown) => e)) as HttpError).message).toBe('upstream unreachable')
    answer(500, 'boom', 'text/plain')
    const plain = (await platformApi(caller(), BASE)('/api/x').catch((e: unknown) => e)) as HttpError
    expect(plain.status).toBe(500)
    expect(plain.message).toBe('request failed (500)')
  })

  it('answers null for an empty body and the text for a non-JSON one', async () => {
    answer(204, '')
    expect(await platformApi(caller(), BASE)('/api/x', { method: 'DELETE' })).toBeNull()
    answer(200, '# notes', 'text/markdown')
    expect(await platformApi(caller(), BASE)('/api/x')).toBe('# notes')
  })

  it('reaches the platform API only', async () => {
    answer(200, '{}')
    const api = platformApi(caller(), BASE)
    for (const bad of ['boards', 'http://evil.test/api/x', '//evil.test/api/x', '/apix', '/api/../admin', '/api/apps/other/x', '/api/app-host/crm/rebuild', '/api/mcp/gw/app-crm', '/api/internal/apps/mcp-sync', '/api/internal']) {
      await expect(api(bad)).rejects.toBeInstanceOf(TypeError)
    }
    expect(calls).toHaveLength(0)
    // A bare /api/apps (the discovery listing) is the api's.
    await api('/api/apps')
    expect(calls).toHaveLength(1)
  })

  it('answers 503 when the api is stood down', async () => {
    const err = (await platformApi(caller(), () => undefined)('/api/me').catch((e: unknown) => e)) as HttpError
    expect(err).toBeInstanceOf(HttpError)
    expect(err.status).toBe(503)
  })

  it('follows the request\'s signal: a caller who leaves cuts the call', async () => {
    const gone = new AbortController()
    let seen: AbortSignal | undefined
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (_url: URL, init: RequestInit) =>
          new Promise<Response>((_res, reject) => {
            seen = init.signal ?? undefined
            init.signal?.addEventListener('abort', () => reject(init.signal!.reason), { once: true })
          }),
      ),
    )
    const pending = platformApi(caller({}, gone.signal), BASE)('/api/boards', { method: 'POST', body: {} })
    await Promise.resolve()
    expect(seen?.aborted).toBe(false)
    gone.abort(new DOMException('the client disconnected', 'AbortError'))
    await expect(pending).rejects.toMatchObject({ name: 'AbortError' })
  })

  it('puts a deadline on reads and none on writes, unless told otherwise', async () => {
    vi.useFakeTimers()
    try {
      const hang = vi.fn(
        (_url: URL, init: RequestInit) =>
          new Promise<Response>((_res, reject) => {
            init.signal?.addEventListener('abort', () => reject(init.signal!.reason), { once: true })
          }),
      )
      vi.stubGlobal('fetch', hang)
      const read = platformApi(caller(), BASE)('/api/boards').catch((e: unknown) => e)
      const write = platformApi(caller(), BASE)('/api/boards', { method: 'POST', body: {} }).then(
        () => 'settled',
        () => 'settled',
      )
      await vi.advanceTimersByTimeAsync(30_000)
      expect(String(await read)).toMatch(/TimeoutError|timed out|aborted/i)
      const race = await Promise.race([write, vi.advanceTimersByTimeAsync(600_000).then(() => 'still-pending')])
      expect(race).toBe('still-pending')
      const short = platformApi(caller(), BASE)('/api/boards', { method: 'POST', body: {}, timeoutMs: 1_000 }).catch((e: unknown) => e)
      await vi.advanceTimersByTimeAsync(1_000)
      expect(String(await short)).toMatch(/TimeoutError|timed out|aborted/i)
    } finally {
      vi.useRealTimers()
    }
  })
})
