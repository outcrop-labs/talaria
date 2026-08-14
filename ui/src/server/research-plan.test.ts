// WHICH MODEL SEARCHES, AND HOW — the resolution that stopped requiring
// Perplexity.
//
// WHAT THIS REPLACED. Research used to hold a hardcoded list of sonar spellings
// and refuse to start when none was registered: "register a Perplexity sonar
// model on /models first". An org running its own models, with its own SearXNG
// that can search perfectly well for anything that calls a tool, could not
// research at all. It was also a hardcoded model list, which this codebase has a
// standing rule against for the ordinary reason — they rot the week a vendor
// renames something.
//
// The three steps below are three different questions, and the tests are
// arranged as those questions.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Reach } from '@/server/capability-reach'

const resolveRoleModel = vi.fn(async (_role: string): Promise<string | null> => null)
const capabilityKeysFor = vi.fn(async (_m: string): Promise<string[]> => ['k'])
const reachFor = vi.fn(async (_k: readonly string[], _w: readonly string[]): Promise<Record<string, Reach>> => ({}))

vi.mock('@/server/model-roles', () => ({ resolveRoleModel: (r: string) => resolveRoleModel(r) }))
vi.mock('@/server/harness/run', () => ({ capabilityKeysFor: (m: string) => capabilityKeysFor(m), runHarness: async () => ({ value: null }) }))
vi.mock('@/server/capability-reach', () => ({ reachFor: (k: readonly string[], w: readonly string[]) => reachFor(k, w) }))

const { planSearch, NO_SEARCH_REASON } = await import('@/server/research')

/** `reachFor` keyed by the model whose keys were asked for — the stub records
 *  the model in its key so one map can answer for several. */
const reachBy = (byModel: Record<string, Reach | undefined>) => {
  capabilityKeysFor.mockImplementation(async (m: string) => [m])
  reachFor.mockImplementation(async (keys: readonly string[]): Promise<Record<string, Reach>> => {
    const r = byModel[String(keys[0])]
    return r ? { search: r } : {}
  })
}

const models = async () => [{ id: 'llama-70b' }, { id: 'sonar-pro' }]
const TOOL: Reach = { via: 'tool', supplier: { server: 'talaria', tool: 'web_search' } } as Reach
const NATIVE: Reach = { via: 'native' } as Reach

beforeEach(() => {
  resolveRoleModel.mockReset().mockResolvedValue(null)
  capabilityKeysFor.mockReset().mockResolvedValue(['k'])
  reachFor.mockReset().mockResolvedValue({})
})

describe('1. the admin’s own assignment wins', () => {
  it('uses the assigned model, and says which path it will take', async () => {
    resolveRoleModel.mockResolvedValue('sonar-pro')
    reachBy({ 'sonar-pro': NATIVE })
    expect(await planSearch('brief')).toEqual({ model: 'sonar-pro', via: 'native', supplier: null })
  })

  it('honours an assignment that needs OUR search rather than second-guessing it', async () => {
    // An admin assigning a model with no native search has not made a mistake —
    // they have chosen the model, and the tool path is how it searches.
    resolveRoleModel.mockResolvedValue('llama-70b')
    reachBy({ 'llama-70b': TOOL })
    expect(await planSearch('brief')).toEqual({ model: 'llama-70b', via: 'tool', supplier: { server: 'talaria', tool: 'web_search' } })
  })

  it('proceeds on an assignment whose capability is UNKNOWN — unknown is not missing', async () => {
    // The cardinal rule. A fresh self-host has probed nothing and its catalog
    // may say nothing, so `reachFor` answers with silence. Reading that silence
    // as "cannot search" would refuse the admin's own explicit choice. The
    // runtime defence is the harness floor, one layer down.
    resolveRoleModel.mockResolvedValue('mystery-model')
    reachBy({})
    expect(await planSearch('brief')).toEqual({ model: 'mystery-model', via: 'native', supplier: null })
  })
})

describe('2. a model that searches by itself, found by CAPABILITY not by name', () => {
  it('prefers it over one that would need the tool path', async () => {
    reachBy({ 'llama-70b': TOOL, 'sonar-pro': NATIVE })
    // No sonar spelling appears anywhere in the resolution — the catalog derives
    // `search` from `web_search_options`, so a new vendor shipping native search
    // is picked up the day it is registered with nothing here to edit.
    expect(await planSearch('brief', { models })).toEqual({ model: 'sonar-pro', via: 'native', supplier: null })
  })
})

describe('3. anything routable, through our own search', () => {
  it('runs on a model with no native search at all — the point of the change', async () => {
    reachBy({ 'llama-70b': TOOL, 'sonar-pro': TOOL })
    const plan = await planSearch('brief', { models })
    expect(plan?.via).toBe('tool')
    expect(plan?.supplier).toEqual({ server: 'talaria', tool: 'web_search' })
  })

  it('refuses only when the workspace can do NEITHER, and says so in fixable terms', async () => {
    // Not "register a Perplexity model". Two ways out, both named.
    reachBy({})
    expect(await planSearch('brief', { models })).toBeNull()
    expect(NO_SEARCH_REASON).toContain('search backend')
    expect(NO_SEARCH_REASON).toContain('native web search')
    expect(NO_SEARCH_REASON.toLowerCase()).not.toContain('perplexity')
    expect(NO_SEARCH_REASON.toLowerCase()).not.toContain('sonar')
  })

  it('refuses when the gateway has no models at all', async () => {
    expect(await planSearch('brief', { models: async () => [] })).toBeNull()
  })
})
