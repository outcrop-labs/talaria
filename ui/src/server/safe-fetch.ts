// SSRF guard — the door every outbound request to a URL that came from a USER,
// an AGENT, a DB row, or an upstream server's own response has to walk through.
//
// The threat: Talaria's server sits inside a private network with a cloud
// metadata service at 169.254.169.254, a Qdrant on loopback, a Postgres, a
// Redis, and whatever else the operator runs. Anyone who can persuade Talaria
// to fetch a URL of their choosing is, without this module, fetching from
// INSIDE that network with Talaria's own reachability — and (for the MCP OAuth
// flow) sometimes with a client_secret attached.
//
// What this module does:
//   • http(s) only — no file:, gopher:, ftp:, data:, jar:, …
//   • resolves the hostname and refuses loopback / RFC1918 / link-local
//     (169.254 — the metadata service) / CGNAT / multicast / reserved space,
//     IPv4 and IPv6, including IPv4-mapped, 6to4 and NAT64 disguises
//   • refuses .internal / .local / .home.arpa / localhost by name
//   • re-validates AFTER EVERY REDIRECT. `redirect: 'follow'` resolves the
//     chain inside fetch(), where we never see the hops — so we drive the
//     chain ourselves with `redirect: 'manual'` and re-check each Location.
//     Without this, an allowed public host 302s straight to 169.254.169.254.
//   • strips Authorization / Cookie / api-key-ish headers on a cross-origin
//     redirect, so a hostile upstream can't harvest credentials by bouncing us
//   • bounds every request: overall timeout + response size cap
//
// DNS rebinding, honestly:
//   Pre-flight resolution has a TOCTOU window — an attacker's DNS can answer
//   "public" for our check and "127.0.0.1" for the connection a millisecond
//   later. Where the `undici` package is loadable we close that window by
//   handing fetch a dispatcher whose connect-time `lookup` re-validates the
//   address actually being dialled, which is the resolution the socket uses.
//   That is best-effort: undici reaches us transitively today, so if it ever
//   disappears we fall back to pre-flight validation alone and the rebinding
//   window reopens. Nothing here defends against a service Talaria is SUPPOSED
//   to reach being hostile, against redirects performed by a proxy in front of
//   us, or against an operator allowlisting something they shouldn't.
//
// What deliberately does NOT come through here: Talaria's own first-party
// infrastructure — Qdrant (TALARIA_QDRANT_URL), the embedder (TALARIA_EMBED_URL),
// object storage (TALARIA_S3_URL), the built-in MCP toolkit on loopback, and
// admin-registered LLM endpoints. Those URLs are set by the OPERATOR, not by a
// user or an agent, and they legitimately point at loopback/docker-internal
// addresses. Routing them through the blocking path would break the product
// while protecting nothing. Each such call site says so at the call site.
//
// TALARIA_FETCH_ALLOW_HOSTS is the escape hatch for operators who really do
// want user-configurable targets on their internal network (a self-hosted MCP
// server on the LAN, say). Comma/space separated, each entry either
//   a host        mcp.corp.example     (exact, case-insensitive)
//   a host suffix *.corp.example       (matches the domain and anything under it)
//   an address    127.0.0.1
//   a CIDR        10.42.0.0/16         (permits resolved addresses in range)
// A hostname match short-circuits address checking entirely; a CIDR match
// permits addresses in that range wherever they were resolved from.
import { lookup as dnsLookupCb } from 'node:dns'
import { lookup as dnsLookup } from 'node:dns/promises'

export class BlockedUrlError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'BlockedUrlError'
  }
}

// ── Address classification ──────────────────────────────────────────────────

const v4Bytes = (ip: string): Uint8Array | null => {
  const parts = ip.split('.')
  if (parts.length !== 4) return null
  const out = new Uint8Array(4)
  for (let i = 0; i < 4; i++) {
    const p = parts[i]!
    if (!/^\d{1,3}$/.test(p)) return null
    const n = Number(p)
    if (n > 255) return null
    out[i] = n
  }
  return out
}

