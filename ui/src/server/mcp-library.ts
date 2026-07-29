// The MCP server library — the OFFICIAL MCP registry, queried live (never a
// hardcoded list) and ranked for a BUSINESS platform:
//   first-party  the namespace is a verified reverse-DNS domain AND the
//                hosted endpoint lives on that same domain — Vercel's own
//                mcp.vercel.com, Stripe's own, Linear's own. The default view.
//   verified     domain-verified namespace, endpoint hosted elsewhere.
//   community    io.github.* — shown only when searched for explicitly.
// Only servers with a hosted (streamable-http) endpoint appear at all: the
// gateway speaks that transport; npm/stdio packages would need a process
// runtime, not a URL.
interface RegistryEntry {
  server: {
    name: string
    title?: string
    description?: string
    version?: string
    remotes?: Array<{ type: string; url: string }>
    icons?: Array<{ src: string }>
  }
  _meta?: Record<string, { status?: string; isLatest?: boolean }>
}

export interface LibraryServer {
  /** Registry identity, e.g. "com.vercel/vercel-mcp". */
  registryName: string
  title: string
  description: string | null
  url: string
  /** The publisher's verified domain (reverse of the namespace), e.g. "vercel.com". */
  domain: string | null
  /** Registry-declared icon URL, when the entry carries one. */
  icon: string | null
  tier: 'first-party' | 'verified' | 'community'
}

const REGISTRY_URL = 'https://registry.modelcontextprotocol.io/v0/servers'
const CACHE_MS = 15 * 60 * 1000

const classify = (e: RegistryEntry): LibraryServer | null => {
  const official = e._meta?.['io.modelcontextprotocol.registry/official']
  if (official && (official.status !== 'active' || official.isLatest === false)) return null
  const remote = e.server.remotes?.find((x) => x.type === 'streamable-http' && /^https:\/\//.test(x.url))
  if (!remote) return null
  const ns = e.server.name.split('/')[0] ?? ''
  const community = ns.startsWith('io.github.')
  const domain = community ? null : ns.split('.').reverse().join('.')
  let tier: LibraryServer['tier'] = community ? 'community' : 'verified'
  if (domain) {
    try {
      const host = new URL(remote.url).hostname
      if (host === domain || host.endsWith(`.${domain}`)) tier = 'first-party'
    } catch {
      /* unparseable url — stays verified */
    }
  }
  return {
    registryName: e.server.name,
    title: e.server.title ?? e.server.name.split('/').pop() ?? e.server.name,
    description: e.server.description?.slice(0, 200) ?? null,
    url: remote.url,
    domain,
    icon: e.server.icons?.find((i) => /^https:\/\//.test(i.src))?.src ?? null,
    tier,
  }
}

const TIER_RANK: Record<LibraryServer['tier'], number> = { 'first-party': 0, verified: 1, community: 2 }

/** Search the registry SERVER-SIDE (complete, 20k+ entries) and rank locally:
 *  first-party publishers top the results, community wrappers sink. */
const searchCache = new Map<string, { at: number; results: LibraryServer[] }>()
export async function searchMcpLibrary(query: string): Promise<LibraryServer[]> {
  const q = query.trim().toLowerCase()
  const hit = searchCache.get(q)
  if (hit && Date.now() - hit.at < CACHE_MS) return hit.results
  const url = new URL(REGISTRY_URL)
  if (q) url.searchParams.set('search', q)
  url.searchParams.set('limit', '100')
  const r = await fetch(url, { signal: AbortSignal.timeout(10_000) })
  if (!r.ok) throw new Error(`registry ${r.status}`)
  const entries = ((await r.json()) as { servers?: RegistryEntry[] }).servers ?? []
  const byName = new Map<string, LibraryServer>()
  for (const e of entries) {
    const s = classify(e)
    if (s) byName.set(s.registryName, s)
  }
  const results = [...byName.values()]
    .sort((a, b) => TIER_RANK[a.tier] - TIER_RANK[b.tier] || a.title.localeCompare(b.title))
    .slice(0, 40)
  searchCache.set(q, { at: Date.now(), results })
  return results
}

/** The featured shelf: first-party servers (companies hosting their own MCP
 *  endpoint on their own domain), collected via a bounded cursor sweep and
 *  pageable with "load more". */
const featuredCache = new Map<string, { at: number; servers: LibraryServer[]; nextCursor: string | null }>()
export async function featuredMcpLibrary(cursor: string | null): Promise<{ servers: LibraryServer[]; nextCursor: string | null }> {
  const key = cursor ?? ''
  const hit = featuredCache.get(key)
  if (hit && Date.now() - hit.at < CACHE_MS) return { servers: hit.servers, nextCursor: hit.nextCursor }
  const byName = new Map<string, LibraryServer>()
  let cur = cursor
  for (let page = 0; page < 12 && byName.size < 24; page++) {
    const url = new URL(REGISTRY_URL)
    url.searchParams.set('limit', '100')
    if (cur) url.searchParams.set('cursor', cur)
    const r = await fetch(url, { signal: AbortSignal.timeout(10_000) })
    if (!r.ok) break
    const data = (await r.json()) as { servers?: RegistryEntry[]; metadata?: { nextCursor?: string } }
    for (const e of data.servers ?? []) {
      const s = classify(e)
      if (s && s.tier === 'first-party') byName.set(s.registryName, s)
    }
    cur = data.metadata?.nextCursor ?? null
    if (!cur) break
  }
  const out = { servers: [...byName.values()].slice(0, 24), nextCursor: cur }
  featuredCache.set(key, { at: Date.now(), ...out })
  return out
}
