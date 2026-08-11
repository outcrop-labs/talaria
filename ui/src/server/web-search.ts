// HOW THIS DEPLOYMENT SEARCHES THE WEB — one answer, for every caller.
//
// THE INCONSISTENCY THIS FILE ENDS. Talaria had two of them. `supplierFor`
// (capability-reach.ts) resolved search the way the whole capability model
// says it should — the org's registered MCP servers first, Talaria's own
// SearXNG as the floor underneath — and the fitness sweep and the research
// harness both used it. Meanwhile `/api/search`, which is the endpoint behind
// the `web_search` MCP tool, called `searchWeb` directly and was therefore
// nailed to SearXNG forever.
//
// That is backwards, and expensively so. `/api/search` is what every HERMES
// AGENT reaches — the containerized personas doing real work for real people —
// so an org that had gone and registered Exa or Tavily got it in their research
// pipeline and in the fitness matrix, and their actual working agents kept
// getting a self-hosted metasearch instance held together by mojeek and bing.
// The callers who needed the good search were the ones who could not reach it.
//
// SEARXNG IS THE FLOOR, NOT THE CEILING, and that is the whole design. A fresh
// install with no keys and no registry has working web search on day one. An
// org that registers a real provider upgrades every caller at once — agents,
// research, the fitness sweep — with no code change and nothing to migrate,
// because they all ask this file.
import { searchWeb, type SearchResult, DEFAULT_LIMIT } from './search'
import { supplierFor, PROVIDERS_KEY, type CapabilityProviders } from './capability-reach'
import { isPlatformServer, platformSupply } from './capability-platform'
import { callMcpTool, listMcpServers } from './mcp-registry'
import { getSetting } from './audit'

export interface WebSearch {
  results: SearchResult[]
  /** WHO ACTUALLY ANSWERED. Reported rather than hidden because "we searched"
   *  and "we searched with Exa" are different claims, and an admin debugging a
   *  thin result set needs to know which engine to go and look at. Null means
   *  Talaria's own instance. */
  via: { server: string; tool: string } | null
}

export interface WebSearchDeps {
  servers: typeof listMcpServers
  providers: () => Promise<CapabilityProviders>
  platform: typeof platformSupply
  callTool: (server: string, tool: string, args: Record<string, unknown>) => Promise<{ text: string; structured: unknown }>
  own: typeof searchWeb
}

const REAL: WebSearchDeps = {
  servers: listMcpServers,
  providers: () => getSetting<CapabilityProviders>(PROVIDERS_KEY, {}),
  platform: platformSupply,
  callTool: (server, tool, args) => callMcpTool(server, tool, args),
  own: searchWeb,
}

/** Search the web with whatever this deployment has, best first.
 *
 *  THROWS RATHER THAN RETURNING EMPTY when nothing can search, because the
 *  caller is a tool handler and its error text lands in an agent's transcript.
 *  A model handed an empty result set answers from memory in a confident voice;
 *  a model handed a sentence saying search is unavailable says so. */
export async function searchTheWeb(query: string, opts: { limit?: number; deps?: Partial<WebSearchDeps> } = {}): Promise<WebSearch> {
  const d = { ...REAL, ...opts.deps }
  const limit = opts.limit ?? DEFAULT_LIMIT

  const [servers, providers, platform] = await Promise.all([
    d.servers().catch(() => []),
    d.providers().catch((): CapabilityProviders => ({})),
    d.platform().catch(() => []),
  ])
  const supplier = supplierFor('search', servers, providers, platform)

  // Nothing registered and nothing of our own that works — `platformSupply`
  // withholds SearXNG when its canary query comes back empty, which is what a
  // CAPTCHA-walled instance looks like. Refusing here is the honest answer.
  if (!supplier) {
    throw new Error(
      'live web search is not available in this workspace: no search provider is registered and this deployment has no working search engine of its own. Tell whoever asked that you could not search rather than answering from memory.',
    )
  }

  // OUR OWN ENGINE GOES STRAIGHT TO THE CLIENT, not out through `callMcpTool` —
  // Talaria is in nobody's MCP registry, so routing it there is the exact bug
  // that made the platform supplier a lie the first time (see `callPlatformTool`).
  if (isPlatformServer(supplier.server)) return { results: await d.own(query, { limit }), via: null }

  const out = await d.callTool(supplier.server, supplier.tool, { query, limit })
  return { results: resultsFromPayload(out.structured ?? out.text, supplier.server).slice(0, limit), via: supplier }
}

/** A REGISTERED TOOL'S PAYLOAD, NORMALISED — and it has to be shape-agnostic,
 *  because every provider spells a result differently and none of them owes us
 *  SearXNG's field names. Walks the whole tree for anything carrying an http URL
 *  and takes the neighbouring title and snippet, whatever they are called.
 *
 *  A RESULT WITHOUT A URL IS DROPPED, the same rule `resultsFrom` applies to our
 *  own engine: an uncitable result in a research pipeline is worse than one
 *  fewer result, because `ungrounded_ref` grounds every claim against these. */
export function resultsFromPayload(payload: unknown, engine: string, cap = 25): SearchResult[] {
  const out: SearchResult[] = []
  const seen = new Set<string>()
  const walk = (node: unknown, depth: number): void => {
    if (out.length >= cap || depth > 6 || !node) return
    if (Array.isArray(node)) {
      for (const item of node) walk(item, depth + 1)
      return
    }
    if (typeof node !== 'object') return
    const o = node as Record<string, unknown>
    const url = [o.url, o.link, o.href, o.source].find((v) => typeof v === 'string' && /^https?:\/\//i.test(v)) as string | undefined
    if (url && !seen.has(url)) {
      seen.add(url)
      const title = [o.title, o.name, o.heading].find((v) => typeof v === 'string') as string | undefined
      const snippet = [o.snippet, o.description, o.text, o.content, o.summary].find((v) => typeof v === 'string') as string | undefined
      out.push({ title: title ?? url, url, snippet: snippet ? snippet.slice(0, 600) : '', engine })
    }
    for (const v of Object.values(o)) walk(v, depth + 1)
  }
  walk(payload, 0)
  return out
}
