// The one fetch handler's routing of /app-builds/: the door gets the RAW
// request before anything decodes the path. The door decodes each segment
// once itself (app-builds-door.ts), so an encoded slash stays inside its
// segment and a malformed escape is the door's 404 — not a URIError thrown by
// a decode of the whole path ahead of it.
import { describe, expect, it, vi } from 'vitest'

const serveAppBuild = vi.fn(async (_req: Request) => new Response('door', { status: 299 }))
vi.mock('./app-builds-door', () => ({ APP_BUILDS_PREFIX: '/app-builds/', serveAppBuild }))
vi.mock('./rust-proxy', async (orig) => ({ ...(await orig<typeof import('./rust-proxy')>()), maybeProxy: vi.fn(async () => null) }))

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
})
