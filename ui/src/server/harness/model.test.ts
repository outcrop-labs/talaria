import { describe, expect, it } from 'vitest'
import { modelAllowedFor } from '@/server/model-access'
import { resolveHarnessModelWith, type HarnessModelDeps, type ModelChainStep, type ModelSpec } from '@/server/harness/model'
import type { GatewayModel } from '@/server/llm-gateway'
import type { ModelRole } from '@/server/model-roles'
import type { PlatformAgentId } from '@/server/platform-agents'
import type { Role } from '@/server/users'

// The chain is defined over an injected HarnessModelDeps rather than mocked
// modules, because the ORDERING is the thing under test and it is pure — the
// real dependencies are five settings reads and a user row, and mocking them
// would test the mocks. `modelAllowedFor` is the exception: it is already pure,
// so the real one runs here and the allowlist cases exercise org policy for
// real rather than a re-statement of it.
//
// One rule holds the fake together: a model ROUTES iff the gateway catalog
// serves it. That is exactly the real invariant, so a stale pin/role/preference
// in a case below is a model simply absent from `catalog`.

interface World {
  catalog?: GatewayModel[]
  pins?: Partial<Record<PlatformAgentId, string>>
  roles?: Partial<Record<ModelRole, string>>
  env?: string | null
  preferred?: string | null
  userRole?: Role
  allow?: string[]
}

const bare = (...ids: string[]): GatewayModel[] => ids.map((id) => ({ id, endpoints: ['spark'], qualified: false }))
const pinnedTo = (...ids: string[]): GatewayModel[] =>
  ids.map((id) => ({ id, endpoints: [id.slice(0, id.indexOf('/'))], qualified: true }))

const deps = (w: World): HarnessModelDeps => {
  const catalog = [...(w.catalog ?? [])].sort((a, b) => a.id.localeCompare(b.id)) // gatewayModels() sorts; "first" must mean the same thing
  const routes = (m: string) => catalog.some((c) => c.id === m)
  return {
    // Mirrors the real contract of both resolvers: an assignment only survives
    // while it still routes (platform-agents.ts, model-roles.ts).
    platformAgentModel: async (id) => {
      const m = w.pins?.[id]
      return m && routes(m) ? m : null
    },
    resolveRoleModel: async (role) => {
      const m = w.roles?.[role]
      return m && routes(m) ? m : null
    },
    routes: async (m) => routes(m),
    gatewayModels: async () => catalog,
    getPreferredModel: async () => w.preferred ?? null,
    getUserRole: async () => w.userRole ?? 'member',
    memberModelAllowlist: async () => w.allow ?? [],
    modelAllowedFor,
    copilotEnvModel: () => w.env ?? null,
  }
}

const resolve = (spec: ModelSpec, w: World) => resolveHarnessModelWith(spec, deps(w))

// A world where every step has an answer, so a case can prove which one won by
// removing exactly one thing.
const FULL: World = {
  catalog: bare('pl-main', 'assigned-role', 'utility-model', 'env-model', 'pinned-model', 'preferred-model', 'aardvark'),
  pins: { titler: 'pinned-model' },
  roles: { 'code-light': 'assigned-role', utility: 'utility-model' },
  env: 'env-model',
  preferred: 'preferred-model',
}

describe('the chain, step by step', () => {
  it('takes the admin pin first', async () => {
    expect(await resolve({ pin: 'titler', role: 'code-light' }, FULL)).toEqual({ model: 'pinned-model', step: 'pin' })
  })

  it('falls to the harness role when nothing is pinned', async () => {
    expect(await resolve({ pin: 'titler', role: 'code-light' }, { ...FULL, pins: {} })).toEqual({
      model: 'assigned-role',
      step: 'role',
    })
  })

  it('falls to the utility role when the harness role is unassigned', async () => {
    expect(await resolve({ pin: 'titler', role: 'code-light' }, { ...FULL, pins: {}, roles: { utility: 'utility-model' } })).toEqual({
      model: 'utility-model',
      step: 'utility',
    })
  })

  it('falls to the env default when no role is assigned', async () => {
    expect(await resolve({ pin: 'titler', role: 'code-light' }, { ...FULL, pins: {}, roles: {} })).toEqual({
      model: 'env-model',
      step: 'env',
    })
  })

  it('falls to the last-resort catalog scan when the env default is unset', async () => {
    expect(await resolve({ pin: 'titler', role: 'code-light' }, { ...FULL, pins: {}, roles: {}, env: null })).toEqual({
      model: 'pl-main',
      step: 'first-routable',
    })
  })

  it('reports first-routable, not a fabricated step, when the last resort carries the harness', async () => {
    const got = await resolve({}, { catalog: bare('only-model') })
    expect(got).toEqual({ model: 'only-model', step: 'first-routable' })
  })
})

