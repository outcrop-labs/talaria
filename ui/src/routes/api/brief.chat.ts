import { z } from 'zod'
import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { parseBody, requireUser } from '@/server/api-guard'
import { briefChat } from '@/server/daily-brief-chat'

const Body = z.object({
  content: z.string().min(1).max(4_000),
  history: z
    .array(z.object({ role: z.enum(['user', 'assistant']), content: z.string().max(8_000) }))
    .max(20)
    .default([]),
  /** Ask about ONE line of the brief rather than the day as a whole. */
  sourceKey: z.string().max(200).nullable().default(null),
})

export const Route = defineApi('/api/brief/chat', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    try {
      return await briefChat(user, body)
    } catch (e: unknown) {
      // A dead gateway answers 500 with its reason, rather than an empty
      // event-stream the panel would render as the assistant saying nothing.
      return json({ error: e instanceof Error ? e.message : 'the assistant could not be reached' }, { status: 500 })
    }
  },
})
