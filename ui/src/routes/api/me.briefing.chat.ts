import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { briefingChat, type BriefingScope } from '@/server/briefing'

const Body = z.object({
  scope: z.enum(['inbox', 'boards', 'comms', 'plans', 'research']),
  content: z.string().min(1).max(4000),
  history: z.array(z.object({ role: z.enum(['user', 'assistant']), content: z.string().max(20_000) })).max(12).default([]),
})

// POST → an EPHEMERAL follow-up on a briefing: streams the assistant's reply
// (SSE passthrough) with the briefing as context; nothing is persisted.
export const Route = createFileRoute('/api/me/briefing/chat')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        try {
          return await briefingChat(user.id, parsed.data.scope as BriefingScope, parsed.data.history, parsed.data.content)
        } catch (e) {
          return json({ error: (e as Error).message }, { status: 500 })
        }
      },
    },
  },
})
