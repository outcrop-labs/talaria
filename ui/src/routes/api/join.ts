import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { clientIp, rateLimit } from '@/server/rate-limit'
import { inviteByToken } from '@/server/invites'

// Public invite lookup for the /join page: token → who's invited, by whom,
// to which org. Expired/revoked/accepted tokens read as gone.
//
// Unauthenticated by design — which makes it an oracle that names the org and
// the invited address for every VALID token — so both axes are braked, the
// same dual-counter shape as the login route. (Tokens are 24 random bytes;
// guessing one is not the threat. Hammering is.)
const IP_LIMIT = 120 // page loads per window per client — generous: a NAT'd office shares one counter
const TOKEN_LIMIT = 20 // lookups against any one token
const WINDOW_SECONDS = 15 * 60
// doc: Public invite lookup for the /join page: token → who's invited, by whom,
// doc: to which org. Rate-limited; expired/revoked/accepted tokens read as gone.


export const Route = defineApi('/api/join', {
  GET: async ({ request }) => {
    const token = new URL(request.url).searchParams.get('token') ?? ''
    if (!token) return json({ error: 'missing token' }, { status: 400 })
    const [byIp, byToken] = await Promise.all([
      rateLimit(`join:ip:${clientIp(request)}`, IP_LIMIT, WINDOW_SECONDS),
      rateLimit(`join:token:${token}`, TOKEN_LIMIT, WINDOW_SECONDS),
    ])
    const limited = !byIp.ok ? byIp : !byToken.ok ? byToken : null
    if (limited) {
      return json({ error: 'too many attempts, try again shortly' }, { status: 429, headers: { 'Retry-After': String(limited.retryAfter) } })
    }
    const invite = await inviteByToken(token)
    if (!invite) return json({ error: 'invite not found or no longer valid' }, { status: 404 })
    return json({ invite })
  },
})
