import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { proxyChat } from '@/server/gateway'
import { getSessionUser } from '@/server/auth/session'
import { allowedAgents, canUseAgent } from '@/server/users'
import {
  createConversation,
  insertStreamingAssistant,
  insertUserMessage,
  nextSeq,
  ownedConversationModel,
  priorMessages,
  touchConversation,
} from '@/server/conversations'
import { persistAssistantStream } from '@/server/chat-persist'

const Body = z.object({
  model: z.string().min(1),
  conversationId: z.string().uuid().optional(),
  content: z.string().min(1).max(100_000),
})

const titleFrom = (s: string) => s.replace(/\s+/g, ' ').trim().slice(0, 80)

// POST /api/chat { model, conversationId?, content } → durable streaming chat.
// Persists the turn to Postgres (server owns history) and tees the gateway
// stream: one branch to the client, one drained to the DB so an in-progress
// reply survives a disconnect. Returns SSE + X-Conversation-Id / X-Message-Id.
export const Route = createFileRoute('/api/chat')({
  server: {
    handlers: {
      POST: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })

        const parsed = Body.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const { model, conversationId, content } = parsed.data

        // Resolve the conversation (ownership-checked) or create a new one.
        let convId = conversationId ?? null
        let agentModel = model
        if (convId) {
          const owned = await ownedConversationModel(user.id, convId)
          if (!owned) return json({ error: 'conversation not found' }, { status: 404 })
          agentModel = owned
        }

        const access = await allowedAgents(user.id, user.role)
        if (!canUseAgent(access, agentModel)) {
          return json({ error: 'forbidden: no access to this agent' }, { status: 403 })
        }

        if (!convId) convId = await createConversation(user.id, agentModel, titleFrom(content))

        // Build gateway history from the DB (prior turns), then record this turn.
        const prior = await priorMessages(convId)
        const userSeq = await nextSeq(convId)
        await insertUserMessage(convId, userSeq, content)
        await touchConversation(convId, titleFrom(content))
        const messages = [...prior, { role: 'user' as const, content }]
        const assistantId = await insertStreamingAssistant(convId, userSeq + 1)

        const upstream = await proxyChat({ model: agentModel, messages })
        const headers = new Headers({
          'Content-Type': upstream.headers.get('content-type') ?? 'text/event-stream',
          'Cache-Control': 'no-cache',
          'X-Conversation-Id': convId,
          'X-Message-Id': assistantId,
        })

        if (!upstream.body) return new Response(upstream.body, { status: upstream.status, headers })

        // Tee: one branch relays to the client, one persists server-side
        // (detached, so it completes even if the client disconnects).
        const [toClient, toStore] = upstream.body.tee()
        const promptChars = messages.reduce((n, m) => n + m.content.length, 0)
        void persistAssistantStream(toStore, assistantId, convId, { agentModel, promptChars })
        return new Response(toClient, { status: upstream.status, headers })
      },
    },
  },
})
