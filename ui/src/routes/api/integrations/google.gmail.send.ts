import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
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
export const Route = createFileRoute('/api/integrations/google/gmail/send')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          return json({ sent: await sendMessage(user.id, Date.now(), parsed.data) })
        } catch (err) {
          return googleFail(err as Error, 'Gmail')
        }
      },
    },
  },
})
