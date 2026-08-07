import { beforeEach, describe, expect, it, vi } from 'vitest'

// Audit 1.6. Two stubs, and only two: the settings row (shared with
// `harness/capability.ts`, which resolves `../audit` to the same module, so
// these cases run the REAL capability reader over a real stored fact) and the
// gateway's routing table. Nothing here touches a database or a model.
const { store } = vi.hoisted(() => ({ store: new Map<string, unknown>() }))

vi.mock('./audit', () => ({
  getSetting: <T>(key: string, fallback: T): Promise<T> => Promise.resolve(store.has(key) ? (store.get(key) as T) : fallback),
  setSetting: (key: string, value: unknown): Promise<void> => {
    store.set(key, structuredClone(value))
    return Promise.resolve()
  },
}))

// Which endpoints serve which model id. `routingFor` is the one the fitness
// path uses (it must not advance the round-robin cursor); `resolveRoute` is
// here only because the module imports it.
const { routes } = vi.hoisted(() => ({ routes: new Map<string, string[]>() }))

vi.mock('./llm-gateway', () => ({
  routingFor: (model: string): Promise<{ endpoints: Array<{ name: string }>; upstreamModel: string }> =>
    Promise.resolve({ endpoints: (routes.get(model) ?? []).map((name) => ({ name })), upstreamModel: model }),
  resolveRoute: (model: string): Promise<{ endpoint: { name: string }; upstreamModel: string } | null> => {
    const eps = routes.get(model) ?? []
    return Promise.resolve(eps[0] ? { endpoint: { name: eps[0] }, upstreamModel: model } : null)
  },
}))

import { recordCapability, capabilityKey, type Capability, type CapabilityFact } from './harness/capability'
import { MODEL_ROLES, roleAssignmentIssues, roleModelGaps, setModelRole, type ModelRole } from './model-roles'

const fact = (value: boolean): CapabilityFact => ({ value, source: 'probe', at: new Date().toISOString(), score: value ? 1 : 0 })

const serves = (model: string, endpoints: string[]): void => {
  routes.set(model, endpoints)
}

const knows = (endpoint: string, model: string, cap: Capability, value: boolean): Promise<void> =>
  recordCapability(capabilityKey(endpoint, model), cap, fact(value))

beforeEach(() => {
  store.clear()
  routes.clear()
})

// ── The declarations themselves ──────────────────────────────────────────────

describe('MODEL_ROLES capability declarations', () => {
  it('requires search on every research role', () => {
    for (const role of ['research-recon', 'research-brief', 'research-expedition'] as ModelRole[]) {
      expect(MODEL_ROLES.find((r) => r.role === role)?.requires).toEqual(['search'])
    }
  })

  it('requires tool calling on every code role', () => {
    // Not a quality bar: without `tools` a coding harness edits no files at all.
    for (const role of ['code-light', 'code-standard', 'code-heavy'] as ModelRole[]) {
      expect(MODEL_ROLES.find((r) => r.role === role)?.requires).toContain('tools')
    }
  })

  it('leaves utility requiring nothing, because it is the last link in every chain', () => {
    expect(MODEL_ROLES.find((r) => r.role === 'utility')?.requires).toEqual([])
  })

  it('declares a requires array for every role, including reserved slots', () => {
    for (const r of MODEL_ROLES) expect(Array.isArray(r.requires)).toBe(true)
  })
})

// ── roleModelGaps ────────────────────────────────────────────────────────────

