import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'
import { installActiveKey, newDek, seal } from '../secretbox'

// The provisioning pass, held against scripted Google responses. The pass is
// one decision chain — preflight (connected? scope? domain?) → create-or-reuse
// the container → check-then-add the domain share → store what it made — and
// these tests pin each link, including the two operational walls an admin
// actually hits: the pre-reconnect connection and the consumer account.

const FULL_SCOPES = [
  'openid',
  'email',
  'https://www.googleapis.com/auth/calendar',
  'https://www.googleapis.com/auth/drive',
  'https://www.googleapis.com/auth/gmail.modify',
  'https://www.googleapis.com/auth/gmail.send',
].join(' ')

const OLD_SCOPES = 'openid email https://www.googleapis.com/auth/calendar.events https://www.googleapis.com/auth/drive.file'

const state = vi.hoisted(() => ({
  orgRow: null as Record<string, unknown> | null,
  updates: [] as string[], // the SET clauses provisioning wrote, in order
  calls: [] as Array<{ method: string; url: string; body?: unknown }>,
}))

// postgres.js-shaped tagged template answering by query text (see
// daily-brief.test.ts for the pattern): the org row for the status/token
// reads, and a recorder for the target writes. Self-contained in the factory
// — `state` is vi.hoisted, everything else it needs is inline.
vi.mock('@/server/db/pg', () => ({
  db: async () => {
    const sql = (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown[]> => {
      const text = strings.join(' ').replace(/\s+/g, ' ').trim()
      // Record text WITH bound values — assertions check ids landed in writes.
      state.updates.push(`${text} :: ${values.join(' | ')}`)
      if (text.includes('from google_org_connection')) return Promise.resolve(state.orgRow ? [state.orgRow] : [])
      return Promise.resolve([])
    }
    return sql as never
  },
}))

import { provisionOrgCalendar, provisionSharedDrive, provisioningReadiness } from './provisioning'

// Arm the secretbox with a test key so the seeded access token is sealable.
beforeAll(() => installActiveKey(newDek(), 1, 'provisioning-test-root'))

const page = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status })

/** Route a fetch to a scripted Google answer, recording every call. */
function stubGoogle(routes: Array<{ match: (url: string, method: string) => boolean; respond: () => Response }>) {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
      const url = String(input instanceof Request ? input.url : input)
      const method = (init?.method ?? 'GET').toUpperCase()
      state.calls.push({ method, url, body: init?.body ? JSON.parse(String(init.body)) : undefined })
      for (const r of routes) {
        if (r.match(url, method)) return r.respond()
      }
      return page({ error: { message: `unscripted ${method} ${url}` } }, 500)
    }),
  )
}

const connected = (over: Record<string, unknown> = {}) => ({
  google_sub: 'g1',
  email: 'jon@outcroplabs.com',
  scope: FULL_SCOPES,
  refresh_token_enc: 'sealed-refresh',
  access_token_enc: seal('tok'),
  access_expires_at: new Date(Date.now() + 3600_000).toISOString(),
  drive_folder_id: null,
  calendar_id: null,
  send_as: null,
  shared_drive_id: null,
  created_at: new Date().toISOString(),
  ...over,
})

beforeEach(() => {
  state.orgRow = connected()
  state.updates = []
  state.calls = []
})
afterEach(() => vi.unstubAllGlobals())

describe('provisionOrgCalendar', () => {
  it('creates the calendar, shares it with the domain as writer, stores the id', async () => {
    stubGoogle([
      { match: (u, m) => m === 'POST' && /\/calendar\/v3\/calendars$/.test(u), respond: () => page({ id: 'cal-new' }) },
      { match: (u) => u.includes('/calendars/cal-new/acl?'), respond: () => page({ items: [] }) },
      { match: (u, m) => m === 'POST' && u.includes('/calendars/cal-new/acl'), respond: () => page({ id: 'rule-1' }) },
    ])
    const res = await provisionOrgCalendar()
    expect(res).toEqual({ ok: true, state: 'created', id: 'cal-new' })
    // The share rule names the ORG DOMAIN (from the connected email), as writer.
    const acl = state.calls.find((c) => c.method === 'POST' && c.url.includes('/acl'))
    expect(acl?.body).toEqual({ scope: { type: 'domain', value: 'outcroplabs.com' }, role: 'writer' })
    // And the id landed in the org targets.
    expect(state.updates.some((q) => q.includes('set calendar_id') && q.includes('cal-new'))).toBe(true)
  })

  it('reuses a stored calendar that still exists, without re-creating or re-sharing', async () => {
    state.orgRow = connected({ calendar_id: 'cal-old' })
    stubGoogle([
      { match: (u) => u.includes('/calendars/cal-old?'), respond: () => page({ id: 'cal-old' }) },
      { match: (u) => u.includes('/calendars/cal-old/acl?'), respond: () => page({ items: [{ scope: { type: 'domain', value: 'outcroplabs.com' } }] }) },
    ])
    const res = await provisionOrgCalendar()
    expect(res).toEqual({ ok: true, state: 'reused', id: 'cal-old' })
    expect(state.calls.filter((c) => c.method === 'POST')).toHaveLength(0)
  })

  it('adds the missing share to an existing calendar (reused → shared)', async () => {
    state.orgRow = connected({ calendar_id: 'cal-old' })
    stubGoogle([
      { match: (u) => u.includes('/calendars/cal-old?'), respond: () => page({ id: 'cal-old' }) },
      { match: (u) => u.includes('/calendars/cal-old/acl?'), respond: () => page({ items: [] }) },
      { match: (u, m) => m === 'POST' && u.includes('/calendars/cal-old/acl'), respond: () => page({ id: 'rule-2' }) },
    ])
    expect(await provisionOrgCalendar()).toEqual({ ok: true, state: 'shared', id: 'cal-old' })
  })

  it('recreates a stored calendar Google no longer has', async () => {
    state.orgRow = connected({ calendar_id: 'cal-gone' })
    stubGoogle([
      { match: (u) => u.includes('/calendars/cal-gone?'), respond: () => page({ error: { message: 'not found' } }, 404) },
      { match: (u, m) => m === 'POST' && /\/calendar\/v3\/calendars$/.test(u), respond: () => page({ id: 'cal-new' }) },
      { match: (u) => u.includes('/calendars/cal-new/acl?'), respond: () => page({ items: [] }) },
      { match: (u, m) => m === 'POST' && u.includes('/calendars/cal-new/acl'), respond: () => page({ id: 'rule-3' }) },
    ])
    expect(await provisionOrgCalendar()).toEqual({ ok: true, state: 'created', id: 'cal-new' })
  })

  it('says reconnect, calling nothing, when the connection predates the wide scopes', async () => {
    state.orgRow = connected({ scope: OLD_SCOPES })
    stubGoogle([])
    const res = await provisionOrgCalendar()
    expect(res).toMatchObject({ ok: false, error: 'reconnect_needed' })
    expect(state.calls).toHaveLength(0)
  })

  it('says not_connected when there is no org connection', async () => {
    state.orgRow = null
    stubGoogle([])
    expect(await provisionOrgCalendar()).toMatchObject({ ok: false, error: 'not_connected' })
  })
})

