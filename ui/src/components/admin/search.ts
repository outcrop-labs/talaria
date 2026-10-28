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
  /** The canary from the app process — what Research dials. */
  reachable: boolean
  error: string | null
  /** The same canary from inside a running agent, against the URL the
   *  render hands the agents. null: nothing was probed — no URL handed to
   *  the agents, or no agent running — and `agentError` says which. */
  agentReachable: boolean | null
  agentError: string | null
  /** The agent the probe ran in. */
  probedFrom: string | null
  /** The URL as the agents dial it (a host loopback becomes
   *  host.docker.internal). */
  agentUrl: string | null
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

/** What the render hands every agent from the saved setting, computed by
 *  the render itself (talaria-agent-web-search), never restated here. */
export interface RenderedWeb {
  /** The keys merged into every agent's config.yaml `web:` block. */
  web: Record<string, string | boolean>
  /** The Hermes env names that carry a vendor key. Never the value. */
  env: string[]
  /** The workspace search URL reaches the agents' env as SEARXNG_URL.
   *  Rendered is not reachable: the Search URL panel's "from agents" probe
   *  is what says whether they can connect to it. */
  searxngUrl: boolean
  /** What the render would warn about, e.g. web_extract landing on a
   *  search-only backend. */
  warnings: string[]
}

export interface AgentWebSearch {
  backend: string | null
  extractBackend: string | null
  allowKeylessPublicTiers: boolean
  /** Which vendors hold a sealed key. Never the value. */
  keys: Record<string, boolean>
  searchUrl: string
  backends: WebBackend[]
  rendered: RenderedWeb
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

/** What every agent's Hermes gets from the saved setting, one line, read
 *  off the server's own render (`rendered`) rather than re-derived. */
export function renderSummary(cfg: AgentWebSearch): string {
  const { web, env, searxngUrl } = cfg.rendered
  const parts: string[] = []
  if (typeof web.search_backend === 'string') parts.push(`search: ${web.search_backend}`)
  if (typeof web.extract_backend === 'string') parts.push(`extract: ${web.extract_backend}`)
  for (const name of env) parts.push(`${name} sealed`)
  if (searxngUrl) parts.push('SEARXNG_URL')
  if (web.keyless_fallback === false) parts.push('keyless tiers off')
  else if (parts.length && cfg.allowKeylessPublicTiers) parts.push('keyless tiers allowed')
  return parts.length ? parts.join(' · ') : "nothing rendered: Hermes resolves its own default"
}

/** The line under the Search URL dots for the agents' probe, or null when
 *  the dot says it all (up, or no URL to probe). A failure names where the
 *  probe ran and what it dialled, because "down from the agents but up from
 *  the app" is exactly the case where the address is the question. */
export function agentProbeNote(s: SearchEndpoint): { tone: 'danger' | 'muted'; text: string } | null {
  if (!s.url || s.agentReachable === true) return null
  if (s.agentReachable === false) {
    const where = [s.probedFrom && `probed in ${s.probedFrom}`, s.agentUrl && `dialling ${s.agentUrl}`]
      .filter(Boolean)
      .join(', ')
    return { tone: 'danger', text: `From the agents${where ? ` (${where})` : ''}: ${s.agentError ?? 'the probe failed'}` }
  }
  return s.agentError ? { tone: 'muted', text: `From the agents: ${s.agentError}` } : null
}
