import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
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
        const user = await requireUser(request)
        if (user instanceof Response) return user
        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        try {
          return await briefingChat(user.id, body.scope as BriefingScope, body.history, body.content)
        } catch (e) {
          console.error('[me.briefing]', e)
          return json({ error: 'briefing failed — see server logs' }, { status: 500 })
        }
      },
    },
  },
})
