import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { refuseLegacy, requireAgent } from '@/server/agent-auth'
import { resolveAgentGoogle } from '@/server/google/agent-google'
import { getMessageWithToken } from '@/server/google/gmail'
import { googleFail } from '@/server/google/errors'

// GET /api/integrations/google/agent/gmail/$id → one FULL message (headers +
// plain-text body) for the calling agent. The listing tool hands out ids and
// snippets; this is the read an actual answer needs — summarize the thread,
// quote the question, draft the informed reply. Reads are free (confirm-sends
// govern the outbound half only).
export const Route = defineApi('/api/integrations/google/agent/gmail/$id', {
  GET: async ({ request, params }) => {
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    // Acting as a HUMAN — the owner's mailbox (or the shared org one). A
    // legacy shared-key caller only ASSERTS which agent it is, so it never
    // reaches a token.
    const denied = refuseLegacy(caller, 'Gmail access')
    if (denied) return denied
    const google = await resolveAgentGoogle(caller.model, Date.now())
    if (!google) return json({ error: 'not_connected', message: 'No Google account is connected for this agent (its owner, or the org account).' }, { status: 409 })
    if (!/^[A-Za-z0-9_-]{4,64}$/.test(params.id)) return json({ error: 'bad request' }, { status: 400 })
    try {
      return json({ message: await getMessageWithToken(google.token, params.id) })
    } catch (err) {
      return googleFail(err as Error, 'Gmail')
    }
  },
})
