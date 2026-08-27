import { defineApi } from '@/server/api-route'
import { parseBody, requireUser } from '@/server/api-guard'
import { json } from '@/server/http'
import { z } from 'zod'
import { sendMessage } from '@/server/google/gmail'
import { googleFail } from '@/server/google/errors'

const Body = z.object({
  to: z.string().min(3).max(500),
  subject: z.string().max(500).default(''),
  body: z.string().max(50_000).default(''),
  cc: z.string().max(500).optional(),
  bcc: z.string().max(500).optional(),
})

// POST /api/integrations/google/gmail/send → send a plain-text email as the user.
export const Route = defineApi('/api/integrations/google/gmail/send', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const parsed = await parseBody(request, Body)
    if (parsed instanceof Response) return parsed
    try {
      return json({ sent: await sendMessage(user.id, Date.now(), parsed) })
    } catch (err) {
      return googleFail(err as Error, 'Gmail')
    }
  },
})
