/** Only public, dotted hostnames can plausibly serve a favicon. Internal
 *  endpoints — IP literals (the built-in toolkit's local gateway), single-label
 *  hosts from pseudo-URLs like `talaria-workbench://core`, localhost and
 *  friends — would only ever 404 the icon proxy, so callers skip the request
 *  entirely and fall straight to the monogram tile (spec §8: raised tile +
 *  mono initial). Shared by the client (don't ask) and the server resolver
 *  (never dial internal hosts). */
export function publicIconDomain(domain: string | null | undefined): string | null {
  const d = domain?.trim().toLowerCase()
  if (!d || !d.includes('.')) return null
  if (d === 'localhost' || d.endsWith('.localhost') || d.endsWith('.local') || d.endsWith('.internal')) return null
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(d)) return null // IPv4 literal
  if (d.startsWith('[') || d.includes(':')) return null // IPv6 literal
  return d
}
