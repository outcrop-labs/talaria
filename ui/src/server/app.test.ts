// The one fetch handler's routing of /app-builds/: the door gets the RAW
// request before anything decodes the path. The door decodes each segment
// once itself (app-builds-door.ts), so an encoded slash stays inside its
// segment and a malformed escape is the door's 404 — not a URIError thrown by
// a decode of the whole path ahead of it.
import { describe, expect, it, vi } from 'vitest'

const serveAppBuild = vi.fn(async (_req: Request) => new Response('door', { status: 299 }))
vi.mock('./app-builds-door', () => ({ APP_BUILDS_PREFIX: '/app-builds/', serveAppBuild }))
const maybeProxy = vi.fn(async (_req: Request, _pathname: string): Promise<Response | null> => null)
vi.mock('./rust-proxy', async (orig) => ({ ...(await orig<typeof import('./rust-proxy')>()), maybeProxy }))

const { default: server } = await import('./app')

describe('server/app.ts', () => {
  it('sends /app-builds/ to the door undecoded', async () => {
    // %E0%A4%A is a malformed escape: decodeURIComponent on the whole path
    // throws on it. The door must see it first, as sent.
    const url = 'http://app.test/app-builds/crm/abc123/assets/%E0%A4%A.js'
    const res = await server.fetch(new Request(url))
    expect(res.status).toBe(299)
    expect(serveAppBuild).toHaveBeenCalledTimes(1)
    expect(serveAppBuild.mock.calls[0]![0].url).toBe(url)

    const encoded = 'http://app.test/app-builds/crm/abc123/assets%2F..%2Fserver.js'
    await server.fetch(new Request(encoded))
    expect(new URL(serveAppBuild.mock.calls[1]![0].url).pathname).toBe('/app-builds/crm/abc123/assets%2F..%2Fserver.js')
  })

  it('does not send anything else to the door', async () => {
    serveAppBuild.mockClear()
    await server.fetch(new Request('http://app.test/api/healthz'))
    await server.fetch(new Request('http://app.test/xapp-builds/crm/abc/app.js'))
    expect(serveAppBuild).not.toHaveBeenCalled()
  })

  it('answers a malformed escape under /api with a 400, never a thrown decode', async () => {
    maybeProxy.mockClear()
    for (const path of ['/api/healthz%', '/api/apps/crm/100%', '/api/boards/%E0%A4%A']) {
      const res = await server.fetch(new Request(`http://app.test${path}`))
      expect(res.status).toBe(400)
      expect(await res.json()).toEqual({ error: 'malformed path: a percent-escape does not decode' })
    }
    expect(maybeProxy).not.toHaveBeenCalled()
    // Outside /api it is no route: the SPA's answer, not a 500.
    expect((await server.fetch(new Request('http://app.test/x/%E0%A4%A'))).status).not.toBe(500)
  })

  it('hands the proxy the path as sent, still encoded', async () => {
    maybeProxy.mockClear()
    maybeProxy.mockImplementationOnce(async () => new Response('{}', { status: 200 }))
    await server.fetch(new Request('http://app.test/api/boards/a%2Fb/100%25'))
    expect(maybeProxy.mock.calls[0]![1]).toBe('/api/boards/a%2Fb/100%25')
  })

  it('routes a path holding %25 to its resident instead of throwing', async () => {
    // healthz is a resident with no params; a literal % elsewhere in the
    // table's paths reaches matching as one decoded segment.
    const res = await server.fetch(new Request('http://app.test/api/healthz%25'))
    expect(res.status).toBe(404)
  })
})