describe('provisionSharedDrive', () => {
  const driveRoutes = (id: string): Array<{ match: (u: string, m: string) => boolean; respond: () => Response }> => [
    { match: (u, m) => m === 'POST' && /\/drive\/v3\/drives\?requestId=/.test(u), respond: () => page({ id }) },
    { match: (u) => u.includes(`/drives/${id}?`), respond: () => page({ id }) },
    { match: (u) => u.includes(`/drives/${id}/permissions?`), respond: () => page({ permissions: [] }) },
    { match: (u, m) => m === 'POST' && u.includes(`/drives/${id}/permissions`), respond: () => page({ id: 'perm-1' }) },
  ]

  it('creates the drive, grants the domain fileOrganizer, stores it AND repoints exports at it', async () => {
    stubGoogle(driveRoutes('drv-new'))
    const res = await provisionSharedDrive()
    expect(res).toEqual({ ok: true, state: 'created', id: 'drv-new' })
    const perm = state.calls.find((c) => c.method === 'POST' && c.url.includes('/permissions'))
    expect(perm?.body).toEqual({ role: 'fileOrganizer', type: 'domain', allowFileDiscovery: true })
    expect(state.updates.some((q) => q.includes('set shared_drive_id') && q.includes('drv-new'))).toBe(true)
    // The drive is the export target now — team files land team-owned.
    expect(state.updates.some((q) => q.includes('set drive_folder_id') && q.includes('drv-new'))).toBe(true)
  })

  it('reuses a stored drive that exists and is already shared', async () => {
    state.orgRow = connected({ shared_drive_id: 'drv-old' })
    stubGoogle([
      { match: (u) => u.includes('/drives/drv-old?'), respond: () => page({ id: 'drv-old' }) },
      { match: (u) => u.includes('/drives/drv-old/permissions?'), respond: () => page({ permissions: [{ type: 'domain' }] }) },
    ])
    expect(await provisionSharedDrive()).toEqual({ ok: true, state: 'reused', id: 'drv-old' })
    expect(state.calls.filter((c) => c.method === 'POST')).toHaveLength(0)
  })

  it('names the Workspace wall when a consumer account cannot create Shared Drives', async () => {
    stubGoogle([
      {
        match: (u, m) => m === 'POST' && /\/drive\/v3\/drives\?requestId=/.test(u),
        respond: () => page({ error: { message: 'Shared Drives cannot be created by this account' } }, 403),
      },
    ])
    const res = await provisionSharedDrive()
    expect(res).toMatchObject({ ok: false, error: 'consumer_account' })
    expect(res.ok === false && res.message).toMatch(/Workspace/i)
  })

  it('says reconnect, calling nothing, without the full drive scope', async () => {
    state.orgRow = connected({ scope: OLD_SCOPES })
    stubGoogle([])
    expect(await provisionSharedDrive()).toMatchObject({ ok: false, error: 'reconnect_needed' })
    expect(state.calls).toHaveLength(0)
  })
})

describe('provisioningReadiness', () => {
  it('reports per-container scope readiness for the panel', async () => {
    state.orgRow = connected({ scope: OLD_SCOPES })
    expect(await provisioningReadiness()).toEqual({
      connected: true,
      email: 'jon@outcroplabs.com',
      calendarScope: false,
      driveScope: false,
    })
    state.orgRow = connected()
    expect(await provisioningReadiness()).toMatchObject({ calendarScope: true, driveScope: true })
  })
})
