import { defineApi } from '@/server/api-route'
import { parseBody } from '@/server/api-guard'
import { json } from '@/server/http'
import { z } from 'zod'
import { refuseLegacy, requireAgent } from '@/server/agent-auth'
import { resolveAgentGoogle } from '@/server/google/agent-google'
import { createLabelWithToken, listLabelsWithToken } from '@/server/google/gmail'
import { googleFail } from '@/server/google/errors'

const Body = z.object({ name: z.string().min(1).max(60) })

// The label half of inbox organizing. Gmail's folders ARE labels: INBOX and
// UNREAD are system labels a message carries, and "filing" mail means applying
// and removing them (see the organize route for the mutations).
// GET  → every label (read)
// POST → find-or-create a label (safe to retry)
export const Route = defineApi('/api/integrations/google/agent/gmail/labels', {
  GET: async ({ request }) => {
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    // The label list sketches the mailbox's whole filing scheme — a legacy
    // shared-key caller only ASSERTS which agent it is, so it never sees one.
    const denied = refuseLegacy(caller, 'Gmail access')
    if (denied) return denied
    const google = await resolveAgentGoogle(caller.model, Date.now())
    if (!google) return json({ error: 'not_connected', message: 'No Google account is connected for this agent (its owner, or the org account).' }, { status: 409 })
    try {
      return json({ labels: await listLabelsWithToken(google.token) })
    } catch (err) {
      return googleFail(err as Error, 'Gmail')
    }
  },
  POST: async ({ request }) => {
    const caller = await requireAgent(request)
    if (caller instanceof Response) return caller
    // Creating a label rewrites the mailbox's filing scheme — same proof bar.
    const denied = refuseLegacy(caller, 'Gmail access')
    if (denied) return denied
    const google = await resolveAgentGoogle(caller.model, Date.now())
    if (!google) return json({ error: 'not_connected', message: 'No Google account is connected for this agent (its owner, or the org account).' }, { status: 409 })
    const parsed = await parseBody(request, Body)
    if (parsed instanceof Response) return parsed
    try {
      return json({ label: await createLabelWithToken(google.token, parsed.name) })
    } catch (err) {
      return googleFail(err as Error, 'Gmail')
    }
  },
})
