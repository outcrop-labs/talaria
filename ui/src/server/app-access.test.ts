// The app gate's decision and the session read it rests on. The gate admits a
// caller holding ANY of the app's surfaces — read off the session's listed
// `appGrants` — and tells the app which; the session comes from the Rust api
// with the caller's cookie, and a session the api cannot answer (or answers
// without its lists) is never read as "no denials" or "no grants": the gate
// answers 503.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const enabledApps = vi.fn(async (): Promise<AppManifest[]> => [])
vi.mock('./apps', () => ({ enabledApps }))

const { authorizeApp, canFor, decideAppGate } = await import('./app-access')
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
/** A session holding exactly `appGrants`. Its `deniedViews` is empty — what
 *  once admitted every member — because the gate must not read the denials
 *  for an app at all. */
const session = (role: 'admin' | 'member' | null, appGrants: string[] | null = [], perms: string[] = []): RustSession => ({
  user: role ? user(role) : null,
  deniedViews: [],
  appGrants,
  appsUnreadable: appGrants === null,
  perms,
})

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
    expect(status(decideAppGate(session('member'), APPS, 'crm'))).toBe(403)
    // A settings-only app is gated too — it used to be open to every member.
    expect(status(decideAppGate(session('member'), APPS, 'prefs'))).toBe(403)
    // Nothing to grant: members never reach it.
    expect(status(decideAppGate(session('member'), APPS, 'bare'))).toBe(403)
  })

  it('never admits on a denial list that leaves the app out', () => {
    // The fail-open this gate had: Rust could not read the enabled apps, so
    // their views were missing from deniedViews, and a missing denial read as
    // a grant. No listed grant, no entry — whatever the denials say.
    const s: RustSession = { user: user('member'), deniedViews: [], appGrants: [], appsUnreadable: false, perms: [] }
    expect(status(decideAppGate(s, APPS, 'crm'))).toBe(403)
    expect(status(decideAppGate(s, APPS, 'prefs'))).toBe(403)
  })

  it('answers 503 to a member whose grants the api could not read — never a grant', () => {
    // `appGrants: null`: Rust could not read the enabled apps. The session
    // still answered (the core views are whole), but no app opens on it.
    for (const slug of ['crm', 'prefs', 'bare']) expect(status(decideAppGate(session('member', null), APPS, slug)), slug).toBe(503)
    // Disclosure order holds: nobody is 401, a missing app is 404.
    expect(status(decideAppGate(session(null, null), APPS, 'crm'))).toBe(401)
    expect(status(decideAppGate(session('member', null), APPS, 'nope'))).toBe(404)
    // An admin holds every app by role, so the list does not matter.
    expect(status(decideAppGate(session('admin', null), APPS, 'crm'))).toBe(200)
  })

  it('admits a work-only member and says manage is not theirs', () => {
    const g = decideAppGate(session('member', ['/x/crm']), APPS, 'crm')
    expect(g.ok && g.caller.access).toEqual({ admin: false, work: true, manage: false, settings: false })
  })

  it('admits a manage-only member — the grant that used to do nothing', () => {
    const g = decideAppGate(session('member', ['/x/crm/manage']), APPS, 'crm')
    expect(g.ok && g.caller.access).toEqual({ admin: false, work: false, manage: true, settings: true })
  })

  it('admits a settings-only app to the member granted its manage view', () => {
    const g = decideAppGate(session('member', ['/x/prefs/manage']), APPS, 'prefs')
    expect(g.ok && g.caller.access).toEqual({ admin: false, work: false, manage: false, settings: true })
  })

  it('admits an admin to everything the app ships', () => {
    const g = decideAppGate(session('admin'), APPS, 'crm')
    expect(g.ok && g.caller.access).toEqual({ admin: true, work: true, manage: true, settings: true })
    expect(status(decideAppGate(session('admin'), APPS, 'bare'))).toBe(200)
  })

  it('carries the caller and their permissions through', () => {
    const g = decideAppGate(session('member', ['/x/crm'], ['boards.create']), APPS, 'crm')
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

describe('authorizeApp', () => {
  const prev = process.env.TALARIA_RUST_API_URL
  beforeEach(() => {
    process.env.TALARIA_RUST_API_URL = 'http://api.test:5999'
    enabledApps.mockResolvedValue(APPS)
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    enabledApps.mockReset()
    if (prev === undefined) delete process.env.TALARIA_RUST_API_URL
    else process.env.TALARIA_RUST_API_URL = prev
  })
  const api = (status: number, body: unknown) =>
    vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(body), { status })))
  const req = () => new Request('http://app.test/api/apps/crm/x', { headers: { cookie: 'talaria_session=abc' } })
  const member = { ...user('member') }

  it('admits through the real session read when the api lists a grant', async () => {
    api(200, { user: member, deniedViews: [], appGrants: ['/x/crm'], perms: [] })
    const g = await authorizeApp(req(), 'crm')
    expect(g.ok && g.caller.access).toEqual({ admin: false, work: true, manage: false, settings: false })
  })

  it('answers 503 — never opens — when the session read throws', async () => {
    // The api 500s: the Rust side could not read the enabled apps.
    api(500, { error: 'internal' })
    expect(status(await authorizeApp(req(), 'crm'))).toBe(503)
    // A 200 without the grant list is a broken answer, not an empty one.
    api(200, { user: member, deniedViews: [], perms: [] })
    expect(status(await authorizeApp(req(), 'crm'))).toBe(503)
    // The api cannot be reached at all.
    vi.stubGlobal('fetch', vi.fn(async () => Promise.reject(new TypeError('fetch failed'))))
    expect(status(await authorizeApp(req(), 'crm'))).toBe(503)
    // The api is stood down.
    process.env.TALARIA_RUST_API_URL = 'off'
    expect(status(await authorizeApp(req(), 'crm'))).toBe(503)
  })

  it('answers 503 when the api withholds the member\'s app grants', async () => {
    // The api's answer when it could not read the enabled apps: a whole
    // session for the core views, and no app grants at all.
    api(200, { user: member, deniedViews: ['/teams'], appGrants: null, appsUnreadable: true, perms: [] })
    expect(status(await authorizeApp(req(), 'crm'))).toBe(503)
    // Even beside a list: the flag withholds it.
    api(200, { user: member, deniedViews: [], appGrants: ['/x/crm'], appsUnreadable: true, perms: [] })
    expect(status(await authorizeApp(req(), 'crm'))).toBe(503)
  })

  it('answers 503 when the enabled apps cannot be read', async () => {
    api(200, { user: member, deniedViews: [], appGrants: ['/x/crm'], perms: [] })
    enabledApps.mockRejectedValueOnce(new Error('the apps_enabled setting is not a list of app slugs'))
    expect(status(await authorizeApp(req(), 'crm'))).toBe(503)
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
    const f = answer(200, { user: null, deniedViews: [], appGrants: [], perms: [] })
    await rustSession(new Request('http://app.test/x', { headers: { cookie: 'talaria_session=abc', authorization: 'Bearer k' } }), f)
    const [url, init] = f.mock.calls[0] as unknown as [URL, RequestInit]
    expect(String(url)).toBe('http://api.test:5999/api/auth/session')
    const sent = new Headers(init.headers)
    expect(sent.get('cookie')).toBe('talaria_session=abc')
    expect(sent.get('authorization')).toBeNull()
  })

  it('hands app code the documented user shape, not the whole wire', async () => {
    const wire = { ...user('member'), statusEmoji: '📅', statusText: 'out' }
    const s = await rustSession(
      req('talaria_session=abc'),
      answer(200, { user: wire, deniedViews: ['/boards'], appGrants: ['/x/crm/manage'], perms: ['kb.edit'] }),
    )
    expect(s.user).toEqual(user('member'))
    expect(s.deniedViews).toEqual(['/boards'])
    expect(s.appGrants).toEqual(['/x/crm/manage'])
    expect(s.appsUnreadable).toBe(false)
    expect(s.perms).toEqual(['kb.edit'])
  })

  it('reads `appsUnreadable` as no app grants, and keeps the core lists', async () => {
    const s = await rustSession(
      req('talaria_session=abc'),
      answer(200, { user: user('member'), deniedViews: ['/teams'], appGrants: null, appsUnreadable: true, perms: ['kb.edit'] }),
    )
    expect(s.appsUnreadable).toBe(true)
    expect(s.appGrants).toBeNull()
    expect(s.deniedViews).toEqual(['/teams'])
    expect(s.perms).toEqual(['kb.edit'])
    // The flag withholds a list that came with it, too.
    const t = await rustSession(req(), answer(200, { user: user('member'), deniedViews: [], appGrants: ['/x/crm'], appsUnreadable: true, perms: [] }))
    expect(t.appGrants).toBeNull()
  })

  it('throws on a null grant list the flag does not explain, or a flag that is not a boolean', async () => {
    const full = { user: user('member'), deniedViews: [], appGrants: [], perms: [] }
    await expect(rustSession(req(), answer(200, { ...full, appGrants: null }))).rejects.toThrow(/no "appGrants" list/)
    await expect(rustSession(req(), answer(200, { ...full, appGrants: null, appsUnreadable: false }))).rejects.toThrow(/no "appGrants" list/)
    await expect(rustSession(req(), answer(200, { ...full, appsUnreadable: 'yes' }))).rejects.toThrow(/appsUnreadable/)
  })

  it('reads `user: null` as signed out', async () => {
    const s = await rustSession(req(), answer(200, { user: null, deniedViews: [], appGrants: [], perms: [] }))
    expect(s.user).toBeNull()
  })

  it('throws rather than guess when the api cannot answer', async () => {
    await expect(rustSession(req(), answer(500, { error: 'boom' }))).rejects.toThrow(/500/)
    await expect(rustSession(req(), answer(200, { deniedViews: [] }))).rejects.toThrow(/no "user"/)
    await expect(rustSession(req(), answer(200, null))).rejects.toThrow(/no object/)
    process.env.TALARIA_RUST_API_URL = 'off'
    await expect(rustSession(req(), answer(200, { user: null }))).rejects.toThrow(/stood down/)
  })

  it('throws when a list is missing or is not a list of strings — never reads it as empty', async () => {
    const full = { user: user('member'), deniedViews: [], appGrants: [], perms: [] }
    for (const key of ['deniedViews', 'appGrants', 'perms'] as const) {
      const { [key]: _gone, ...without } = full
      await expect(rustSession(req(), answer(200, without)), `${key} missing`).rejects.toThrow(new RegExp(`no "${key}" list`))
      await expect(rustSession(req(), answer(200, { ...full, [key]: 'x' })), `${key} a string`).rejects.toThrow(new RegExp(`no "${key}" list`))
      await expect(rustSession(req(), answer(200, { ...full, [key]: ['/x', 7] })), `${key} with a number`).rejects.toThrow(
        new RegExp(`no "${key}" list`),
      )
    }
  })
})
