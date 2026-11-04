import { describe, expect, it } from 'vitest'
import { agentProbeNote, needsKeyFirst, renderSummary, type AgentWebSearch, type RenderedWeb, type SearchEndpoint, type WebBackend } from './search'

const backends: WebBackend[] = [
  { name: 'tavily', label: 'Tavily', keyEnv: 'TAVILY_API_KEY', extract: true, publicTier: true, sdkOnFirstUse: false },
  { name: 'perplexity', label: 'Perplexity Search API', keyEnv: 'PERPLEXITY_API_KEY', extract: true, publicTier: false, sdkOnFirstUse: false },
  { name: 'searxng', label: 'SearXNG (the search URL)', keyEnv: null, extract: false, publicTier: false, sdkOnFirstUse: false },
]
const cfg = (over: Partial<AgentWebSearch> = {}): AgentWebSearch => ({
  backend: null,
  extractBackend: null,
  allowKeylessPublicTiers: false,
  keys: {},
  searchUrl: '',
  backends,
  rendered: { web: {}, env: [], searxngUrl: false, warnings: [] },
  ...over,
})
const off = { keyless_fallback: false, keyless_rescue: false }
const rendered = (over: Partial<RenderedWeb>): RenderedWeb => ({ web: {}, env: [], searxngUrl: false, warnings: [], ...over })

describe('needsKeyFirst', () => {
  it('asks for a key before a keyed vendor can be picked', () => {
    expect(needsKeyFirst(cfg(), 'tavily')).toBe(true)
    expect(needsKeyFirst(cfg({ keys: { tavily: true } }), 'tavily')).toBe(false)
  })
  it('lets a vendor with an anonymous tier go keyless only when the public tiers are allowed', () => {
    expect(needsKeyFirst(cfg({ allowKeylessPublicTiers: true }), 'tavily')).toBe(false)
    expect(needsKeyFirst(cfg({ allowKeylessPublicTiers: true }), 'perplexity')).toBe(true)
  })
  it('never asks for a key a backend does not take', () => {
    expect(needsKeyFirst(cfg(), 'searxng')).toBe(false)
  })
})

describe('renderSummary', () => {
  it('says plainly when nothing is rendered', () => {
    expect(renderSummary(cfg())).toBe('nothing rendered: Hermes resolves its own default')
  })
  it("reads the server's render: backend, sealed key, keyless switch", () => {
    const r = rendered({ web: { search_backend: 'tavily', ...off }, env: ['TAVILY_API_KEY'] })
    expect(renderSummary(cfg({ backend: 'tavily', keys: { tavily: true }, rendered: r }))).toBe(
      'search: tavily · TAVILY_API_KEY sealed · keyless tiers off',
    )
  })
  it('names a rendered search URL, and the keyless tiers when they are allowed', () => {
    expect(renderSummary(cfg({ searchUrl: 'http://searx.local', rendered: rendered({ web: off, searxngUrl: true }) }))).toBe(
      'SEARXNG_URL · keyless tiers off',
    )
    expect(
      renderSummary(cfg({ searchUrl: 'http://searx.local', allowKeylessPublicTiers: true, rendered: rendered({ searxngUrl: true }) })),
    ).toBe('SEARXNG_URL · keyless tiers allowed')
  })
  it('does not count a search URL the render withholds from the agents', () => {
    // A loopback URL in a containerized stack: set, but not rendered.
    expect(renderSummary(cfg({ searchUrl: 'http://127.0.0.1:8888' }))).toBe('nothing rendered: Hermes resolves its own default')
  })
})

describe('agentProbeNote: the from-agents line under the dots', () => {
  const ep = (over: Partial<SearchEndpoint> = {}): SearchEndpoint => ({
    url: 'http://127.0.0.1:8888',
    stored: 'http://127.0.0.1:8888',
    fromEnv: false,
    reachable: true,
    error: null,
    agentReachable: true,
    agentError: null,
    probedFrom: 'Ada Ops',
    agentUrl: 'http://host.docker.internal:8888',
    ...over,
  })
  it('adds nothing when the agents reach it, or there is no URL', () => {
    expect(agentProbeNote(ep())).toBeNull()
    expect(agentProbeNote(ep({ url: '', agentReachable: null, agentError: null }))).toBeNull()
  })
  it('names where the probe ran and what it dialled when the agents cannot reach it', () => {
    expect(agentProbeNote(ep({ agentReachable: false, agentError: 'the search service at http://host.docker.internal:8888 did not answer ([Errno 111] Connection refused).' }))).toEqual({
      tone: 'danger',
      text: 'From the agents (probed in Ada Ops, dialling http://host.docker.internal:8888): the search service at http://host.docker.internal:8888 did not answer ([Errno 111] Connection refused).',
    })
  })
  it('says why nothing was probed, quietly', () => {
    expect(agentProbeNote(ep({ agentReachable: null, agentError: 'no agent is running to probe from', probedFrom: null }))).toEqual({
      tone: 'muted',
      text: 'From the agents: no agent is running to probe from',
    })
  })
  // The withheld sentence once opened "not handed to the agents", so the
  // line read "From the agents: not handed to the agents: …". The fix is in
  // the server's sentence, and the server pins it: probe.rs's
  // the_withheld_reason_does_not_repeat_the_panels_prefix. The sentence below
  // is a copy, so this case does not catch the server rewording it. What it
  // covers is the composition: the panel prints the reason verbatim after
  // its one prefix, with no rewording of its own that could say it twice.
  it('prints the withheld-loopback reason verbatim after its one prefix', () => {
    const withheld =
      'the render withholds a loopback URL, since to an agent it is the app container itself; give the search URL an address the agents can reach'
    const note = agentProbeNote(ep({ agentReachable: null, agentError: withheld, probedFrom: null, agentUrl: null }))
    expect(note).toEqual({ tone: 'muted', text: `From the agents: ${withheld}` })
    expect(note!.text).not.toMatch(/agents:.*agents:/)
  })
})
