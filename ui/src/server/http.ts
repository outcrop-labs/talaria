// The one HTTP helper every API route uses. Formerly `json` from
// '@tanstack/react-start'; same signature, no framework underneath.
export function json(data: unknown, init?: ResponseInit): Response {
  const headers = new Headers(init?.headers)
  if (!headers.has('content-type')) headers.set('content-type', 'application/json')
  return new Response(JSON.stringify(data), { ...init, headers })
}

/** A fetch Response's headers as a plain object for `res.writeHead()` — the
 *  conversion both HTTP wrappers (vite dev middleware, server-entry.js) run at
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