const v6Bytes = (input: string): Uint8Array | null => {
  let s = input.replace(/^\[/, '').replace(/\]$/, '').split('%')[0] ?? ''
  if (!s.includes(':')) return null
  // A dotted-quad tail (::ffff:127.0.0.1) folds into the last two groups.
  const dotted = /(\d{1,3}(?:\.\d{1,3}){3})$/.exec(s)
  if (dotted) {
    const v4 = v4Bytes(dotted[1]!)
    if (!v4) return null
    const g1 = ((v4[0]! << 8) | v4[1]!).toString(16)
    const g2 = ((v4[2]! << 8) | v4[3]!).toString(16)
    s = `${s.slice(0, dotted.index)}${g1}:${g2}`
  }
  const halves = s.split('::')
  if (halves.length > 2) return null
  const head = halves[0] ? halves[0].split(':') : []
  const tail = halves.length === 2 ? (halves[1] ? halves[1].split(':') : []) : []
  if (halves.length === 1 ? head.length !== 8 : head.length + tail.length > 7) return null
  const groups = [...head, ...new Array<string>(8 - head.length - tail.length).fill('0'), ...tail]
  const out = new Uint8Array(16)
  for (let i = 0; i < 8; i++) {
    const g = groups[i]!
    if (!/^[0-9a-fA-F]{1,4}$/.test(g)) return null
    const n = parseInt(g, 16)
    out[i * 2] = n >> 8
    out[i * 2 + 1] = n & 0xff
  }
  return out
}

/** Parsed address bytes, with v6 wrappers around a v4 address unwrapped so the
 *  v4 rules apply to ::ffff:10.0.0.1, 2002:0a00:0001::, 64:ff9b::10.0.0.1. */
const ipBytes = (ip: string): Uint8Array | null => {
  const v4 = v4Bytes(ip)
  if (v4) return v4
  const v6 = v6Bytes(ip)
  if (!v6) return null
  const isPrefix = (bytes: number[]) => bytes.every((b, i) => v6[i] === b)
  if (isPrefix([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff])) return v6.slice(12) // ::ffff:a.b.c.d
  if (isPrefix([0x20, 0x02])) return v6.slice(2, 6) // 6to4
  if (isPrefix([0x00, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0])) return v6.slice(12) // NAT64
  return v6
}

interface Cidr {
  bytes: Uint8Array
  bits: number
}

const parseCidr = (spec: string): Cidr | null => {
  const [addr, maskStr] = spec.split('/')
  if (!addr) return null
  const bytes = ipBytes(addr.trim())
  if (!bytes) return null
  const bits = maskStr === undefined ? bytes.length * 8 : Number(maskStr)
  if (!Number.isInteger(bits) || bits < 0 || bits > bytes.length * 8) return null
  return { bytes, bits }
}

const inCidr = (ip: Uint8Array, c: Cidr): boolean => {
  if (ip.length !== c.bytes.length) return false
  const full = c.bits >> 3
  for (let i = 0; i < full; i++) if (ip[i] !== c.bytes[i]) return false
  const rem = c.bits & 7
  if (rem === 0) return true
  const mask = 0xff << (8 - rem)
  return (ip[full]! & mask) === (c.bytes[full]! & mask)
}

// Everything that is not globally routable public internet. Fail closed:
// anything reserved, private, or special-use is refused.
const BLOCKED: Array<[string, string]> = [
  ['0.0.0.0/8', 'unspecified/this-network'],
  ['10.0.0.0/8', 'private network (RFC1918)'],
  ['100.64.0.0/10', 'carrier-grade NAT (RFC6598)'],
  ['127.0.0.0/8', 'loopback'],
  ['169.254.0.0/16', 'link-local — cloud instance metadata lives here'],
  ['172.16.0.0/12', 'private network (RFC1918)'],
  ['192.0.0.0/24', 'IETF protocol assignments'],
  ['192.0.2.0/24', 'documentation range'],
  ['192.88.99.0/24', '6to4 relay anycast'],
  ['192.168.0.0/16', 'private network (RFC1918)'],
  ['198.18.0.0/15', 'benchmarking range'],
  ['198.51.100.0/24', 'documentation range'],
  ['203.0.113.0/24', 'documentation range'],
  ['224.0.0.0/4', 'multicast'],
  ['240.0.0.0/4', 'reserved'],
  ['::/128', 'unspecified'],
  ['::1/128', 'IPv6 loopback'],
  ['fc00::/7', 'IPv6 unique-local (ULA)'],
  ['fe80::/10', 'IPv6 link-local'],
  ['ff00::/8', 'IPv6 multicast'],
  ['2001:db8::/32', 'documentation range'],
]

const BLOCKED_NETS: Array<{ net: Cidr; why: string }> = BLOCKED.map(([cidr, why]) => {
  const net = parseCidr(cidr)
  if (!net) throw new Error(`safe-fetch: bad built-in CIDR ${cidr}`)
  return { net, why }
})

/** Why this address is refused, or null when it looks like public internet. */
export function blockedAddressReason(ip: string): string | null {
  const bytes = ipBytes(ip)
  if (!bytes) return 'unparseable address'
  for (const { net, why } of BLOCKED_NETS) if (inCidr(bytes, net)) return why
  return null
}

// Names that mean "inside", by convention or by RFC.
const BLOCKED_SUFFIXES = ['.internal', '.local', '.localhost', '.home.arpa', '.intranet', '.lan']
const BLOCKED_NAMES = ['localhost', 'ip6-localhost', 'ip6-loopback']

