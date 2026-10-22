import { QueryClient } from '@tanstack/svelte-query'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { HttpError } from '@/lib/fetch-json'
import { api, appApi, appApiFor, appQueryKey } from './app-client'

// The SDK's HTTP layer and its query keys. Two behaviours an app author relies
// on without reading the source: invalidate('things') refreshes the list the
// docs pair it with (it never did — the key was the raw string), and a failed
// call says which status failed (it never did — Error('500')).

describe('appQueryKey', () => {
  it('builds the key from segments, then search params', () => {
    expect(appQueryKey('crm', 'things')).toEqual(['app', 'crm', 'things'])
    expect(appQueryKey('crm', '/things/123/')).toEqual(['app', 'crm', 'things', '123'])
    expect(appQueryKey('crm', 'things?q=ada&tag=a&tag=b')).toEqual(['app', 'crm', 'things', { q: 'ada', tag: ['a', 'b'] }])
  })

  it("lets invalidate('things') reach every read under things, and nothing beside it", async () => {
    const qc = new QueryClient()
    const reads = ['things', 'things?q=ada', 'things/123', 'things/123?tab=notes', 'thingstore', 'other']
    for (const p of reads) qc.setQueryData(appQueryKey('crm', p), { p })
    qc.setQueryData(appQueryKey('another-app', 'things'), {})

    await qc.invalidateQueries({ queryKey: appQueryKey('crm', 'things') })
    const stale = qc
      .getQueryCache()
      .getAll()
      .filter((q) => q.state.isInvalidated)
      .map((q) => (q.queryKey[1] === 'crm' ? (q.state.data as { p: string }).p : `${String(q.queryKey[1])}:things`))
    expect(stale.sort()).toEqual(['things', 'things/123', 'things/123?tab=notes', 'things?q=ada'])
  })

  it('narrows to one record, or to one search', async () => {
    const qc = new QueryClient()
    for (const p of ['things?q=ada', 'things?q=bob', 'things/1', 'things/2']) qc.setQueryData(appQueryKey('crm', p), { p })
    await qc.invalidateQueries({ queryKey: appQueryKey('crm', 'things/1') })
    await qc.invalidateQueries({ queryKey: appQueryKey('crm', 'things?q=ada') })
    const stale = qc
      .getQueryCache()
      .getAll()
      .filter((q) => q.state.isInvalidated)
      .map((q) => (q.state.data as { p: string }).p)
    expect(stale.sort()).toEqual(['things/1', 'things?q=ada'])
  })
})

describe('api() and the bound verbs', () => {
  const calls: Array<{ url: string; init: RequestInit }> = []
  const reply = (status: number, body: string, type = 'application/json') =>
    vi.fn(async (url: string, init: RequestInit) => {
      calls.push({ url, init })
      return new Response(body, { status, headers: { 'content-type': type } })
    })

  afterEach(() => {
    calls.length = 0
    vi.unstubAllGlobals()
  })

  it('rejects a non-2xx with an HttpError carrying the status and the server sentence', async () => {
    vi.stubGlobal('fetch', reply(403, '{"error":"you cannot see this board"}'))
    const err = await api('/api/boards/x').catch((e: unknown) => e)
    expect(err).toBeInstanceOf(HttpError)
    expect((err as HttpError).status).toBe(403)
    expect((err as HttpError).message).toBe('you cannot see this board')
  })

  it('rejects a text 500 with its status, and a 2xx that is not JSON with a sentence', async () => {
    vi.stubGlobal('fetch', reply(500, 'Internal Server Error', 'text/plain'))
    expect(await api('/api/x').catch((e: HttpError) => [e.status, e.message])).toEqual([500, 'request failed (500)'])
    vi.stubGlobal('fetch', reply(200, '<html>', 'text/html'))
    expect(await api('/api/x').catch((e: HttpError) => [e.status, e.message])).toEqual([200, 'The server sent a reply this app could not read.'])
  })

  it('gives every read a deadline and combines it with the caller signal and timeout', async () => {
    vi.stubGlobal('fetch', reply(200, '{"ok":true}'))
    const mine = new AbortController()
    await api('/api/x', { signal: mine.signal, timeout: 5_000 })
    const signal = calls[0]?.init.signal
    expect(signal).toBeInstanceOf(AbortSignal)
    expect(signal).not.toBe(mine.signal)
    mine.abort()
    expect(signal?.aborted).toBe(true)
  })

  it('aborts a write after its timeout', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(
        (_url: string, init: RequestInit) =>
          new Promise<Response>((_, reject) => init.signal?.addEventListener('abort', () => reject(init.signal?.reason))),
      ),
    )
    const err = await api('/api/slow', { method: 'POST', json: {}, timeout: 20 }).catch((e: unknown) => e)
    expect((err as Error).name).toBe('TimeoutError')
  })

  it('sends a JSON body with its content-type, even beside caller headers, and a raw body untouched', async () => {
    vi.stubGlobal('fetch', reply(200, '{"ok":true}'))
    await api('/api/x', { method: 'post', json: { a: 1 }, headers: { 'x-trace': '1' } })
    expect(calls[0]?.init.method).toBe('POST')
    expect(calls[0]?.init.body).toBe('{"a":1}')
    expect(calls[0]?.init.headers).toEqual({ 'content-type': 'application/json', 'x-trace': '1' })
    await api('/api/upload', { method: 'PUT', body: 'raw text' })
    expect(calls[1]?.init.body).toBe('raw text')
    expect(calls[1]?.init.headers).toBeUndefined()
  })

  it("binds the verbs to /api/apps/<slug>/, reading the slug at call time", async () => {
    vi.stubGlobal('fetch', reply(200, '{"ok":true}'))
    let slug = 'first'
    const live = appApiFor(() => slug)
    await live.post('items', { title: 'a' })
    slug = 'renamed'
    await live.del('/items/1')
    await appApi('crm').get('things?q=ada')
    expect(calls.map((c) => `${c.init.method ?? 'GET'} ${c.url}`)).toEqual([
      'POST /api/apps/first/items',
      'DELETE /api/apps/renamed/items/1',
      'GET /api/apps/crm/things?q=ada',
    ])
    expect(calls[0]?.init.body).toBe('{"title":"a"}')
    expect(live.url('events')).toBe('/api/apps/renamed/events')
  })
})
