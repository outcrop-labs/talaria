// The two routes the api's fitness plane calls on this host — the app
// harness listing and the one-fixture replay — and the guard in front of
// them: loopback, as the HTTP wrapper stamps it, AND the internal credential.
// Neither alone is enough.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { PEER_HEADER, isLoopbackAddress, stampPeer } from './http'
import { admitsInternal, requireInternal } from './internal-guard'
import { INTERNAL_HEADER } from './internal-api'
import { internalToken } from './secretbox'

const listing = vi.hoisted(() => ({ value: { harnesses: [] as unknown[], problems: [] as unknown[] } as { harnesses: unknown[]; problems: unknown[] } | Error }))

vi.mock('@/server/app-harnesses', () => ({
  listAppHarnesses: async () => {
    if (listing.value instanceof Error) throw listing.value
    return listing.value
  },
  wireOf: (h: { def: { id: string }; slug: string }) => ({ id: h.def.id, source: `app:${h.slug}` }),
  replayAppCase: async (_h: unknown, name: string, model: string) =>
    name === 'known' ? { harness: 'support:triage', case: name, model, verdict: 'pass' } : null,
  refreshAppHarnessReports: async () => {
    if (listing.value instanceof Error) throw listing.value
    return new Map([['support', { registered: ['support:triage'], problems: [] }]])
  },
}))

const { Route: List } = await import('@/routes/api/app-host.harnesses')
const { Route: Replay } = await import('@/routes/api/app-host.$app.harnesses.$id.cases.$name')

const saved = process.env.TALARIA_SECRET_KEY
beforeEach(() => {
  process.env.TALARIA_SECRET_KEY = 'root-secret'
  listing.value = { harnesses: [{ slug: 'support', def: { id: 'support:triage' } }], problems: [{ app: 'support', file: 'broken.ts', id: null, reason: 'x' }] }
})
afterEach(() => {
  process.env.TALARIA_SECRET_KEY = saved
})

const req = (path: string, opts: { peer?: string | null; token?: string | null; body?: unknown } = {}): Request => {
  const headers = new Headers()
  if (opts.peer !== null) headers.set(PEER_HEADER, opts.peer ?? 'loopback')
  if (opts.token !== null) headers.set(INTERNAL_HEADER, opts.token ?? internalToken())
  return new Request(`http://127.0.0.1:5273${path}`, {
    method: opts.body === undefined ? 'GET' : 'POST',
    headers,
    ...(opts.body === undefined ? {} : { body: JSON.stringify(opts.body) }),
  })
}

describe('the peer stamp', () => {
  it('knows loopback in every form node reports it', () => {
    for (const a of ['127.0.0.1', '127.8.9.10', '::1', '::ffff:127.0.0.1']) expect(isLoopbackAddress(a)).toBe(true)
    for (const a of ['10.0.0.5', '::ffff:10.0.0.5', '172.17.0.1', 'fe80::1', '', undefined]) expect(isLoopbackAddress(a)).toBe(false)
  })

  it("overwrites whatever the client claimed", () => {
    const h = new Headers({ [PEER_HEADER]: 'loopback' })
    stampPeer(h, '203.0.113.9')
    expect(h.get(PEER_HEADER)).toBe('remote')
    stampPeer(h, '127.0.0.1')
    expect(h.get(PEER_HEADER)).toBe('loopback')
  })
})

describe('requireInternal', () => {
  it('admits the api on this machine', () => {
    expect(requireInternal(req('/api/app-host/harnesses'))).toEqual({ ok: true })
  })

  it('refuses a caller off this machine, even holding the credential', async () => {
    const gate = requireInternal(req('/api/app-host/harnesses', { peer: 'remote' }))
    expect(gate).toBeInstanceOf(Response)
    expect((gate as Response).status).toBe(403)
    // A request no wrapper stamped reads as remote.
    expect(requireInternal(req('/api/app-host/harnesses', { peer: null }))).toBeInstanceOf(Response)
  })

  it('refuses a loopback caller without the credential, or with another root’s', () => {
    expect(requireInternal(req('/api/app-host/harnesses', { token: null }))).toBeInstanceOf(Response)
    expect(requireInternal(req('/api/app-host/harnesses', { token: 'f'.repeat(64) }))).toBeInstanceOf(Response)
    const other = internalToken()
    process.env.TALARIA_SECRET_KEY = 'another-root'
    expect(admitsInternal(other)).toBe(false)
  })

  it('admits nothing when no root is configured', () => {
    const env = { key: process.env.TALARIA_SECRET_KEY, file: process.env.TALARIA_SECRET_KEY_FILE, auth: process.env.AUTH_SECRET }
    delete process.env.TALARIA_SECRET_KEY
    delete process.env.TALARIA_SECRET_KEY_FILE
    delete process.env.AUTH_SECRET
    try {
      expect(admitsInternal('')).toBe(false)
    } finally {
      process.env.TALARIA_SECRET_KEY = env.key
      if (env.file !== undefined) process.env.TALARIA_SECRET_KEY_FILE = env.file
      if (env.auth !== undefined) process.env.AUTH_SECRET = env.auth
    }
  })
})

