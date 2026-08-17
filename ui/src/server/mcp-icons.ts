// Marketplace icon cache. Icons resolve server-side (declared icon URL, else
// the DuckDuckGo favicon CDN — fast — else the site's own favicon) and are
// WARMED in bulk whenever a library page is served, so cards paint from cache
// instead of fanning out cold fetches per image.
//
// Both inputs are attacker-reachable: `domain` comes straight off a signed-in
// user's ?domain= query, and `src` off the public MCP registry. The proxy then
// returns the BYTES it fetched, so an unguarded fetch here is a read primitive
// against the private network. `publicIconDomain` screens the domain candidates
// but says nothing about `src`, and neither screens what a redirect lands on —
// safeFetch (redirects re-validated per hop, size capped) is what keeps this a
// favicon fetcher.
import { publicIconDomain } from '@/lib/icon-domain'
import { safeFetch } from './safe-fetch'

const cache = new Map<string, { at: number; buf: ArrayBuffer; ct: string } | { at: number; miss: true }>()
const CACHE_MS = 24 * 60 * 60 * 1000
const MAX_BYTES = 512 * 1024

const fetchIcon = async (url: string): Promise<{ buf: ArrayBuffer; ct: string } | null> => {
  try {
    const r = await safeFetch(url, { timeoutMs: 5_000, maxBytes: MAX_BYTES })
    if (!r.ok) return null
    const ct = r.headers.get('content-type') ?? ''
    if (!/^image\//.test(ct)) return null
    const buf = await r.arrayBuffer()
    if (buf.byteLength === 0 || buf.byteLength > MAX_BYTES) return null
    return { buf, ct }
  } catch {
    return null
  }
}

// Internal/non-public hosts (IP literals, single-label names, localhost) get
// no candidates — the resolver never dials inside the network and misses fast.
const candidatesFor = (key: { src?: string | null; domain?: string | null }): string[] => {
  if (key.src) return [key.src]
  const domain = publicIconDomain(key.domain)
  return domain ? [`https://icons.duckduckgo.com/ip3/${domain}.ico`, `https://${domain}/favicon.ico`] : []
}

export async function resolveIcon(key: { src?: string | null; domain?: string | null }): Promise<{ buf: ArrayBuffer; ct: string } | null> {
  const id = key.src ?? key.domain ?? ''
  if (!id) return null
  const hit = cache.get(id)
  if (hit && Date.now() - hit.at < CACHE_MS) return 'miss' in hit ? null : hit
  for (const c of candidatesFor(key)) {
    const icon = await fetchIcon(c)
    if (icon) {
      cache.set(id, { at: Date.now(), ...icon })
      return icon
    }
  }
  cache.set(id, { at: Date.now(), miss: true })
  return null
}

/** Fire-and-forget bulk warm — bounded concurrency, silent failures. */
export function warmIcons(keys: Array<{ src?: string | null; domain?: string | null }>): void {
  const fresh = keys.filter((k) => {
    const id = k.src ?? k.domain ?? ''
    const hit = id ? cache.get(id) : null
    return id && (!hit || Date.now() - hit.at >= CACHE_MS)
  })
  void (async () => {
    const queue = [...fresh]
    await Promise.all(
      Array.from({ length: 6 }, async () => {
        for (let k = queue.shift(); k; k = queue.shift()) await resolveIcon(k)
      }),
    )
  })()
}
