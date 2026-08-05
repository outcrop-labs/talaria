import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Perm } from '@/server/permissions'

// Three collaborators are stubbed, and each is stubbed for a reason:
//   • ./db/pg     — the per-user override read is one `select` whose result we
//                   need to vary per case; a live Postgres would add a service
//                   dependency to a test about `??` precedence.
//   • ./audit     — getSetting/setSetting are app_settings row access, same story.
//   • ./auth/session — getSessionUser needs Redis + a signed cookie.
// What is NOT stubbed is the thing under test: the precedence chain itself, the
// catalog defaults, and the 401/403 shapes requirePerm returns.
const state = {
  org: {} as Partial<Record<Perm, boolean>>,
  overrides: {} as Partial<Record<Perm, boolean>>,
  sessionUser: null as { id: string; role: 'admin' | 'member' } | null,
}

const getSetting = vi.fn(async (_key: string, fallback: unknown) => structuredClone(state.org) ?? fallback)
const setSetting = vi.fn(async (_key: string, value: unknown) => {
  state.org = value as Partial<Record<Perm, boolean>>
})
const sqlSpy = vi.fn()
const getSessionUser = vi.fn(async () => state.sessionUser)

vi.mock('@/server/audit', () => ({ getSetting, setSetting }))
vi.mock('@/server/auth/session', () => ({ getSessionUser }))
vi.mock('@/server/db/pg', () => ({
  db: async () => (strings: TemplateStringsArray, ...values: unknown[]) => {
    sqlSpy(strings.join('?'), values)
    const text = strings.join(' ')
    if (/from\s+user_permissions/i.test(text)) {
      return Object.entries(state.overrides).map(([perm, allowed]) => ({ perm, allowed }))
    }
    return []
  },
}))

const { PERMISSIONS, getOrgDefaultPerms, hasPerm, requirePerm, setOrgDefaultPerm, userPermissions } = await import(
  '@/server/permissions'
)

const member = { id: 'u1', role: 'member' as const }
const admin = { id: 'u2', role: 'admin' as const }

// Two fixed reference points from the shipped catalog, so the precedence tests
// exercise both directions of the default.
const ON_BY_DEFAULT: Perm = 'research.run'
const OFF_BY_DEFAULT: Perm = 'kb.official'

beforeEach(() => {
  state.org = {}
  state.overrides = {}
  state.sessionUser = null
  vi.clearAllMocks()
})

describe('catalog', () => {
  it('ships the expected defaults for the two reference permissions', () => {
    const byId = new Map(PERMISSIONS.map((p) => [p.id, p]))
    expect(byId.get(ON_BY_DEFAULT)?.memberDefault).toBe(true)
    expect(byId.get(OFF_BY_DEFAULT)?.memberDefault).toBe(false)
  })

  it('has no duplicate ids', () => {
    expect(new Set(PERMISSIONS.map((p) => p.id)).size).toBe(PERMISSIONS.length)
  })

  it('keeps the dangerous permissions off by default', () => {
    const off = PERMISSIONS.filter((p) => !p.memberDefault).map((p) => p.id)
    // Publishing to the open web, curating OFFICIAL knowledge, minting gateway
    // keys and managing agents are opt-in. A change here is a policy change.
    expect(off).toEqual(
      expect.arrayContaining(['agents.manage', 'kb.official', 'artifacts.publish', 'templates.manage', 'models.mint-keys']),
    )
  })
})

describe('hasPerm — precedence: user override ?? org default ?? catalog ?? false', () => {
  it('uses the catalog default when nothing else is set', async () => {
    expect(await hasPerm(member, ON_BY_DEFAULT)).toBe(true)
    expect(await hasPerm(member, OFF_BY_DEFAULT)).toBe(false)
  })

  it('lets an org default override the catalog, in both directions', async () => {
    state.org = { [ON_BY_DEFAULT]: false, [OFF_BY_DEFAULT]: true }
    expect(await hasPerm(member, ON_BY_DEFAULT)).toBe(false)
    expect(await hasPerm(member, OFF_BY_DEFAULT)).toBe(true)
  })

  it('lets a user override beat the org default, in both directions', async () => {
    state.org = { [ON_BY_DEFAULT]: false, [OFF_BY_DEFAULT]: true }
    state.overrides = { [ON_BY_DEFAULT]: true, [OFF_BY_DEFAULT]: false }
    expect(await hasPerm(member, ON_BY_DEFAULT)).toBe(true)
    expect(await hasPerm(member, OFF_BY_DEFAULT)).toBe(false)
  })

  it('honours an explicit DENY — the escalation this chain exists to prevent', async () => {
    // `??` and not `||`: a stored `false` is a decision, not an absent value. If
    // this ever became `||`, every explicit deny would silently fall through to
    // the (permissive) catalog default.
    state.overrides = { [ON_BY_DEFAULT]: false }
    expect(await hasPerm(member, ON_BY_DEFAULT)).toBe(false)

    state.overrides = {}
    state.org = { [ON_BY_DEFAULT]: false }
    expect(await hasPerm(member, ON_BY_DEFAULT)).toBe(false)
  })

  it('an org default of false is not rescued by an absent user override', async () => {
    state.org = { [OFF_BY_DEFAULT]: false }
    state.overrides = { [ON_BY_DEFAULT]: true } // a different perm entirely
    expect(await hasPerm(member, OFF_BY_DEFAULT)).toBe(false)
  })

  it('denies an unknown permission id rather than defaulting open', async () => {
    expect(await hasPerm(member, 'totally.made.up' as Perm)).toBe(false)
  })

  it('still honours an override/org grant for an id absent from the catalog', async () => {
    state.overrides = { ['totally.made.up' as Perm]: true }
    expect(await hasPerm(member, 'totally.made.up' as Perm)).toBe(true)
  })
})

