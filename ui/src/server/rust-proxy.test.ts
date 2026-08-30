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
    expect(await maybeProxy(req('/api/healthz'), '/api/healthz')).toBeNull()
    // Admin groups still on TS — invites sends email from its handler,
    // model-fitness is the probe suite's own plane.
    expect(await maybeProxy(req('/api/admin/invites'), '/api/admin/invites')).toBeNull()
    expect(await maybeProxy(req('/api/admin/model-fitness'), '/api/admin/model-fitness')).toBeNull()
    // The comms siblings are NOT under the channels prefix — conversations and
    // dms are their own route families and stay TS until the chat batch.
    expect(await maybeProxy(req('/api/conversations'), '/api/conversations')).toBeNull()
    expect(await maybeProxy(req('/api/dms'), '/api/dms')).toBeNull()
    expect(fetch).not.toHaveBeenCalled()
  })

  it('forwards the comms plane whole — every channels.* route under one prefix', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    for (const p of [
      '/api/channels',
      '/api/channels/00000000-0000-4000-8000-000000000000',
      '/api/channels/00000000-0000-4000-8000-000000000000/agents',
      '/api/channels/00000000-0000-4000-8000-000000000000/conclude',
      '/api/channels/00000000-0000-4000-8000-000000000000/events',
      '/api/channels/00000000-0000-4000-8000-000000000000/members',
      '/api/channels/00000000-0000-4000-8000-000000000000/messages',
      '/api/channels/00000000-0000-4000-8000-000000000000/messages/00000000-0000-4000-8000-000000000000',
      '/api/channels/00000000-0000-4000-8000-000000000000/messages/00000000-0000-4000-8000-000000000000/reactions',
      '/api/channels/00000000-0000-4000-8000-000000000000/read',
      // The plan-draft mount that crossed earlier — the prefix subsumes the
      // SHAPES entry it used to need.
      '/api/channels/00000000-0000-4000-8000-000000000000/plan',
    ]) {
      expect(await maybeProxy(req(p), p)).not.toBeNull()
    }
    expect(fetch).toHaveBeenCalledTimes(11)
  })

  it('forwards the model-identity plane — the picker catalog and the effort feed under one prefix', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    for (const p of ['/api/models', '/api/models/efforts']) {
      expect(await maybeProxy(req(p), p)).not.toBeNull()
    }
    expect(fetch).toHaveBeenCalledTimes(2)
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

  it('forwards the workflows group — list and id under one prefix', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    for (const p of ['/api/workflows', '/api/workflows/00000000-0000-4000-8000-000000000000']) {
      expect(await maybeProxy(req(p), p)).not.toBeNull()
    }
    expect(fetch).toHaveBeenCalledTimes(2)
  })

  it('forwards the notifications route — the bell and the settings panel under one path', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    expect(await maybeProxy(req('/api/notifications'), '/api/notifications')).not.toBeNull()
    expect(fetch).toHaveBeenCalledTimes(1)
  })

  it('forwards /api/me exactly — the profile and the firehose, not the other me.* planes', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    expect(await maybeProxy(req('/api/me'), '/api/me')).not.toBeNull()
    // me/events crossed with the realtime slice — the person's own SSE
    // firehose, served by Rust's dedicated-subscriber streams.
    expect(await maybeProxy(req('/api/me/events'), '/api/me/events')).not.toBeNull()
    // The remaining me.* siblings are their own planes — mcp (fleet render)
    // and assistant (agent start) — and stay TS until those batches.
    expect(await maybeProxy(req('/api/me/mcp'), '/api/me/mcp')).toBeNull()
    expect(await maybeProxy(req('/api/me/assistant'), '/api/me/assistant')).toBeNull()
    expect(fetch).toHaveBeenCalledTimes(2)
  })

  it('forwards the run watch stream by path SHAPE — the id is in the path, and the rest of /api/runs is still TS', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    expect(
      await maybeProxy(req('/api/runs/00000000-0000-4000-8000-000000000000/events'), '/api/runs/00000000-0000-4000-8000-000000000000/events'),
    ).not.toBeNull()
    // The shape is anchored: deeper or shallower paths do not match.
    expect(await maybeProxy(req('/api/runs/x/events/tail'), '/api/runs/x/events/tail')).toBeNull()
    expect(await maybeProxy(req('/api/runs/events'), '/api/runs/events')).toBeNull()
    // The rest of the runs surface still belongs to TS — a /api/runs prefix
    // would strand the list, the detail, cancel, and decide on a Rust 404.
    expect(await maybeProxy(req('/api/runs'), '/api/runs')).toBeNull()
    expect(await maybeProxy(req('/api/runs/00000000-0000-4000-8000-000000000000'), '/api/runs/00000000-0000-4000-8000-000000000000')).toBeNull()
    expect(await maybeProxy(req('/api/runs/00000000-0000-4000-8000-000000000000/cancel'), '/api/runs/00000000-0000-4000-8000-000000000000/cancel')).toBeNull()
    expect(fetch).toHaveBeenCalledTimes(1)
  })

  it('forwards exact-match routes but not the TS sub-routes under them', async () => {
    vi.stubEnv('TALARIA_RUST_API_URL', 'http://127.0.0.1:5274')
    const fetch = vi.fn(async () => new Response('{}', { status: 200 }))
    vi.stubGlobal('fetch', fetch as unknown as typeof globalThis.fetch)
    // The route IS the group…
    expect(await maybeProxy(req('/api/agents'), '/api/agents')).not.toBeNull()
    expect(await maybeProxy(req('/api/apps'), '/api/apps')).not.toBeNull()
    expect(await maybeProxy(req('/api/admin/model-roles'), '/api/admin/model-roles')).not.toBeNull()
    // …but its sub-paths still belong to TS: register/heartbeat (the fleet
    // plane) and the app-server gateway would land on a Rust 404.
    expect(await maybeProxy(req('/api/agents/register'), '/api/agents/register')).toBeNull()
    expect(await maybeProxy(req('/api/agents/x/heartbeat'), '/api/agents/x/heartbeat')).toBeNull()
    expect(await maybeProxy(req('/api/apps/contacts/x'), '/api/apps/contacts/x')).toBeNull()
    expect(fetch).toHaveBeenCalledTimes(3)
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
