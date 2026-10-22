// The app gate's decision and the session read it rests on. The gate admits a
// caller holding ANY of the app's surfaces and tells the app which; the
// session comes from the Rust api with the caller's cookie, and a session the
// api cannot answer is never read as "no denials".
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('./apps', () => ({ enabledApps: vi.fn(async () => []) }))

const { canFor, decideAppGate } = await import('./app-access')
const { rustSession } = await import('./rust-session')
import type { AppManifest } from './apps'
import type { RustSession } from './rust-session'

const manifest = (slug: string, surfaces: AppManifest['surfaces']): AppManifest => ({
  slug,
  name: slug,
  icon: '⬡',
  description: '',
  version: '0.0.0',
  surfaces,
  mcp: false,
})
const APPS = [
  manifest('crm', { work: 'CRM', manage: 'CRM Admin', settings: 'CRM' }),
  manifest('prefs', { settings: 'Prefs' }),
  manifest('bare', {}),
]
const user = (role: 'admin' | 'member') => ({ id: 'u1', sub: 'password:u1', email: 'u@t.local', name: 'U', picture: null, provider: 'password' as const, role })
const session = (role: 'admin' | 'member' | null, deniedViews: string[] = [], perms: string[] = []): RustSession => ({
  user: role ? user(role) : null,
  deniedViews,
  perms,
})
// Rust denies every app view a member was not granted.
const ALL_APP_VIEWS = ['/x/crm', '/x/crm/manage', '/x/prefs/manage']
const except = (...granted: string[]) => ALL_APP_VIEWS.filter((v) => !granted.includes(v))

const status = (g: ReturnType<typeof decideAppGate>) => (g.ok ? 200 : g.response.status)

describe('decideAppGate', () => {
  it('answers 401 to nobody before saying anything about the app', () => {
    expect(status(decideAppGate(session(null), APPS, 'crm'))).toBe(401)
    expect(status(decideAppGate(session(null), APPS, 'nope'))).toBe(401)
  })

  it('answers 404 for an app that is not enabled', () => {
    expect(status(decideAppGate(session('member'), APPS, 'nope'))).toBe(404)
  })

  it('refuses a member who holds no surface of the app', () => {
    expect(status(decideAppGate(session('member', except()), APPS, 'crm'))).toBe(403)
    // A settings-only app is gated too — it used to be open to every member.
    expect(status(decideAppGate(session('member', except()), APPS, 'prefs'))).toBe(403)
    // Nothing to grant: members never reach it.
    expect(status(decideAppGate(session('member', except()), APPS, 'bare'))).toBe(403)
  })

  it('admits a work-only member and says manage is not theirs', () => {
    const g = decideAppGate(session('member', except('/x/crm')), APPS, 'crm')
    expect(g.ok && g.caller.access).toEqual({ admin: false, work: true, manage: false, settings: false })
  })

  it('admits a manage-only member — the grant that used to do nothing', () => {
    const g = decideAppGate(session('member', except('/x/crm/manage')), APPS, 'crm')
    expect(g.ok && g.caller.access).toEqual({ admin: false, work: false, manage: true, settings: true })
  })

  it('admits a settings-only app to the member granted its manage view', () => {
    const g = decideAppGate(session('member', except('/x/prefs/manage')), APPS, 'prefs')
    expect(g.ok && g.caller.access).toEqual({ admin: false, work: false, manage: false, settings: true })
  })

  it('admits an admin to everything the app ships', () => {
    const g = decideAppGate(session('admin'), APPS, 'crm')
    expect(g.ok && g.caller.access).toEqual({ admin: true, work: true, manage: true, settings: true })
    expect(status(decideAppGate(session('admin'), APPS, 'bare'))).toBe(200)
  })

  it('carries the caller and their permissions through', () => {
    const g = decideAppGate(session('member', except('/x/crm'), ['boards.create']), APPS, 'crm')
    expect(g.ok && g.caller.user.id).toBe('u1')
    expect(g.ok && g.caller.perms).toEqual(['boards.create'])
  })
})

describe('canFor', () => {
  it('answers from the effective permissions, and yes for an admin', () => {
    const can = canFor({ user: user('member'), perms: ['kb.edit'] })
    expect(can('kb.edit')).toBe(true)
    expect(can('agents.manage')).toBe(false)
    expect(canFor({ user: user('admin'), perms: [] })('agents.manage')).toBe(true)
  })
})

describe('rustSession', () => {
  const prev = process.env.TALARIA_RUST_API_URL
  afterEach(() => {
    if (prev === undefined) delete process.env.TALARIA_RUST_API_URL
    else process.env.TALARIA_RUST_API_URL = prev
  })
  const answer = (status: number, body: unknown) => vi.fn(async () => new Response(JSON.stringify(body), { status }))
  const req = (cookie?: string) => new Request('http://app.test/api/apps/crm/x', { headers: cookie ? { cookie } : {} })

  it('asks the api with the caller\'s cookie and nothing else', async () => {
    process.env.TALARIA_RUST_API_URL = 'http://api.test:5999'
    const f = answer(200, { user: null, deniedViews: [], perms: [] })
    await rustSession(new Request('http://app.test/x', { headers: { cookie: 'talaria_session=abc', authorization: 'Bearer k' } }), f)
    const [url, init] = f.mock.calls[0] as unknown as [URL, RequestInit]
    expect(String(url)).toBe('http://api.test:5999/api/auth/session')
    const sent = new Headers(init.headers)
    expect(sent.get('cookie')).toBe('talaria_session=abc')
    expect(sent.get('authorization')).toBeNull()
  })

  it('hands app code the documented user shape, not the whole wire', async () => {
    const wire = { ...user('member'), statusEmoji: '📅', statusText: 'out' }
    const s = await rustSession(req('talaria_session=abc'), answer(200, { user: wire, deniedViews: ['/x/crm'], perms: ['kb.edit'] }))
    expect(s.user).toEqual(user('member'))
    expect(s.deniedViews).toEqual(['/x/crm'])
    expect(s.perms).toEqual(['kb.edit'])
  })

  it('reads `user: null` as signed out', async () => {
    const s = await rustSession(req(), answer(200, { user: null, deniedViews: [], perms: [] }))
    expect(s.user).toBeNull()
  })

  it('throws rather than guess when the api cannot answer', async () => {
    await expect(rustSession(req(), answer(500, { error: 'boom' }))).rejects.toThrow(/500/)
    await expect(rustSession(req(), answer(200, { deniedViews: [] }))).rejects.toThrow(/no "user"/)
    process.env.TALARIA_RUST_API_URL = 'off'
    await expect(rustSession(req(), answer(200, { user: null }))).rejects.toThrow(/stood down/)
  })
})