describe('hasPerm — admin short-circuit', () => {
  it('grants every catalog permission to an admin', async () => {
    for (const p of PERMISSIONS) expect(await hasPerm(admin, p.id)).toBe(true)
  })

  it('grants an admin a permission both the org and their own override deny', async () => {
    state.org = { [ON_BY_DEFAULT]: false }
    state.overrides = { [ON_BY_DEFAULT]: false }
    expect(await hasPerm(admin, ON_BY_DEFAULT)).toBe(true)
  })

  it('short-circuits before touching the DB or settings at all', async () => {
    await hasPerm(admin, OFF_BY_DEFAULT)
    expect(getSetting).not.toHaveBeenCalled()
    expect(sqlSpy).not.toHaveBeenCalled()
  })
})

describe('userPermissions', () => {
  it('returns the whole catalog for an admin', async () => {
    expect(await userPermissions('u2', 'admin')).toEqual(PERMISSIONS.map((p) => p.id))
  })

  it('returns exactly the members-default set when nothing is customised', async () => {
    expect(await userPermissions('u1', 'member')).toEqual(PERMISSIONS.filter((p) => p.memberDefault).map((p) => p.id))
  })

  it('agrees with hasPerm for every catalog permission under a mixed policy', async () => {
    state.org = { [ON_BY_DEFAULT]: false, [OFF_BY_DEFAULT]: true, 'files.upload': false }
    state.overrides = { [ON_BY_DEFAULT]: true, 'kb.edit': false }
    const list = new Set(await userPermissions('u1', 'member'))
    for (const p of PERMISSIONS) {
      expect(list.has(p.id)).toBe(await hasPerm(member, p.id))
    }
  })
})

describe('setOrgDefaultPerm', () => {
  it('sets a value and clears it back to "inherit the catalog" with null', async () => {
    await setOrgDefaultPerm(ON_BY_DEFAULT, false)
    expect(await getOrgDefaultPerms()).toEqual({ [ON_BY_DEFAULT]: false })
    expect(await hasPerm(member, ON_BY_DEFAULT)).toBe(false)

    await setOrgDefaultPerm(ON_BY_DEFAULT, null)
    expect(await getOrgDefaultPerms()).toEqual({})
    expect(await hasPerm(member, ON_BY_DEFAULT)).toBe(true) // back to the catalog
  })

  it('leaves other keys alone', async () => {
    await setOrgDefaultPerm(ON_BY_DEFAULT, false)
    await setOrgDefaultPerm(OFF_BY_DEFAULT, true)
    expect(await getOrgDefaultPerms()).toEqual({ [ON_BY_DEFAULT]: false, [OFF_BY_DEFAULT]: true })
  })
})

describe('requirePerm', () => {
  const req = new Request('https://talaria.test/api/research')

  it('401s with no session', async () => {
    state.sessionUser = null
    const gate = await requirePerm(req, ON_BY_DEFAULT)
    expect(gate).toBeInstanceOf(Response)
    expect((gate as Response).status).toBe(401)
    await expect((gate as Response).json()).resolves.toEqual({ error: 'unauthorized' })
  })

  it('403s, naming the permission, when the session lacks it', async () => {
    state.sessionUser = member
    const gate = await requirePerm(req, OFF_BY_DEFAULT)
    expect(gate).toBeInstanceOf(Response)
    expect((gate as Response).status).toBe(403)
    await expect((gate as Response).json()).resolves.toEqual({ error: `you don't have permission to do that (${OFF_BY_DEFAULT})` })
  })

  it('returns the user when the permission is held', async () => {
    state.sessionUser = member
    expect(await requirePerm(req, ON_BY_DEFAULT)).toBe(member)
  })

  it('returns an admin regardless of the permission', async () => {
    state.sessionUser = admin
    expect(await requirePerm(req, OFF_BY_DEFAULT)).toBe(admin)
  })

  it('403s when a user override denies a perm the catalog grants', async () => {
    state.sessionUser = member
    state.overrides = { [ON_BY_DEFAULT]: false }
    expect((await requirePerm(req, ON_BY_DEFAULT)) as Response).toMatchObject({ status: 403 })
  })
})