// ── Operator allowlist ──────────────────────────────────────────────────────

let allowCache: { spec: string; hosts: string[]; suffixes: string[]; nets: Cidr[] } | null = null

const allowList = () => {
  const spec = process.env.TALARIA_FETCH_ALLOW_HOSTS ?? ''
  if (allowCache?.spec === spec) return allowCache
  const hosts: string[] = []
  const suffixes: string[] = []
  const nets: Cidr[] = []
  for (const raw of spec.split(/[,\s]+/)) {
    const entry = raw.trim().toLowerCase()
    if (!entry) continue
    if (entry.startsWith('*.')) {
      suffixes.push(entry.slice(1)) // ".corp.example"
      hosts.push(entry.slice(2))
      continue
    }
    const net = entry.includes('/') ? parseCidr(entry) : null
    if (net) {
      nets.push(net)
      continue
    }
    const bare = ipBytes(entry)
    if (bare) nets.push({ bytes: bare, bits: bare.length * 8 })
    else hosts.push(entry)
  }
  allowCache = { spec, hosts, suffixes, nets }
  return allowCache
}

/** An allowlisted hostname skips address checking entirely — the operator has
 *  said this specific name is theirs. */
export function hostAllowed(hostname: string): boolean {
  const h = hostname.toLowerCase().replace(/\.$/, '')
  const { hosts, suffixes } = allowList()
  return hosts.includes(h) || suffixes.some((s) => h.endsWith(s))
}

const addressAllowed = (ip: string): boolean => {
  const bytes = ipBytes(ip)
  if (!bytes) return false
  return allowList().nets.some((n) => inCidr(bytes, n))
}

// ── URL validation ──────────────────────────────────────────────────────────

/** Throws BlockedUrlError unless this URL is http(s) and lands on a public
 *  address (or somewhere the operator allowlisted). Exported for tests and for
 *  callers that want to validate a stored URL without fetching it. */
export async function assertFetchableUrl(input: string | URL): Promise<URL> {
  let url: URL
  try {
    url = input instanceof URL ? input : new URL(input)
  } catch {
    throw new BlockedUrlError(`not a valid URL: ${String(input).slice(0, 120)}`)
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new BlockedUrlError(`refusing ${url.protocol}// — only http and https are allowed`)
  }
  const host = url.hostname.replace(/^\[/, '').replace(/\]$/, '').replace(/\.$/, '').toLowerCase()
  if (!host) throw new BlockedUrlError('URL has no host')
  if (hostAllowed(host)) return url

  const literal = ipBytes(host)
  if (literal) {
    if (addressAllowed(host)) return url
    const why = blockedAddressReason(host)
    if (why) throw new BlockedUrlError(`refusing ${host} — ${why}`)
    return url
  }
  if (BLOCKED_NAMES.includes(host) || BLOCKED_SUFFIXES.some((s) => host.endsWith(s))) {
    throw new BlockedUrlError(`refusing ${host} — internal-only hostname`)
  }

  let addrs: Array<{ address: string }>
  try {
    addrs = await dnsLookup(host, { all: true, verbatim: true })
  } catch {
    throw new BlockedUrlError(`could not resolve ${host}`)
  }
  if (addrs.length === 0) throw new BlockedUrlError(`could not resolve ${host}`)
  for (const { address } of addrs) {
    if (addressAllowed(address)) continue
    const why = blockedAddressReason(address)
    if (why) throw new BlockedUrlError(`refusing ${host} — it resolves to ${address} (${why})`)
  }
  return url
}

// ── Connect-time re-validation (closes most of the rebinding window) ─────────

type LookupCallback = (err: NodeJS.ErrnoException | null, address?: unknown, family?: number) => void

const validatingLookup = (hostname: string, options: object, cb: LookupCallback): void => {
  dnsLookupCb(hostname, options as { all: true }, (err, addresses) => {
    if (err) return cb(err)
    const list: string[] = Array.isArray(addresses)
      ? addresses.map((a) => a.address)
      : [String(addresses as unknown as string)]
    if (!hostAllowed(hostname)) {
      for (const ip of list) {
        if (addressAllowed(ip)) continue
        const why = blockedAddressReason(ip)
        if (why) {
          return cb(Object.assign(new BlockedUrlError(`refusing ${hostname} — it resolves to ${ip} (${why})`), { code: 'EBLOCKED' }))
        }
      }
    }
    cb(null, addresses, (addresses as unknown as { family?: number })?.family)
  })
}

let dispatcherOnce: Promise<object | null> | null = null

/** An undici Agent that re-checks the address at CONNECT time, when one is
 *  available. Optional by design: undici currently reaches us transitively, so
 *  this must degrade to plain fetch rather than break every outbound call. */
