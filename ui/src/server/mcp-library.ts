// The MCP server library — the OFFICIAL MCP registry, queried live and parsed
// per the LATEST server.json schema (2025-12-11: title, websiteUrl, themed
// icons, remote header declarations). Ranked for a BUSINESS platform:
//   first-party  verified reverse-DNS namespace AND the hosted endpoint (or
//                website) lives on that domain — Vercel's own mcp.vercel.com
//   verified     domain-verified namespace, hosted elsewhere
//   community    io.github.* — surfaces only in explicit searches
// Only servers with a hosted (streamable-http) endpoint appear at all: the
// gateway speaks that transport; stdio packages would need a process runtime.
import { warmIcons } from './mcp-icons'

interface RegistryInput {
  name: string
  description?: string
  isRequired?: boolean
  isSecret?: boolean
  placeholder?: string
  default?: string
  choices?: string[]
}

interface RegistryEntry {
  server: {
    name: string
    title?: string
    description?: string
    version?: string
    websiteUrl?: string
    icons?: Array<{ src: string; theme?: 'light' | 'dark'; mimeType?: string; sizes?: string }>
    remotes?: Array<{ type: string; url: string; headers?: RegistryInput[] }>
  }
  _meta?: Record<string, { status?: string; isLatest?: boolean }>
}

export interface LibraryHeader {
  name: string
  description: string | null
  isRequired: boolean
  isSecret: boolean
  placeholder: string | null
  default: string | null
  choices: string[] | null
}

export interface LibraryServer {
  /** Registry identity, e.g. "com.vercel/vercel-mcp". */
  registryName: string
  title: string
  description: string | null
  url: string
  /** The publisher's domain (websiteUrl host, else reversed namespace). */
  domain: string | null
  /** Registry-declared icon (latest schema) — hotlinked directly by the UI;
   *  the favicon proxy is only the fallback when this is absent. */
  icon: string | null
  tier: 'first-party' | 'verified' | 'community'
  /** Credential/header declarations from the remote — drive the install form. */
  requiredHeaders: LibraryHeader[]
}

const REGISTRY_URL = 'https://registry.modelcontextprotocol.io/v0/servers'
const CACHE_MS = 15 * 60 * 1000

const host = (u: string): string | null => {
  try {
    return new URL(u).hostname
  } catch {
    return null
  }
}
const onDomain = (h: string | null, domain: string) => !!h && (h === domain || h.endsWith(`.${domain}`))

