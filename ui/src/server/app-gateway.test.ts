// The app API gateway's wiring (routes/api/apps.$app.$.ts), end to end inside
// the process: the real session read (against a stubbed api), the real gate,
// and the context the app's own fetch receives. The pure decision is pinned in
// app-access.test.ts; this proves the route hands its outcome to the app —
// `access`, `perms` and `can` — and never runs the app on a refusal.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AppRequestContext } from '@/sdk/server'
import type { AppManifest } from './apps'

const CRM: AppManifest = {
  slug: 'crm',
  name: 'CRM',
  icon: '⬡',
  description: '',
  version: '0.0.0',
  surfaces: { work: 'CRM', manage: 'CRM Admin', settings: 'CRM' },
  mcp: false,
}
const seen: AppRequestContext[] = []
const appFetch = vi.fn(async (_req: Request, ctx: AppRequestContext) => {
  seen.push(ctx)
  return new Response(JSON.stringify({ ok: true }), { headers: { 'content-type': 'application/json' } })
})
vi.mock('./apps', () => ({
  enabledApps: vi.fn(async () => [CRM]),
  appServerModule: vi.fn(async () => ({ default: { fetch: appFetch } })),
}))
vi.mock('./app-store', () => ({ storeFor: vi.fn(() => ({})) }))

const { Route } = await import('@/routes/api/apps.$app.$')

const member = { id: 'u1', sub: 'password:u1', email: 'u@t.local', name: 'U', picture: null, provider: 'password', role: 'member' }
const api = (status: number, body: unknown) => vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(body), { status })))
const call = (path: string) =>
  Route.handlers.GET!({
    request: new Request(`http://app.test/api/apps/crm/${path}`, { headers: { cookie: 'talaria_session=abc' } }),
    params: { app: 'crm', _splat: path },
  })

describe('the app API gateway', () => {
  const prev = process.env.TALARIA_RUST_API_URL
  beforeEach(() => {
    process.env.TALARIA_RUST_API_URL = 'http://api.test:5999'
    seen.length = 0
    appFetch.mockClear()
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    if (prev === undefined) delete process.env.TALARIA_RUST_API_URL
    else process.env.TALARIA_RUST_API_URL = prev
  })

  it('hands the app the caller\'s access, permissions and can()', async () => {
    api(200, { user: member, deniedViews: ['/boards'], appGrants: ['/x/crm/manage'], perms: ['kb.edit'] })
    const res = await call('admin/fields')
    expect(res.status).toBe(200)
    expect(seen).toHaveLength(1)
    const ctx = seen[0]!
    expect(ctx.app).toBe('crm')
    expect(ctx.path).toBe('admin/fields')
    expect(ctx.user).toMatchObject({ id: 'u1', role: 'member' })
    expect(ctx.access).toEqual({ admin: false, work: false, manage: true, settings: true })
    expect(ctx.perms).toEqual(['kb.edit'])
    expect(ctx.can('kb.edit')).toBe(true)
    expect(ctx.can('agents.manage')).toBe(false)
  })

  it('gives the app its own copies: app code cannot edit the gate\'s answer', async () => {
    api(200, { user: member, deniedViews: [], appGrants: ['/x/crm'], perms: ['kb.edit'] })
    await call('a')
    const ctx = seen[0]!
    ;(ctx.perms as string[]).push('agents.manage')
    ctx.access.manage = true
    expect(ctx.can('agents.manage')).toBe(false)
  })

  it('never runs the app for a caller the gate refuses, or when it cannot tell', async () => {
    api(200, { user: member, deniedViews: [], appGrants: [], perms: [] })
    expect((await call('a')).status).toBe(403)
    api(200, { user: null, deniedViews: [], appGrants: [], perms: [] })
    expect((await call('a')).status).toBe(401)
    api(500, { error: 'internal' })
    expect((await call('a')).status).toBe(503)
    expect(appFetch).not.toHaveBeenCalled()
  })
})
