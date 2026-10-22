// Admin → Search: the workspace search URL (/api/admin/search) and the agent
// web-search setting (/api/admin/search/agents). Types, queries, and the
// pure decisions the panel makes, kept here so they can be tested.
import { createQuery } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'

/** The workspace's SearXNG-compatible endpoint. Research searches it, and
 *  the fleet render hands it to every agent as SEARXNG_URL. */
export interface SearchEndpoint {
  url: string
  stored: string
  /** SEARXNG_URL is set in the api's environment, and it wins. */
  fromEnv: boolean
  reachable: boolean
  error: string | null
}

/** One Hermes web backend this build offers (talaria-agent-web-search). */
export interface WebBackend {
  name: string
  label: string
  /** The env var Hermes's provider reads its key from; null takes no key. */
  keyEnv: string | null
  extract: boolean
  /** Keyless, Hermes would send this vendor's calls to its anonymous tier. */
  publicTier: boolean
  /** Hermes installs the vendor SDK into the agent on first use. */
  sdkOnFirstUse: boolean
}

export interface AgentWebSearch {
  backend: string | null
  extractBackend: string | null
  allowKeylessPublicTiers: boolean
  /** Which vendors hold a sealed key. Never the value. */
  keys: Record<string, boolean>
  searchUrl: string
  backends: WebBackend[]
}

export function useSearchEndpoint() {
  return createQuery(() => ({
    queryKey: ['admin-search'],
    queryFn: (): Promise<SearchEndpoint> => getJson<SearchEndpoint>('/api/admin/search'),
  }))
}

export function useAgentWebSearch() {
  return createQuery(() => ({
    queryKey: ['admin-search-agents'],
    queryFn: (): Promise<AgentWebSearch> => getJson<AgentWebSearch>('/api/admin/search/agents'),
  }))
}

/** Picking `name` cannot be saved on its own: the vendor takes a key, none
 *  is stored, and without one the server would refuse (no anonymous tier,
 *  or the public tiers are not allowed). The panel then asks for the key and
 *  saves both together. */
export function needsKeyFirst(cfg: AgentWebSearch, name: string): boolean {
  const b = cfg.backends.find((x) => x.name === name)
  if (!b?.keyEnv || cfg.keys[name]) return false
  return !b.publicTier || !cfg.allowKeylessPublicTiers
}

/** What every agent's Hermes gets from this setting, one line, in the
 *  render's own terms. Mirrors talaria-agent-web-search's `render`. */
export function renderSummary(cfg: AgentWebSearch): string {
  const parts: string[] = []
  const vendor = (name: string | null) => cfg.backends.find((b) => b.name === name)
  const search = vendor(cfg.backend)
  const extract = vendor(cfg.extractBackend)
  if (search) parts.push(`search: ${search.name}`)
  if (extract) parts.push(`extract: ${extract.name}`)
  for (const b of [search, extract]) {
    if (b?.keyEnv && cfg.keys[b.name] && !parts.includes(`${b.keyEnv} sealed`)) parts.push(`${b.keyEnv} sealed`)
  }
  const chosen = Boolean(search || extract || cfg.searchUrl)
  if (chosen) parts.push(cfg.allowKeylessPublicTiers ? 'keyless tiers allowed' : 'keyless tiers off')
  return parts.length ? parts.join(' · ') : "nothing rendered: Hermes resolves its own default"
}
