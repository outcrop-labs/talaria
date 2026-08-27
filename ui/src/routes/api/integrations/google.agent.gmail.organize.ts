import { defineApi } from '@/server/api-route'
import { parseBody } from '@/server/api-guard'
import { json } from '@/server/http'
import { z } from 'zod'
import { refuseLegacy, requireAgent } from '@/server/agent-auth'
import { resolveAgentGoogle } from '@/server/google/agent-google'
import { organizeEmailsWithToken } from '@/server/google/gmail'
import { googleFail } from '@/server/google/errors'

const Body = z.object({
  ids: z.array(z.string().min(1).max(128)).min(1).max(100),
  addLabels: z.array(z.string().min(1).max(120)).max(10).optional(),
  removeLabels: z.array(z.string().min(1).max(120)).max(10).optional(),
})

// POST → file/archive/read messages by label. THE HITL LINE, stated once
// because it is the one judgment this route makes: sends and invites leave the
// building under the owner's identity and queue for approval; filing, archiving
// and mark-read stay INSIDE the mailbox and are reversible, so they apply
// immediately — that immediacy is the feature ("clean up my inbox" that takes
// fifty approval cards is not cleanup). Destructive labels (TRASH/SPAM) are
// refused in the service layer: nothing this route can do removes mail from
// All Mail, which is what keeps the immediacy honest.
export const Route = defineApi('/api/integrations/google/agent/gmail/organize', {
  POST: async ({ request }) => {
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    // Mutating the owner's mailbox — a legacy shared-key caller only ASSERTS
    // which agent it is, so it never reaches the token.
    const denied = refuseLegacy(caller, 'Gmail access')
    if (denied) return denied
    const google = await resolveAgentGoogle(caller.model, Date.now())
    if (!google) return json({ error: 'not_connected', message: 'No Google account is connected for this agent (its owner, or the org account).' }, { status: 409 })
    const parsed = await parseBody(request, Body)
    if (parsed instanceof Response) return parsed
    try {
      const { updated } = await organizeEmailsWithToken(google.token, parsed)
      return json({
        updated,
        note: 'filed — labels applied and archived mail stays in All Mail; nothing was deleted or sent',
      })
    } catch (err) {
      // An unknown label is a routing mistake by the caller, not a Google
      // outage: answer it as a 400 the agent can act on (create_label), not a
      // 502 it can only relay.
      const msg = (err as Error).message
      if (msg.startsWith('gmail organize: ')) return json({ error: 'bad request', message: msg }, { status: 400 })
      return googleFail(err as Error, 'Gmail')
    }
  },
})
