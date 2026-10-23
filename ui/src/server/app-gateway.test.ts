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
let appBody: (req: Request, ctx: AppRequestContext) => Promise<Response> = async () =>
  new Response(JSON.stringify({ ok: true }), { headers: { 'content-type': 'application/json' } })
const appFetch = vi.fn(async (req: Request, ctx: AppRequestContext) => {
  seen.push(ctx)
  return appBody(req, ctx)
})
vi.mock('./apps', () => ({
  enabledApps: vi.fn(async () => [CRM]),
  appServerModule: vi.fn(async () => ({ default: { fetch: appFetch } })),
}))
vi.mock('./app-store', () => ({ storeFor: vi.fn(() => ({})) }))

const { Route } = await import('@/routes/api/apps.$app.$')

const member = { id: 'u1', sub: 'password:u1', email: 'u@t.local', name: 'U', picture: null, provider: 'password', role: 'member' }
const api = (status: number, body: unknown) => vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(body), { status })))
const call = (path: string, signal?: AbortSignal) =>
  Route.handlers.GET!({
    request: new Request(`http://app.test/api/apps/crm/${path}`, { headers: { cookie: 'talaria_session=abc' }, signal }),
    params: { app: 'crm', _splat: path },
  })

describe('the app API gateway', () => {
  const prev = process.env.TALARIA_RUST_API_URL
  beforeEach(() => {
    process.env.TALARIA_RUST_API_URL = 'http://api.test:5999'
    seen.length = 0
    appFetch.mockClear()
    appBody = async () => new Response(JSON.stringify({ ok: true }), { headers: { 'content-type': 'application/json' } })
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
    // The api could not read the enabled apps: the session answered, but
    // with no app grants — the app never runs on it.
    api(200, { user: member, deniedViews: [], appGrants: null, appsUnreadable: true, perms: ['kb.edit'] })
    expect((await call('a')).status).toBe(503)
    expect(appFetch).not.toHaveBeenCalled()
  })

  it('hands the app the sub-path decoded once, as segments', async () => {
    api(200, { user: member, deniedViews: [], appGrants: ['/x/crm'], perms: [] })
    expect((await call('files/100%25')).status).toBe(200)
    expect(seen[0]!.segments).toEqual(['files', '100%'])
    expect(seen[0]!.path).toBe('files/100%')
    await call('caf%C3%A9/a%20b/')
    expect(seen[1]!.segments).toEqual(['café', 'a b'])
    await call('')
    expect(seen[2]!.segments).toEqual([])
    expect(seen[2]!.path).toBe('')
  })

  it('refuses an encoded slash and a malformed escape with a 400, before the app runs', async () => {
    api(200, { user: member, deniedViews: [], appGrants: ['/x/crm'], perms: [] })
    const slash = await call('admin%2Fsecrets')
    expect(slash.status).toBe(400)
    expect(((await slash.json()) as { error: string }).error).toContain('%2F')
    expect((await call('files/100%')).status).toBe(400)
    expect(appFetch).not.toHaveBeenCalled()
  })

  it('hands the app origin, signal, api and secret', async () => {
    api(200, { user: member, deniedViews: [], appGrants: ['/x/crm'], perms: [] })
    await call('a')
    const ctx = seen[0]!
    expect(ctx.origin).toBe('http://app.test')
    expect(ctx.signal).toBeInstanceOf(AbortSignal)
    expect(ctx.signal.aborted).toBe(false)
    expect(typeof ctx.api).toBe('function')
    expect(typeof ctx.secret).toBe('function')
    // ctx.api runs as the caller: their cookie, to the api.
    const fetch = vi.fn(async () => new Response(JSON.stringify({ user: member }), { headers: { 'content-type': 'application/json' } }))
    vi.stubGlobal('fetch', fetch)
    expect(await ctx.api('/api/auth/session')).toEqual({ user: member })
    const [url, init] = fetch.mock.calls[0] as unknown as [URL, RequestInit]
    expect(String(url)).toBe('http://api.test:5999/api/auth/session')
    expect(new Headers(init.headers).get('cookie')).toBe('talaria_session=abc')
  })

  it('a handler cut because the caller left is not an app crash', async () => {
    api(200, { user: member, deniedViews: [], appGrants: ['/x/crm'], perms: [] })
    const gone = new AbortController()
    appBody = (_req, ctx) =>
      new Promise((_resolve, reject) => {
        ctx.signal.addEventListener('abort', () => reject(ctx.signal.reason), { once: true })
      })
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {})
    const pending = call('slow', gone.signal)
    await vi.waitFor(() => expect(appFetch).toHaveBeenCalled())
    gone.abort(new DOMException('the client disconnected', 'AbortError'))
    const res = await pending
    expect(res.status).toBe(499)
    expect(errors).not.toHaveBeenCalled()
    errors.mockRestore()
    // A throw while the caller is still there is still a crash.
    appBody = async () => {
      throw new Error('boom')
    }
    const logged = vi.spyOn(console, 'error').mockImplementation(() => {})
    const crashed = await call('x')
    expect(crashed.status).toBe(500)
    expect(await crashed.json()).toEqual({ error: 'app crashed', detail: 'boom' })
    expect(logged).toHaveBeenCalled()
    logged.mockRestore()
  })
})
