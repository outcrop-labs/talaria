// The MCP server library — the OFFICIAL MCP registry, queried live (never a
// hardcoded list). Entries with a streamable-http remote are one-click
// addable: Talaria's gateway speaks that transport natively. npm/stdio-only
// packages are excluded — they'd need a process runtime, not a URL.
interface RegistryEntry {
  server: {
    name: string
    title?: string
    description?: string
    version?: string
    remotes?: Array<{ type: string; url: string }>
  }
  _meta?: Record<string, { status?: string; isLatest?: boolean }>
}

export interface LibraryServer {
  /** Registry identity, e.g. "com.github/mcp". */
  registryName: string
  title: string
  description: string | null
  url: string
}

const REGISTRY_URL = 'https://registry.modelcontextprotocol.io/v0/servers'
const cache = new Map<string, { at: number; results: LibraryServer[] }>()
const CACHE_MS = 15 * 60 * 1000

export async function searchMcpLibrary(query: string): Promise<LibraryServer[]> {
  const key = query.trim().toLowerCase()
  const hit = cache.get(key)
  if (hit && Date.now() - hit.at < CACHE_MS) return hit.results

  const url = new URL(REGISTRY_URL)
  if (key) url.searchParams.set('search', key)
  url.searchParams.set('limit', '50')
  const r = await fetch(url, { signal: AbortSignal.timeout(10_000) })
  if (!r.ok) throw new Error(`registry ${r.status}`)
  const entries = ((await r.json()) as { servers?: RegistryEntry[] }).servers ?? []

  // Latest active version per name, remote-capable only.
  const byName = new Map<string, LibraryServer>()
  for (const e of entries) {
    const official = e._meta?.['io.modelcontextprotocol.registry/official']
    if (official && official.status !== 'active') continue
    const remote = e.server.remotes?.find((x) => x.type === 'streamable-http' && /^https:\/\//.test(x.url))
    if (!remote) continue
    if (byName.has(e.server.name) && official?.isLatest !== true) continue
    byName.set(e.server.name, {
      registryName: e.server.name,
      title: e.server.title ?? e.server.name.split('/').pop() ?? e.server.name,
      description: e.server.description?.slice(0, 200) ?? null,
      url: remote.url,
    })
  }
  const results = [...byName.values()].slice(0, 25)
  cache.set(key, { at: Date.now(), results })
  return results
}
