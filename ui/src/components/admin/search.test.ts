import { describe, expect, it } from 'vitest'
import { needsKeyFirst, renderSummary, type AgentWebSearch, type WebBackend } from './search'

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
  ...over,
})

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
  it('names the backend, the sealed key, and the keyless switch', () => {
    expect(renderSummary(cfg({ backend: 'tavily', keys: { tavily: true } }))).toBe(
      'search: tavily · TAVILY_API_KEY sealed · keyless tiers off',
    )
  })
  it('counts a search URL alone as a choice', () => {
    expect(renderSummary(cfg({ searchUrl: 'http://searx.local' }))).toBe('keyless tiers off')
    expect(renderSummary(cfg({ searchUrl: 'http://searx.local', allowKeylessPublicTiers: true }))).toBe('keyless tiers allowed')
  })
})