const pinnedDispatcher = (): Promise<object | null> =>
  (dispatcherOnce ??= (async () => {
    try {
      // Specifier kept out of static analysis so bundlers don't hard-require a
      // package we treat as optional.
      const spec = 'undici'
      const mod = (await import(/* @vite-ignore */ spec)) as { Agent?: new (opts: unknown) => object }
      if (!mod.Agent) return null
      return new mod.Agent({ connect: { lookup: validatingLookup } })
    } catch {
      return null
    }
  })())

// ── The fetch ───────────────────────────────────────────────────────────────

export interface SafeFetchInit extends Omit<RequestInit, 'redirect' | 'signal'> {
  /** Whole-request budget, redirects included. Default 15s. */
  timeoutMs?: number
  /** Response body cap; a larger body (or Content-Length) is refused. Default 5MB. */
  maxBytes?: number
  /** Default 5. */
  maxRedirects?: number
  signal?: AbortSignal | null
}

const REDIRECTS = new Set([301, 302, 303, 307, 308])
const NULL_BODY = new Set([101, 204, 205, 304])
// Anything that could carry a credential gets dropped when the redirect leaves
// the origin we validated it for.
const CREDENTIAL_HEADER = /^(authorization|cookie|proxy-authorization)$|(key|token|secret|auth|credential)/i

async function capBody(res: Response, maxBytes: number): Promise<Response> {
  const declared = Number(res.headers.get('content-length'))
  if (Number.isFinite(declared) && declared > maxBytes) {
    await res.body?.cancel().catch(() => {})
    throw new BlockedUrlError(`response declares ${declared} bytes, over the ${maxBytes}-byte cap`)
  }
  if (!res.body || NULL_BODY.has(res.status)) return res
  const reader = res.body.getReader()
  const chunks: Uint8Array[] = []
  let total = 0
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    total += value.byteLength
    if (total > maxBytes) {
      await reader.cancel().catch(() => {})
      throw new BlockedUrlError(`response exceeded the ${maxBytes}-byte cap`)
    }
    chunks.push(value)
  }
  const buf = new Uint8Array(total)
  let at = 0
  for (const c of chunks) {
    buf.set(c, at)
    at += c.byteLength
  }
  const headers = new Headers(res.headers)
  headers.delete('content-encoding') // body is already decoded and re-length'd
  headers.delete('content-length')
  return new Response(buf, { status: res.status, statusText: res.statusText, headers })
}

/**
 * fetch() for URLs Talaria does not control. Same shape as fetch, minus
 * `redirect` (we always drive the chain) — every hop is re-validated, the
 * whole thing is time-bounded, and the body is size-capped.
 *
 * Throws BlockedUrlError for anything refused; network errors surface as
 * normal fetch errors. Bodies must be replayable (string / buffer /
 * URLSearchParams), because a redirect re-sends them.
 */
export async function safeFetch(input: string | URL, init: SafeFetchInit = {}): Promise<Response> {
  const { timeoutMs = 15_000, maxBytes = 5 * 1024 * 1024, maxRedirects = 5, signal, ...rest } = init
  const deadline = AbortSignal.timeout(timeoutMs)
  const abort = signal ? AbortSignal.any([signal, deadline]) : deadline

  let target = await assertFetchableUrl(input)
  let method = (rest.method ?? 'GET').toUpperCase()
  let body = rest.body
  const headers = new Headers(rest.headers as HeadersInit | undefined)
  const dispatcher = await pinnedDispatcher()

  for (let hop = 0; ; hop++) {
    const res = await fetch(target, {
      ...rest,
      method,
      body,
      headers,
      redirect: 'manual',
      signal: abort,
      ...(dispatcher ? { dispatcher } : {}),
    } as RequestInit)

    const location = REDIRECTS.has(res.status) ? res.headers.get('location') : null
    if (!location) return await capBody(res, maxBytes)
    if (hop >= maxRedirects) {
      await res.body?.cancel().catch(() => {})
      throw new BlockedUrlError(`too many redirects (over ${maxRedirects})`)
    }
    await res.body?.cancel().catch(() => {})

    // THE point of driving redirects by hand: the new target is validated
    // before we dial it, so a public host cannot bounce us onto the metadata
    // service or a loopback admin port.
    const next = await assertFetchableUrl(new URL(location, target))
    if (next.origin !== target.origin) {
      for (const name of [...headers.keys()]) if (CREDENTIAL_HEADER.test(name)) headers.delete(name)
    }
    if (res.status === 303 || ((res.status === 301 || res.status === 302) && method === 'POST')) {
      method = 'GET'
      body = undefined
      headers.delete('content-type')
      headers.delete('content-length')
    }
    target = next
  }
}