describe('the last-resort step (AUDIT 1.7 — no bare pl-main)', () => {
  it('prefers pl-main when the gateway happens to serve it', async () => {
    expect(await resolve({}, { catalog: bare('aardvark', 'pl-main', 'zebra') })).toEqual({
      model: 'pl-main',
      step: 'first-routable',
    })
  })

  it('still resolves on an install that never named an endpoint pl-main', async () => {
    // The bug this whole module exists to kill: seven call sites stopped at the
    // literal 'pl-main' and no-opped silently on every self-host without one.
    expect(await resolve({}, { catalog: bare('qwen3-14b', 'zebra') })).toEqual({
      model: 'qwen3-14b',
      step: 'first-routable',
    })
  })

  it('never lands on an endpoint-qualified id — that would strand the harness on one backend', async () => {
    expect(await resolve({}, { catalog: pinnedTo('spark/pl-main', 'spark/qwen3-14b') })).toBeNull()
  })
})

describe('staleness — a deleted model never silently breaks a subsystem', () => {
  it('falls through a pinned model that no longer routes', async () => {
    // The admin assignment survives in app_settings after the endpoint serving
    // it is deleted. The harness must keep working on the next step down.
    const got = await resolve({ pin: 'titler', role: 'code-light' }, { ...FULL, pins: { titler: 'retired-model' } })
    expect(got).toEqual({ model: 'assigned-role', step: 'role' })
  })

  it('falls through an env default that no longer routes', async () => {
    const got = await resolve({}, { ...FULL, pins: {}, roles: {}, env: 'TALARIA_COPILOT_MODEL-typo' })
    expect(got).toEqual({ model: 'pl-main', step: 'first-routable' })
  })

  it('falls through a preferred model that no longer routes', async () => {
    const got = await resolve(
      { chain: ['preferred', 'utility'], userId: 'u1' },
      { ...FULL, preferred: 'model-the-admin-deleted' },
    )
    expect(got).toEqual({ model: 'utility-model', step: 'utility' })
  })
})

describe('the member model allowlist is org policy, not a suggestion', () => {
  const CHAIN: ModelChainStep[] = ['pin', 'preferred', 'utility', 'env', 'first-routable']

  it('lets a member use their preferred model when it is allowed', async () => {
    const got = await resolve(
      { chain: CHAIN, userId: 'u1' },
      { ...FULL, pins: {}, allow: ['preferred-model', 'utility-model'] },
    )
    expect(got).toEqual({ model: 'preferred-model', step: 'preferred' })
  })

  it('rejects a preferred model outside the allowlist and keeps going', async () => {
    // An admin gating the expensive brains must not be routed around by this
    // refactor: the member drops to the next step, they do not get their pick.
    const got = await resolve({ chain: CHAIN, userId: 'u1' }, { ...FULL, pins: {}, allow: ['utility-model'] })
    expect(got).toEqual({ model: 'utility-model', step: 'utility' })
  })

  it('gates the role/utility steps and the last-resort scan too', async () => {
    const got = await resolve(
      { chain: CHAIN, userId: 'u1' },
      { ...FULL, pins: {}, env: null, allow: ['aardvark'] },
    )
    expect(got).toEqual({ model: 'aardvark', step: 'first-routable' })
  })

  it('does not restrict admins', async () => {
    const got = await resolve({ chain: CHAIN, userId: 'u1' }, { ...FULL, pins: {}, userRole: 'admin', allow: ['aardvark'] })
    expect(got).toEqual({ model: 'preferred-model', step: 'preferred' })
  })

  it('does not apply to an admin pin — org policy outranks org policy it set itself', async () => {
    const got = await resolve({ chain: CHAIN, pin: 'muse', userId: 'u1' }, { ...FULL, pins: { muse: 'pinned-model' }, allow: ['aardvark'] })
    expect(got).toEqual({ model: 'pinned-model', step: 'pin' })
  })

  it('has nothing to gate on an org-scoped harness (no userId)', async () => {
    // titler/librarian/blurb-writer run for the org, not for a person: an
    // allowlist that applied here would silently downgrade platform chores.
    const got = await resolve({ chain: CHAIN }, { ...FULL, pins: {}, allow: ['aardvark'] })
    expect(got).toEqual({ model: 'utility-model', step: 'utility' })
  })
})

describe('the declared chain is the whole chain', () => {
  it('never tries a step the spec did not name', async () => {
    expect(await resolve({ chain: ['pin'], pin: 'titler' }, { ...FULL, pins: {} })).toBeNull()
  })

  it('skips preferred when the harness is not user-scoped', async () => {
    const got = await resolve({ chain: ['preferred', 'utility'] }, FULL)
    expect(got).toEqual({ model: 'utility-model', step: 'utility' })
  })

  it('skips pin and role when the spec declares neither', async () => {
    expect(await resolve({ chain: ['pin', 'role', 'utility'] }, FULL)).toEqual({ model: 'utility-model', step: 'utility' })
  })
})

describe('nothing to run on', () => {
  it('returns null when the gateway serves nothing', async () => {
    expect(await resolve({ pin: 'titler', role: 'code-light', userId: 'u1' }, { catalog: [], env: 'env-model', preferred: 'p' })).toBeNull()
  })
})
