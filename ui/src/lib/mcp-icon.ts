import { publicIconDomain } from '@/lib/icon-domain'

/** What a server row knows about where its mark might live. */
export interface IconSource {
  /** The publisher's own domain, when the row carries one: a marketplace
   *  entry's `domain`, or the provider an OAuth server signs in with. */
  publisher?: string | null
  /** The server's address. A pseudo-scheme (`talaria-pkg://x`) has no host. */
  url?: string | null
  /** The address's host, for a row that carries that and not the address. */
  host?: string | null
}

/** API hosts whose mark lives on the brand's own domain. The api keeps the
 *  same fact for OAuth (`CROSS_DOMAIN_ISSUERS` in talaria-mcp-oauth). */
const BRAND_HOME: Record<string, string> = {
  'githubcopilot.com': 'github.com',
  'googleapis.com': 'google.com',
}

const MULTI_LABEL_SUFFIXES = ['co.uk', 'com.au', 'co.jp', 'co.nz', 'com.br', 'co.in']

/** The domain a company registers: `drivemcp.googleapis.com` is
 *  `googleapis.com`. The api's rule (`registrable` in talaria-mcp-oauth),
 *  so both sides name the same parent. */
export function registrableDomain(host: string): string {
  const labels = host.toLowerCase().replace(/\.$/, '').split('.')
  if (labels.length <= 2) return labels.join('.')
  const last2 = labels.slice(-2).join('.')
  return MULTI_LABEL_SUFFIXES.includes(last2) ? labels.slice(-3).join('.') : last2
}

/** The host of a real web address. A pseudo-scheme such as
 *  talaria-workbench://core carries a routing token, not a host. */
export function addressHost(url: string | null | undefined): string | null {
  if (!url) return null
  try {
    const u = new URL(url)
    return u.protocol === 'http:' || u.protocol === 'https:' ? u.hostname : null
  } catch {
    return null
  }
}

/** The hosts to ask the icon proxy for, best first. The publisher's domain
 *  when the row knows it, then the address host, then the domain that host
 *  is registered under. Empty means there is nothing to ask: draw the letter. */
export function iconDomains(source: IconSource): string[] {
  // `mcp.` names the protocol, not the product: mcp.hubspot.com serves a 16px
  // favicon where hubspot.com serves the brand's full mark.
  const host = publicIconDomain(source.host ?? addressHost(source.url))?.replace(/^mcp\./, '') ?? null
  const parent = host ? registrableDomain(host) : null
  const publisher = publicIconDomain(source.publisher) ?? (parent ? (BRAND_HOME[parent] ?? null) : null)
  return [...new Set([publisher, host, parent].filter((d): d is string => !!d))]
}

/** Two letters for a server that has no mark of its own: the first letters
 *  of the first two words of its name, or the first two of a one-word name.
 *  Punctuation ahead of a word is not a letter ("(your account)"). */
export function serverMonogram(name: string): string {
  const words = name
    .split(/\s+/)
    .map((w) => w.replace(/^[^\p{L}\p{N}]+/u, ''))
    .filter(Boolean)
  const [first, second] = words
  if (!first) return '?'
  const letters = second ? `${[...first][0] ?? ''}${[...second][0] ?? ''}` : [...first].slice(0, 2).join('')
  return letters.toUpperCase()
}
