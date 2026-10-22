import { describe, expect, it } from 'vitest'
import { needsKeyFirst, renderSummary, type AgentWebSearch, type RenderedWeb, type WebBackend } from './search'

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
