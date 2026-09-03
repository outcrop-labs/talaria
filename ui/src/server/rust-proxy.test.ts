// The boundary, tested without a Rust api (house rule: no service-dependent
// tests). fetch is stubbed per case; what's under test is the decision —
// after the cutover there is exactly one: every /api/* path forwards unless
// it is one of the four permanent residents, and the port-era table of
// per-batch prefixes is gone. What these cases lock in is the residents'
// edges, the default-on posture (unset env hops to the loopback default;
// only `off` stands the boundary down), and the mechanics of the hop itself.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { maybeProxy, DEFAULT_RUST_API_URL } from './rust-proxy'

const req = (path: string, init?: RequestInit) =>
  new Request(`http://talaria.test${path}`, init)

describe('maybeProxy', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.unstubAllEnvs()
  })

  it('unset env hops to the loopback default — first spin-up assumes the Rust api', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', '')
    let sawTarget = ''
    vi.stubGlobal(
      'fetch',
      (async (target: string) => {
        sawTarget = target
        return new Response('{}', { status: 200 })
      }) as unknown as typeof globalThis.fetch,
    )
    expect(await maybeProxy(req('/api/llm/v1/models'), '/api/llm/v1/models')).not.toBeNull()
    expect(sawTarget).toBe(`${DEFAULT_RUST_API_URL}/api/llm/v1/models`)
  })

  it("`off` forwards nothing — the hop stood down on purpose keeps its residents", async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'off')
    const fetch = vi.fn()
    vi.stubGlobal('fetch', fetch)
    expect(await maybeProxy(req('/api/llm/v1/models'), '/api/llm/v1/models')).toBeNull()
    expect(fetch).not.toHaveBeenCalled()
  })

  it('forwards with query intact, hop-by-hop stripped, response headers allow-listed', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    let sawTarget = ''
    let sawHeaders: Headers | null = null
    vi.stubGlobal(
      'fetch',
      (async (target: string, init: { headers: Headers }) => {
        sawTarget = target
        sawHeaders = init.headers
        return new Response('{"object":"list"}', { status: 200, headers: { 'content-type': 'application/json' } })
      }) as unknown as typeof globalThis.fetch,
    )

    const res = await maybeProxy(
      req('/api/llm/v1/models?limit=2', { headers: { authorization: 'Bearer tlk_x', host: 'talaria.test:80' } }),
      '/api/llm/v1/models',
    )
    expect(res).not.toBeNull()
    expect(res!.status).toBe(200)

    expect(sawTarget).toBe('http://127.0.0.1:5274/api/llm/v1/models?limit=2')
    expect(sawHeaders!.get('authorization')).toBe('Bearer tlk_x') // rides through
    expect(sawHeaders!.get('host')).toBeNull() // hop-by-hop dies here
    // The response allow-list: content-type through, nothing the Rust api
    // says about itself.
    expect(res!.headers.get('content-type')).toBe('application/json')
    expect(res!.headers.get('server')).toBeNull()
  })

  it('the one prefix carries the whole surface — one path from every family the port moved', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    for (const p of [
      '/api/llm/v1/chat/completions',
      '/api/auth/password',
      '/api/boards/mine',
      '/api/runs',
      '/api/runs/00000000-0000-4000-8000-000000000000',
      '/api/runs/00000000-0000-4000-8000-000000000000/events',
      '/api/runs/00000000-0000-4000-8000-000000000000/events/tail',
      '/api/runs/00000000-0000-4000-8000-000000000000/cancel',
      '/api/channels',
      '/api/channels/00000000-0000-4000-8000-000000000000/events',
      '/api/chat',
      '/api/conversations',
      '/api/dms',
      '/api/inbox/focus',
      '/api/brief',
      '/api/secrets/reveal',
      '/api/integrations/google/gmail/send',
      '/api/workbench/flow',
      '/api/mcp/gw/github',
      '/api/fleet/defs',
      '/api/agents/register',
      '/api/keys',
      '/api/teams',
      '/api/models',
      '/api/notifications',
      '/api/me',
      '/api/me/events',
      '/api/search',
      '/api/memory/00000000-0000-4000-8000-000000000000',
      '/api/definitely-not-a-route-the-table-never-knew',
    ]) {
      expect(await maybeProxy(req(p), p)).not.toBeNull()
    }
    // The last one is the point of the cutover: an unknown path is the Rust
    // api's 404 to give, not this process's — a new route exists on one side
    // only by design, and this boundary no longer decides per prefix.
    expect(fetch).toHaveBeenCalledTimes(30)
  })

  it('bare /api (no slash) is not proxied — no route ever lived at the bare path', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn()
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    expect(await maybeProxy(req('/api'), '/api')).toBeNull()
    expect(fetch).not.toHaveBeenCalled()
  })

  it('the four residents stay on TS — and their edges are anchored', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    // healthz — the app process's own liveness answer (Rust carries its own
    // at the same path for whoever asks it there).
    expect(await maybeProxy(req('/api/healthz'), '/api/healthz')).toBeNull()
    // admin.update — the deployer of the TS half itself, EXACT: a deeper or
    // extended path under the same characters is not it.
    expect(await maybeProxy(req('/api/admin/update'), '/api/admin/update')).toBeNull()
    expect(await maybeProxy(req('/api/admin/update/tail'), '/api/admin/update/tail')).not.toBeNull()
    expect(await maybeProxy(req('/api/admin/updates'), '/api/admin/updates')).not.toBeNull()
    // app dispatch (rule 10) — the whole subtree stays; the LISTING does not:
    // bare /api/apps is the discovery read, and it serves from Rust.
    expect(await maybeProxy(req('/api/apps/contacts/x'), '/api/apps/contacts/x')).toBeNull()
    expect(await maybeProxy(req('/api/apps/contacts'), '/api/apps/contacts')).toBeNull()
    expect(await maybeProxy(req('/api/apps'), '/api/apps')).not.toBeNull()
    // the app-MCP branch of the gateway — anchored to the app- name shape: a
    // registry server literally named "app-x" is indistinguishable, but
    // "appendix" is not.
    expect(await maybeProxy(req('/api/mcp/gw/app-contacts'), '/api/mcp/gw/app-contacts')).toBeNull()
    expect(await maybeProxy(req('/api/mcp/gw/app-anything/at/all'), '/api/mcp/gw/app-anything/at/all')).toBeNull()
    expect(await maybeProxy(req('/api/mcp/gw/appendix'), '/api/mcp/gw/appendix')).not.toBeNull()
    expect(fetch).toHaveBeenCalledTimes(4)
  })

  it('answers 502 with the fixed sentence when the Rust api is down — no fallback', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        throw new Error('connect ECONNREFUSED 127.0.0.1:5274')
      }) as unknown as typeof globalThis.fetch,
    )
    const res = await maybeProxy(req('/api/llm/v1/models'), '/api/llm/v1/models')
    expect(res).not.toBeNull()
    expect(res!.status).toBe(502)
    // The exact boundary sentence — the ECONNREFUSED text names the Rust
    // api's host:port and must not leak.
    expect(await res!.text()).toBe('{"error":{"message":"upstream unreachable"}}')
  })

  it('streams the upstream status and body through untouched', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response('{"error":{"message":"invalid API key"}}', { status: 401 })),
    )
    const res = await maybeProxy(req('/api/llm/v1/models'), '/api/llm/v1/models')
    expect(res!.status).toBe(401) // a 401 from Rust is a 401 to the caller
    expect(await res!.text()).toBe('{"error":{"message":"invalid API key"}}')
  })

  it('carries the no-store triad the secrets routes answer with — pragma and referrer-policy ride through', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    vi.stubGlobal(
      'fetch',
      vi.fn(
        async () =>
          new Response('{"value":"x"}', {
            status: 200,
            headers: {
              'content-type': 'application/json',
              'cache-control': 'no-store, no-cache, must-revalidate, private',
              pragma: 'no-cache',
              'referrer-policy': 'no-referrer',
              // A header the allow-list does not name: the Rust api's own
              // business, not ours to re-claim under this origin.
              server: 'talaria-api',
            },
          }),
      ),
    )
    const res = await maybeProxy(req('/api/secrets/reveal'), '/api/secrets/reveal')
    expect(res!.headers.get('cache-control')).toBe('no-store, no-cache, must-revalidate, private')
    expect(res!.headers.get('pragma')).toBe('no-cache')
    expect(res!.headers.get('referrer-policy')).toBe('no-referrer')
    expect(res!.headers.get('server')).toBeNull()
  })

  it('carries a download’s name and its sandbox belt — disposition, CSP, nosniff ride through', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    // Exactly what serve_upload answers for a non-inline file: the name in
    // the disposition, the belt in CSP + nosniff. Stripping these at this
    // boundary is the download half of "attachments are broken" — the file
    // arrives nameless, and the api's sandbox stops one hop short of the
    // browser that serves it.
    vi.stubGlobal(
      'fetch',
      vi.fn(
        async () =>
          new Response(new Uint8Array([1, 2, 3]), {
            status: 200,
            headers: {
              'content-type': 'application/octet-stream',
              'content-disposition': 'attachment; filename="big.bin"',
              'content-security-policy': "default-src 'none'; sandbox",
              'x-content-type-options': 'nosniff',
            },
          }),
      ),
    )
    const res = await maybeProxy(req('/api/uploads/some-id'), '/api/uploads/some-id')
    expect(res!.headers.get('content-disposition')).toBe('attachment; filename="big.bin"')
    expect(res!.headers.get('content-security-policy')).toBe("default-src 'none'; sandbox")
    expect(res!.headers.get('x-content-type-options')).toBe('nosniff')
  })
})
