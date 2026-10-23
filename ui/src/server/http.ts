// The one HTTP helper every API route uses. Formerly `json` from
// '@tanstack/react-start'; same signature, no framework underneath.
export function json(data: unknown, init?: ResponseInit): Response {
  const headers = new Headers(init?.headers)
  if (!headers.has('content-type')) headers.set('content-type', 'application/json')
  return new Response(JSON.stringify(data), { ...init, headers })
}

/** A fetch Response's headers as a plain object for `res.writeHead()` — the
 *  conversion both HTTP wrappers (vite dev middleware, server-entry.ts) run at
 *  their boundary. Exists because the obvious one-liner is wrong:
 *  `Object.fromEntries(response.headers.entries())` collapses duplicate keys,
 *  and Set-Cookie is the one header that legitimately repeats. The Google
 *  login callback answers with TWO cookies — session on, one-shot state off —
 *  so the object kept only the last (the state clear) and the session cookie
 *  died at this hop on every deployment: Google authorized, the SPA booted at
 *  `/`, `/api/auth/session` said `{user: null}`, and the cockpit bounced back
 *  to /login. Password login sets one cookie and never noticed. An array is
 *  the shape writeHead wants for repeats — node and bun both emit one
 *  Set-Cookie line per element. */
export function writeHeadHeaders(response: Response): Record<string, string | string[]> {
  const out: Record<string, string | string[]> = Object.fromEntries(response.headers.entries())
  const cookies = response.headers.getSetCookie()
  if (cookies.length > 0) out['set-cookie'] = cookies
  return out
}

/** THE HOST'S OWN STATEMENT OF WHO IS ON THE OTHER END OF THE SOCKET, for the
 *  routes that answer only a caller on this machine (the api's calls into
 *  /api/app-host/harnesses). A route sees a fetch `Request`, which carries no
 *  peer address, so the two HTTP wrappers (vite dev middleware, server-entry)
 *  write it as a header at their boundary — and OVERWRITE whatever a client
 *  sent under the same name, so the value is never the caller's claim.
 *  `loopback` or `remote`; absent (a bundle older than the wrapper, a test)
 *  reads as remote, so a missing stamp refuses rather than admits. */
export const PEER_HEADER = 'x-talaria-peer'

/** Is `address` (node's `socket.remoteAddress`) this machine? 127/8, ::1 and
 *  the v4-mapped form node reports for a v4 caller on a dual-stack socket. */
export function isLoopbackAddress(address: string | undefined): boolean {
  if (!address) return false
  const a = address.toLowerCase()
  if (a === '::1') return true
  const v4 = a.startsWith('::ffff:') ? a.slice('::ffff:'.length) : a
  return /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(v4)
}

/** Stamp `headers` (built from the client's own) with the peer, replacing any
 *  value the client sent. */
export function stampPeer(headers: Headers, remoteAddress: string | undefined): void {
  headers.set(PEER_HEADER, isLoopbackAddress(remoteAddress) ? 'loopback' : 'remote')
}

/** Did the wrapper see this request arrive from this machine? */
export const fromLoopback = (request: Request): boolean => request.headers.get(PEER_HEADER) === 'loopback'
