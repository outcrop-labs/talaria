// The MCP server library — the OFFICIAL MCP registry, queried live and parsed
// per the LATEST server.json schema (2025-12-11: title, websiteUrl, themed
// icons, remote header declarations). Ranked for a BUSINESS platform:
//   first-party  verified reverse-DNS namespace AND the hosted endpoint (or
//                website) lives on that domain — Vercel's own mcp.vercel.com
//   verified     domain-verified namespace, hosted elsewhere
//   community    io.github.* — surfaces only in explicit searches
// Only servers with a hosted (streamable-http) endpoint appear at all: the
// gateway speaks that transport; stdio packages would need a process runtime.
//
// SERVING STRATEGY — why this module has three layers of caching:
//   1. Bounded concurrency. The registry answers one search in ~100-400ms and
//      THROTTLES bursts: the featured shelf used to resolve all 32 publishers
//      through one unbounded Promise.all, half the fan-out queued for 3-4.6s,
//      and the shelf (which waits for the slowest) took ~4.6s — the
//      "marketplace loads super slowly" report. Six in flight stays in the
//      registry's fast lane: measured 271ms pooled vs 4615ms unbounded.
//   2. Stale-while-revalidate. The featured shelf is editorial — an hour-old
//      answer NOW beats a several-second fan-out for a fresh one. Stale data
//      is served instantly and refreshed in the background, single-flighted.
//   3. A scheduler job warms the shelf at boot and every 30 minutes, so even
//      the one truly cold load (first request ever, registry up) usually
//      happens before any user opens the marketplace.
import { registerJob } from './scheduler'
import { warmIcons } from './mcp-icons'
import { safeFetch } from './safe-fetch'

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

/** Same number warmIcons uses; measured to stay under the registry's burst
 *  throttle (see the header). See mapPool for how it is applied. */
const REGISTRY_CONCURRENCY = 6

/** items → promises → results, at most `limit` in flight, results in input
 *  order. The queue discipline is the whole point: an unbounded fan-out is
 *  what made the marketplace slow. */
async function mapPool<T, R>(items: readonly T[], limit: number, fn: (item: T) => Promise<R>): Promise<R[]> {
  const out = new Array<R>(items.length)
  let next = 0
  await Promise.all(
    Array.from({ length: Math.min(limit, items.length) }, async () => {
      for (let i = next++; i < items.length; i = next++) out[i] = await fn(items[i]!)
    }),
  )
  return out
}

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
  // Third-party feed (and, for the well-known probe below, an arbitrary
  // publisher domain) — both go through the SSRF guard.
  const r = await safeFetch(url, { timeoutMs: 10_000 })
  if (!r.ok) throw new Error(`registry ${r.status}`)
  return ((await r.json()) as { servers?: RegistryEntry[] }).servers ?? []
}

// ── Publisher resolution beyond the registry ────────────────────────────────
// Not every company has registered (GitHub hasn't). Two live fallbacks:
//   1. the /.well-known/mcp.json convention on the publisher's domain
//      (Notion serves it: name/description/icon/endpoint)
//   2. a tiny factual map of DOCUMENTED official endpoints for majors absent
//      from both — the exception that keeps GitHub findable, not a catalog.
const KNOWN_ENDPOINTS: Record<string, { title: string; url: string; description: string }> = {
  'github.com': {
    title: 'GitHub',
    url: 'https://api.githubcopilot.com/mcp/',
    description: "GitHub's official MCP server — repos, issues, PRs, actions — via OAuth.",
  },
}

const wellKnownCache = new Map<string, { at: number; server: LibraryServer | null }>()
async function wellKnownServer(domain: string): Promise<LibraryServer | null> {
  const hit = wellKnownCache.get(domain)
  if (hit && Date.now() - hit.at < 60 * 60 * 1000) return hit.server
  let server: LibraryServer | null = null
  try {
    const r = await safeFetch(`https://${domain}/.well-known/mcp.json`, { timeoutMs: 5_000, maxBytes: 256 * 1024 })
    if (r.ok) {
      const j = (await r.json()) as { name?: string; description?: string; icon?: string; endpoint?: string }
      if (j.endpoint && /^https:\/\//.test(j.endpoint)) {
        server = {
          registryName: `wk:${domain}`,
          title: j.name ?? domain.split('.')[0]!,
          description: j.description?.slice(0, 220) ?? null,
          url: j.endpoint,
          domain,
          icon: j.icon && /^https:\/\//.test(j.icon) ? j.icon : null,
          tier: 'first-party',
          requiredHeaders: [],
        }
      }
    }
  } catch {
    /* no well-known — fall through */
  }
  wellKnownCache.set(domain, { at: Date.now(), server })
  return server
}

const knownServer = (domain: string): LibraryServer | null => {
  const k = KNOWN_ENDPOINTS[domain]
  if (!k) return null
  return {
    registryName: `known:${domain}`,
    title: k.title,
    description: k.description,
    url: k.url,
    domain,
    icon: null,
    tier: 'first-party',
    requiredHeaders: [],
  }
}

const PUBLISHER_TTL_MS = 60 * 60 * 1000
const publisherCache = new Map<string, { at: number; server: LibraryServer | null }>()

/** One publisher, best source wins: registry → well-known → documented.
 *  Resolved at most once an hour per domain, and the featured refresh warms
 *  every FEATURED_DOMAIN — so a brand-shaped search ("git", "stripe") pins
 *  its publisher from cache instead of re-resolving it live. */
