// The coexistence switch, tested without a Rust api (house rule: no
// service-dependent tests). fetch is stubbed per case; what's under test is
// the decision table — when to forward, what survives the hop, and the
// no-fallback posture when the Rust side is down.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { maybeProxy } from './rust-proxy'

const req = (path: string, init?: RequestInit) =>
  new Request(`http://talaria.test${path}`, init)

describe('maybeProxy', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.unstubAllEnvs()
  })

  it('forwards nothing when the env is unset — byte-identical TS behavior', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', '')
    const fetch = vi.fn()
    vi.stubGlobal('fetch', fetch)
    // Even a migrated prefix: no env, no proxy, no fetch call.
    expect(await maybeProxy(req('/api/llm/v1/models'), '/api/llm/v1/models')).toBeNull()
    expect(fetch).not.toHaveBeenCalled()
  })

  it('forwards migrated prefixes with query intact, hop-by-hop stripped', async () => {
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

  it('does not forward prefixes outside the switch list', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn()
    vi.stubGlobal('fetch', fetch)
    expect(await maybeProxy(req('/api/boards'), '/api/boards')).toBeNull()
    expect(await maybeProxy(req('/api/healthz'), '/api/healthz')).toBeNull()
    // Admin groups still on TS — invites sends email from its handler,
    // model-roles reads the effort-prefs plane; neither has crossed yet.
    expect(await maybeProxy(req('/api/admin/invites'), '/api/admin/invites')).toBeNull()
    expect(await maybeProxy(req('/api/admin/model-roles'), '/api/admin/model-roles')).toBeNull()
    expect(fetch).not.toHaveBeenCalled()
  })

  it('forwards the admin wave-1 groups, /login riding under its parent', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    for (const p of [
      '/api/agent-role-templates',
      '/api/admin/password-accounts',
      '/api/admin/google-client',
      '/api/admin/google-client/login',
      '/api/admin/instance',
      '/api/admin/permissions',
    ]) {
      expect(await maybeProxy(req(p), p)).not.toBeNull()
    }
    expect(fetch).toHaveBeenCalledTimes(6)
  })

  it('forwards the keys group — the list and the id route under one prefix', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    for (const p of ['/api/keys', '/api/keys/00000000-0000-4000-8000-000000000000']) {
      expect(await maybeProxy(req(p), p)).not.toBeNull()
    }
    expect(fetch).toHaveBeenCalledTimes(2)
  })

  it('forwards the teams group — list, id, and members under one prefix', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    for (const p of [
      '/api/teams',
      '/api/teams/00000000-0000-4000-8000-000000000000',
      '/api/teams/00000000-0000-4000-8000-000000000000/members',
    ]) {
      expect(await maybeProxy(req(p), p)).not.toBeNull()
    }
    expect(fetch).toHaveBeenCalledTimes(3)
  })

  it('forwards exact-match routes but not the TS sub-routes under them', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    // The route IS the group…
    expect(await maybeProxy(req('/api/agents'), '/api/agents')).not.toBeNull()
    expect(await maybeProxy(req('/api/apps'), '/api/apps')).not.toBeNull()
    // …but its sub-paths still belong to TS: register/heartbeat (the fleet
    // plane) and the app-server gateway would land on a Rust 404.
    expect(await maybeProxy(req('/api/agents/register'), '/api/agents/register')).toBeNull()
    expect(await maybeProxy(req('/api/agents/x/heartbeat'), '/api/agents/x/heartbeat')).toBeNull()
    expect(await maybeProxy(req('/api/apps/contacts/x'), '/api/apps/contacts/x')).toBeNull()
    expect(fetch).toHaveBeenCalledTimes(2)
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
    // The exact boundary sentence from llm.v1.chat.completions.ts — the
    // ECONNREFUSED text names the Rust api's host:port and must not leak.
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
})
