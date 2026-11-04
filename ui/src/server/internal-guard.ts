// The other direction of internal-api.ts: routes on THIS host that only the
// api may call. The api presents the same root-derived service credential
// (`internalToken`, secretbox.ts — `SecretRoot::internal_token` on its side)
// in `x-talaria-internal`, and it calls over loopback, which the HTTP wrapper
// states as `x-talaria-peer` (http.ts PEER_HEADER). Both must hold: the token
// is the proof, and loopback keeps the route off every network interface even
// for a caller that somehow held it.
//
// Today's callers: the fitness plane, reading the enabled apps' harnesses and
// replaying their fixtures (routes/api/app-host.harnesses*.ts).
import { timingSafeEqual } from 'node:crypto'
import { fromLoopback, json } from './http'
import { INTERNAL_HEADER } from './internal-api'
import { internalToken } from './secretbox'

const refuse = (why: string): Response => json({ error: `internal route: ${why}` }, { status: 403 })

/** Whether `presented` is this host's internal token, in constant time. No
 *  root configured admits nothing. */
export function admitsInternal(presented: string): boolean {
  let want: string
  try {
    want = internalToken()
  } catch {
    return false
  }
  const a = Buffer.from(want)
  const b = Buffer.from(presented.trim())
  return a.length === b.length && timingSafeEqual(a, b)
}

/** The api on this machine, or the 403 to answer with. */
export function requireInternal(request: Request): Response | { ok: true } {
  if (!fromLoopback(request)) return refuse('only a caller on this machine may call it')
  if (!admitsInternal(request.headers.get(INTERNAL_HEADER) ?? '')) return refuse('only the api may call it')
  return { ok: true }
}