async function resolvePublisher(domain: string): Promise<LibraryServer | null> {
  const hit = publisherCache.get(domain)
  if (hit && Date.now() - hit.at < PUBLISHER_TTL_MS) return hit.server
  const term = domain.split('.')[0]!
  let server: LibraryServer | null = null
  try {
    const entries = await registryPage({ search: term, limit: '30' })
    const hits = entries
      .map(classify)
      .filter((s): s is LibraryServer => !!s && (s.domain === domain || onDomain(s.domain, domain)))
      .sort((a, b) => TIER_RANK[a.tier] - TIER_RANK[b.tier])
    server = hits[0] ?? null
  } catch {
    /* registry hiccup — try the fallbacks */
  }
  server ??= (await wellKnownServer(domain)) ?? knownServer(domain)
  publisherCache.set(domain, { at: Date.now(), server })
  return server
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
  let results = [...byName.values()]
    .sort((a, b) => TIER_RANK[a.tier] - TIER_RANK[b.tier] || a.title.localeCompare(b.title))
    .slice(0, 40)
  // A brand-shaped query ("github", "notion") pins the publisher's OFFICIAL
  // server on top — resolved through the full chain, so a company the
  // registry lacks (GitHub) still lands first instead of wrapper noise.
  if (q && /^[a-z0-9 ]{2,20}$/.test(q)) {
    const brands = [...FEATURED_DOMAINS, ...Object.keys(KNOWN_ENDPOINTS)]
      .filter((d, i, a) => a.indexOf(d) === i)
      .filter((d) => d.split('.')[0]!.startsWith(q.replace(/\s+/g, '')))
      .slice(0, 3)
    const pinned = (await Promise.all(brands.map(resolvePublisher))).filter((s): s is LibraryServer => s !== null)
    const pinnedUrls = new Set(pinned.map((s) => s.url))
    results = [...pinned, ...results.filter((s) => !pinnedUrls.has(s.url))].slice(0, 40)
  }
  searchCache.set(q, { at: Date.now(), results })
  warmIcons(results.filter((s) => !s.icon && s.domain).map((s) => ({ domain: s.domain })))
  return results
}

// The featured shelf is EDITORIAL — services businesses actually run on — but
// the DATA stays live: each name resolves against the registry at request
// time, and companies that haven't published a server simply don't appear.
// Exported so tests (and anything that asks "is this publisher featured?")
// read the one true list instead of a copy that drifts.
export const FEATURED_DOMAINS = [
  'github.com', 'gitlab.com', 'linear.app', 'atlassian.com', 'asana.com', 'monday.com',
  'notion.so', 'airtable.com', 'figma.com', 'canva.com', 'slack.com', 'intercom.com',
  'stripe.com', 'paypal.com', 'squareup.com', 'shopify.com', 'hubspot.com',
  'vercel.com', 'netlify.com', 'cloudflare.com', 'sentry.io', 'supabase.com',
  'neon.tech', 'mongodb.com', 'elastic.co', 'twilio.com', 'zapier.com',
  'huggingface.co', 'browserbase.com', 'e2b.dev', 'postman.com', 'linkup.so',
]

const FEATURED_FRESH_MS = 60 * 60 * 1000
let featured: { at: number; servers: LibraryServer[] } | null = null
let featuredRefresh: Promise<LibraryServer[]> | null = null

/** Resolve the featured shelf from live sources and cache the result. Never
 *  throws for per-domain failures — a publisher that cannot be resolved just
 *  drops off the shelf until it can be. */
async function refreshFeatured(): Promise<LibraryServer[]> {
  const servers = (await mapPool(FEATURED_DOMAINS, REGISTRY_CONCURRENCY, resolvePublisher)).filter(
    (s): s is LibraryServer => s !== null,
  )
  // Only a NON-EMPTY answer replaces what's cached: an empty or partial one
  // means the registry is misbehaving, and evicting a good shelf for it would
  // trade a stale shelf for an empty one.
  if (servers.length > 0) featured = { at: Date.now(), servers }
  warmIcons(servers.filter((s) => !s.icon && s.domain).map((s) => ({ domain: s.domain })))
  return servers
}

/** Single-flight the refresh so N concurrent stale serves trigger ONE
 *  fan-out, not N. The in-flight slot clears on settlement, success or not. */
const refreshFeaturedOnce = (): Promise<LibraryServer[]> =>
  (featuredRefresh ??= (async () => {
    try {
      return await refreshFeatured()
    } finally {
      featuredRefresh = null
    }
  })())

export async function featuredMcpLibrary(): Promise<{ servers: LibraryServer[] }> {
  if (featured) {
    // Stale-while-revalidate: serve the shelf we have, however old, and only
    // then kick a refresh. The only caller who ever blocks on the fan-out is
    // the one who arrives before ANY answer exists — in production the
    // scheduler job below has usually been there first.
    if (Date.now() - featured.at >= FEATURED_FRESH_MS) void refreshFeaturedOnce().catch(() => {})
    return { servers: featured.servers }
  }
  return { servers: await refreshFeaturedOnce() }
}

// Scheduled, not on-demand-only. Without this, the first marketplace open
// after every boot — or after the hourly TTL expired with nobody looking —
// paid the full 32-publisher fan-out, skeletons and all. perInstance because
// the cache being warmed is THIS process's memory; the upstream work is
// read-only, so instances duplicating it is the intended behavior.
registerJob({
  name: 'mcp-library-refresh',
  everyMs: 30 * 60_000,
  // Early: it warms a cache, writes nothing anyone sees, and the whole point
  // is to be done before the first user opens the marketplace.
  firstRunDelayMs: 10_000,
  maxRunMs: 2 * 60_000,
  perInstance: true,
  run: async () => {
    const servers = await refreshFeaturedOnce()
    return `${servers.length} featured marketplace server(s)`
  },
})