describe('roleModelGaps', () => {
  it('returns [] for an unprobed model — UNKNOWN IS NOT A LACK', async () => {
    // The state every fresh self-host is in. Warning here would train admins to
    // ignore the warning that matters.
    serves('qwen3-14b', ['pl-main'])
    expect(await roleModelGaps('research-recon', 'qwen3-14b')).toEqual([])
  })

  it('returns the missing capability for a probed-and-lacking model', async () => {
    serves('qwen3-14b', ['pl-main'])
    await knows('pl-main', 'qwen3-14b', 'search', false)
    expect(await roleModelGaps('research-recon', 'qwen3-14b')).toEqual(['search'])
  })

  it('returns [] for a probed-and-capable model', async () => {
    serves('sonar-pro', ['perplexity'])
    await knows('perplexity', 'sonar-pro', 'search', true)
    expect(await roleModelGaps('research-brief', 'sonar-pro')).toEqual([])
  })

  it('never reports an issue for a role that requires nothing', async () => {
    // Utility must run on anything, so even a model probed as lacking
    // everything is a legitimate assignment here.
    serves('tiny', ['pl-main'])
    for (const cap of ['search', 'tools', 'code', 'vision'] as Capability[]) await knows('pl-main', 'tiny', cap, false)
    expect(await roleModelGaps('utility', 'tiny')).toEqual([])
    expect(await roleModelGaps('embedding', 'tiny')).toEqual([])
  })

  it('reports every missing capability a role asked for', async () => {
    serves('prose-only', ['pl-main'])
    await knows('pl-main', 'prose-only', 'code', false)
    await knows('pl-main', 'prose-only', 'tools', false)
    expect((await roleModelGaps('code-standard', 'prose-only')).sort()).toEqual(['code', 'tools'])
  })

  it('is unanimous over a routing pool: one capable member clears the gap', async () => {
    // Capability belongs to the ENDPOINT, and a bare model name round-robins.
    // We cannot know which member takes a later call, so a split pool is not
    // evidence of a lack — same rule runHarness applies.
    serves('qwen3-14b', ['pl-main', 'pl-spare'])
    await knows('pl-main', 'qwen3-14b', 'search', false)
    await knows('pl-spare', 'qwen3-14b', 'search', true)
    expect(await roleModelGaps('research-recon', 'qwen3-14b')).toEqual([])
  })

  it('reports the gap when every member of the pool lacks it', async () => {
    serves('qwen3-14b', ['pl-main', 'pl-spare'])
    await knows('pl-main', 'qwen3-14b', 'search', false)
    await knows('pl-spare', 'qwen3-14b', 'search', false)
    expect(await roleModelGaps('research-recon', 'qwen3-14b')).toEqual(['search'])
  })

  it('credits no facts across endpoints', async () => {
    // A fact learned about one endpoint's quantized build says nothing about
    // the vendor API serving the same model id.
    serves('qwen3-14b', ['pl-main'])
    await knows('openrouter', 'qwen3-14b', 'search', false)
    expect(await roleModelGaps('research-recon', 'qwen3-14b')).toEqual([])
  })

  it('returns [] for a model nothing routes', async () => {
    // resolveRoleModel already declines it; a second warning about a dead
    // assignment is noise.
    await knows('pl-main', 'ghost', 'search', false)
    expect(await roleModelGaps('research-recon', 'ghost')).toEqual([])
  })
})

// ── roleAssignmentIssues ─────────────────────────────────────────────────────

describe('roleAssignmentIssues', () => {
  it('is empty when nothing is assigned', async () => {
    expect(await roleAssignmentIssues()).toEqual([])
  })

  it('is empty when the assignment is unprobed', async () => {
    serves('qwen3-14b', ['pl-main'])
    await setModelRole('research-recon', 'qwen3-14b')
    expect(await roleAssignmentIssues()).toEqual([])
  })

  it('names the role, the model and the missing capability in plain words', async () => {
    serves('qwen3-14b', ['pl-main'])
    await knows('pl-main', 'qwen3-14b', 'search', false)
    await setModelRole('research-recon', 'qwen3-14b')

    const issues = await roleAssignmentIssues()
    expect(issues).toHaveLength(1)
    expect(issues[0]?.role).toBe('research-recon')
    expect(issues[0]?.model).toBe('qwen3-14b')
    expect(issues[0]?.missing).toEqual(['search'])
    expect(issues[0]?.note).toContain('qwen3-14b')
    expect(issues[0]?.note).toContain('no web search')
    // It says the assignment stands. This is a sentence, not a rejection.
    expect(issues[0]?.note).toMatch(/stands/)
  })

  it('reports one issue per unfit role and stays silent about fit ones', async () => {
    serves('qwen3-14b', ['pl-main'])
    serves('sonar-pro', ['perplexity'])
    await knows('pl-main', 'qwen3-14b', 'search', false)
    await knows('pl-main', 'qwen3-14b', 'tools', false)
    await knows('perplexity', 'sonar-pro', 'search', true)
    await setModelRole('research-recon', 'qwen3-14b')
    await setModelRole('research-brief', 'sonar-pro')
    await setModelRole('code-heavy', 'qwen3-14b')
    await setModelRole('utility', 'qwen3-14b')

    const issues = await roleAssignmentIssues()
    expect(issues.map((i) => i.role).sort()).toEqual(['code-heavy', 'research-recon'])
  })

  it('clears once the assignment goes back to auto', async () => {
    serves('qwen3-14b', ['pl-main'])
    await knows('pl-main', 'qwen3-14b', 'search', false)
    await setModelRole('research-recon', 'qwen3-14b')
    expect(await roleAssignmentIssues()).toHaveLength(1)
    await setModelRole('research-recon', null)
    expect(await roleAssignmentIssues()).toEqual([])
  })
})
