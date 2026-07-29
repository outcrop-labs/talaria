import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'

// Marketplace icons: the publisher's favicon, proxied + cached server-side so
// the browser never talks to third parties. ?src= serves a registry-declared
// icon URL; ?domain= tries the site's own favicon, then a resolver fallback.
const cache = new Map<string, { at: number; buf: ArrayBuffer; ct: string } | { at: number; miss: true }>()
const CACHE_MS = 24 * 60 * 60 * 1000
const MAX_BYTES = 512 * 1024

const fetchIcon = async (url: string): Promise<{ buf: ArrayBuffer; ct: string } | null> => {
  try {
    const r = await fetch(url, { signal: AbortSignal.timeout(6_000), redirect: 'follow' })
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

export const Route = createFileRoute('/api/mcp/icon')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        if (!(await getSessionUser(request))) return json({ error: 'unauthorized' }, { status: 401 })
        const url = new URL(request.url)
        const src = url.searchParams.get('src')
        const domain = url.searchParams.get('domain')
        const key = src ?? domain ?? ''
        if (!key || (domain && !/^[a-z0-9.-]+$/i.test(domain)) || (src && !/^https:\/\//.test(src))) {
          return json({ error: 'bad request' }, { status: 400 })
        }
        const hit = cache.get(key)
        if (hit && Date.now() - hit.at < CACHE_MS) {
          if ('miss' in hit) return new Response(null, { status: 404 })
          return new Response(hit.buf, { headers: { 'content-type': hit.ct, 'cache-control': 'public, max-age=86400' } })
        }
        const candidates = src
          ? [src]
          : [`https://${domain}/favicon.ico`, `https://icons.duckduckgo.com/ip3/${domain}.ico`]
        for (const c of candidates) {
          const icon = await fetchIcon(c)
          if (icon) {
            cache.set(key, { at: Date.now(), ...icon })
            return new Response(icon.buf, { headers: { 'content-type': icon.ct, 'cache-control': 'public, max-age=86400' } })
          }
        }
        cache.set(key, { at: Date.now(), miss: true })
        return new Response(null, { status: 404 })
      },
    },
  },
})