describe('GET /api/app-host/harnesses', () => {
  it('lists the harnesses as data, with the skipped ones named', async () => {
    const res = await List.handlers.GET!({ request: req('/api/app-host/harnesses'), params: {} })
    expect(res.status).toBe(200)
    expect(await res.json()).toEqual({
      harnesses: [{ id: 'support:triage', source: 'app:support' }],
      problems: [{ app: 'support', file: 'broken.ts', id: null, reason: 'x' }],
    })
  })

  it('refuses a caller without the credential', async () => {
    const res = await List.handlers.GET!({ request: req('/api/app-host/harnesses', { token: null }), params: {} })
    expect(res.status).toBe(403)
  })

  it('says the enabled set could not be read rather than answering an empty list', async () => {
    listing.value = new Error('the apps_enabled setting is not a list of app slugs')
    const res = await List.handlers.GET!({ request: req('/api/app-host/harnesses'), params: {} })
    expect(res.status).toBe(503)
  })
})

describe('POST /api/app-host/harnesses — the reports, rewritten after the enabled set changed', () => {
  it('rewrites every enabled app’s report for the api', async () => {
    const res = await List.handlers.POST!({ request: req('/api/app-host/harnesses', { body: {} }), params: {} })
    expect(res.status).toBe(200)
    expect(await res.json()).toEqual({ ok: true, apps: ['support'] })
  })

  it('refuses a caller without the credential', async () => {
    const res = await List.handlers.POST!({ request: req('/api/app-host/harnesses', { body: {}, token: null }), params: {} })
    expect(res.status).toBe(403)
  })

  it('says the enabled set could not be read', async () => {
    listing.value = new Error('the apps_enabled setting is not a list of app slugs')
    const res = await List.handlers.POST!({ request: req('/api/app-host/harnesses', { body: {} }), params: {} })
    expect(res.status).toBe(503)
  })
})

describe('POST /api/app-host/<slug>/harnesses/<id>/cases/<name>', () => {
  const post = (params: { app: string; id: string; name: string }, body: unknown, opts: { token?: string | null } = {}) =>
    Replay.handlers.POST!({ request: req(`/api/app-host/${params.app}/harnesses/${params.id}/cases/${params.name}`, { body, ...opts }), params })

  it('replays a fixture of an enabled app’s harness with the model it was given', async () => {
    const res = await post({ app: 'support', id: 'support:triage', name: 'known' }, { model: 'mock-small' })
    expect(res.status).toBe(200)
    expect(await res.json()).toEqual({ harness: 'support:triage', case: 'known', model: 'mock-small', verdict: 'pass' })
  })

  it('answers 404 for a harness or a fixture that is not there, and 400 without a model', async () => {
    expect((await post({ app: 'other', id: 'support:triage', name: 'known' }, { model: 'm' })).status).toBe(404)
    expect((await post({ app: 'support', id: 'support:nope', name: 'known' }, { model: 'm' })).status).toBe(404)
    expect((await post({ app: 'support', id: 'support:triage', name: 'unknown' }, { model: 'm' })).status).toBe(404)
    expect((await post({ app: 'support', id: 'support:triage', name: 'known' }, {})).status).toBe(400)
    expect((await post({ app: 'Not A Slug', id: 'support:triage', name: 'known' }, { model: 'm' })).status).toBe(400)
  })

  it('refuses a caller without the credential', async () => {
    expect((await post({ app: 'support', id: 'support:triage', name: 'known' }, { model: 'm' }, { token: null })).status).toBe(403)
  })
})
