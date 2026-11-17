/** A Google connect bounce as the landing page's flash reads it: the status
 *  the callback sent back (`?google=`, `?googleOrg=`, `?googleAgent=`) and,
 *  when Google said why, `&reason=` — Google's own OAuth error code
 *  (`invalid_client`, `access_denied`). The page quotes the code; it never
 *  rewrites it into a guess at what went wrong.
 *
 *  Only a bare snake_case code is shown. The bounce is a URL anyone can
 *  craft, so free text in it is never rendered — the server sends nothing
 *  else, and this drops anything else. */

export interface GoogleBounce {
  status: string
  reason: string | null
}

const OAUTH_CODE = /^[a-z_]{1,64}$/

/** The bounce in `search` under `param`, or null when there is none. */
export function readGoogleBounce(search: string, param: string): GoogleBounce | null {
  const q = new URLSearchParams(search)
  const status = q.get(param)
  if (!status) return null
  const reason = q.get('reason')
  return { status, reason: reason && OAUTH_CODE.test(reason) ? reason : null }
}

/** What Google said, as a clause to put after the flash sentence. */
export function googleSaid(reason: string | null): string {
  return reason ? `Google said: ${reason}.` : ''
}

/** The flash sentence for a bounce: the page's own text for the status, then
 *  Google's code when there is one. */
export function googleBounceText(bounce: GoogleBounce, texts: Record<string, string>): string {
  const base = texts[bounce.status] ?? bounce.status
  return bounce.reason ? `${base} ${googleSaid(bounce.reason)}` : base
}