const classify = (e: RegistryEntry): LibraryServer | null => {
  const official = e._meta?.['io.modelcontextprotocol.registry/official']
  if (official && (official.status !== 'active' || official.isLatest === false)) return null
  const remote = e.server.remotes?.find((x) => x.type === 'streamable-http' && /^https:\/\//.test(x.url))
  if (!remote) return null
  const ns = e.server.name.split('/')[0] ?? ''
  const community = ns.startsWith('io.github.')
  const nsDomain = community ? null : ns.split('.').reverse().join('.')
  const domain = host(e.server.websiteUrl ?? '') ?? nsDomain
  let tier: LibraryServer['tier'] = community ? 'community' : 'verified'
  if (nsDomain && (onDomain(host(remote.url), nsDomain) || onDomain(host(e.server.websiteUrl ?? ''), nsDomain))) {
    tier = 'first-party'
  }
  // Themed icons: prefer an untinted/light entry with an https src.
  const icons = (e.server.icons ?? []).filter((i) => /^https:\/\//.test(i.src))
  const icon = (icons.find((i) => !i.theme || i.theme === 'light') ?? icons[0])?.src ?? null
  // Generic titles ("mcp", "mcp-server") read as noise — brand from the
  // publisher's domain instead: stripe.com → Stripe.
  const displayDomain = (nsDomain ?? domain)?.replace(/^(www|mcp|docs|support|api)\./, '') ?? null
  let title = e.server.title ?? e.server.name.split('/').pop() ?? e.server.name
  if (/^(mcp|mcp[-_]?server|server)$/i.test(title) && displayDomain) {
    const brand = displayDomain.split('.')[0]!
    title = brand.charAt(0).toUpperCase() + brand.slice(1)
  }
  // "linear" → "Linear": lone lowercase words present as brands.
  if (/^[a-z][a-z0-9]*$/.test(title)) title = title.charAt(0).toUpperCase() + title.slice(1)
  return {
    registryName: e.server.name,
    title,
    description: e.server.description?.slice(0, 220) ?? null,
    url: remote.url,
    domain: displayDomain,
    icon,
    tier,
    requiredHeaders: (remote.headers ?? []).map((h) => ({
      name: h.name,
      description: h.description ?? null,
      isRequired: h.isRequired ?? false,
      isSecret: h.isSecret ?? false,
      placeholder: h.placeholder ?? null,
      default: h.default ?? null,
      choices: h.choices ?? null,
    })),
  }
}

const TIER_RANK: Record<LibraryServer['tier'], number> = { 'first-party': 0, verified: 1, community: 2 }

const registryPage = async (params: Record<string, string>): Promise<RegistryEntry[]> => {
  const url = new URL(REGISTRY_URL)
  for (const [k, v] of Object.entries(params)) url.searchParams.set(k, v)
  const r = await fetch(url, { signal: AbortSignal.timeout(10_000) })
  if (!r.ok) throw new Error(`registry ${r.status}`)
  return ((await r.json()) as { servers?: RegistryEntry[] }).servers ?? []
}

/** Search the registry SERVER-SIDE (complete over 20k+ entries), rank locally:
 *  first-party publishers top the results, community wrappers sink. Favicon
 *  fallbacks warm in the background as results are served. */
const searchCache = new Map<string, { at: number; results: LibraryServer[] }>()
export async function searchMcpLibrary(query: string): Promise<LibraryServer[]> {
  const q = query.trim().toLowerCase()
  const hit = searchCache.get(q)
  if (hit && Date.now() - hit.at < CACHE_MS) return hit.results
  const entries = await registryPage(q ? { search: q, limit: '100' } : { limit: '100' })
  const byName = new Map<string, LibraryServer>()
  for (const e of entries) {
    const s = classify(e)
    if (s) byName.set(s.registryName, s)
  }
  const results = [...byName.values()]
    .sort((a, b) => TIER_RANK[a.tier] - TIER_RANK[b.tier] || a.title.localeCompare(b.title))
    .slice(0, 40)
  searchCache.set(q, { at: Date.now(), results })
  warmIcons(results.filter((s) => !s.icon && s.domain).map((s) => ({ domain: s.domain })))
  return results
}

// The featured shelf is EDITORIAL — services businesses actually run on — but
// the DATA stays live: each name resolves against the registry at request
// time, and companies that haven't published a server simply don't appear.
const FEATURED_DOMAINS = [
  'github.com', 'gitlab.com', 'linear.app', 'atlassian.com', 'asana.com', 'monday.com',
  'notion.so', 'airtable.com', 'figma.com', 'canva.com', 'slack.com', 'intercom.com',
  'stripe.com', 'paypal.com', 'squareup.com', 'shopify.com', 'hubspot.com',
  'vercel.com', 'netlify.com', 'cloudflare.com', 'sentry.io', 'supabase.com',
  'neon.tech', 'mongodb.com', 'elastic.co', 'twilio.com', 'zapier.com',
  'huggingface.co', 'browserbase.com', 'e2b.dev', 'postman.com', 'linkup.so',
]

let featured: { at: number; servers: LibraryServer[] } | null = null
export async function featuredMcpLibrary(): Promise<{ servers: LibraryServer[] }> {
  if (featured && Date.now() - featured.at < 60 * 60 * 1000) return { servers: featured.servers }
  const results = await Promise.all(
    FEATURED_DOMAINS.map(async (domain) => {
      const term = domain.split('.')[0]!
      try {
        const entries = await registryPage({ search: term, limit: '30' })
        const hits = entries
          .map(classify)
          .filter((s): s is LibraryServer => !!s && (s.domain === domain || onDomain(s.domain, domain)))
          .sort((a, b) => TIER_RANK[a.tier] - TIER_RANK[b.tier])
        return hits[0] ?? null
      } catch {
        return null
      }
    }),
  )
  const servers = results.filter((s): s is LibraryServer => s !== null)
  if (servers.length > 0) featured = { at: Date.now(), servers }
  warmIcons(servers.filter((s) => !s.icon && s.domain).map((s) => ({ domain: s.domain })))
  return { servers }
}
