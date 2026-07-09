import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { proxyChat } from '@/server/gateway'
import { routedModelFor } from '@/server/fleet-agents'
import { getSessionUser } from '@/server/auth/session'
import { canUseAgentModel } from '@/server/users'
import {
  activeStreamingAssistant,
  createConversation,
  insertStreamingAssistant,
  insertUserMessage,
  nextSeq,
  ownedConversation,
  priorMessages,
  touchConversation,
} from '@/server/conversations'
import { continueConversation, persistAssistantStream } from '@/server/chat-persist'
import { attachmentAsDataUrl, resolveAttachments, isImage } from '@/server/uploads'
import { notifyPlanMentions } from '@/server/plan-doc'
import { indexActivity } from '@/server/retrieval/sources'

const Body = z.object({
  model: z.string().min(1),
  conversationId: z.string().uuid().optional(),
  // Content may be empty when the turn is attachments-only.
  content: z.string().max(100_000).default(''),
  /** Model tier (alias name) to route this turn to; omit for the main model. */
  tier: z.string().max(60).optional(),
  attachmentIds: z.array(z.string().uuid()).max(10).optional(),
  /** New conversations are 'chat' unless started from the Plan surface. */
  kind: z.enum(['chat', 'plan']).optional(),
  /** Sent while a reply streams: queue into history, never interrupt. */
  queue: z.boolean().optional(),
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
        let kind: 'chat' | 'plan' = parsed.data.kind ?? 'chat'
        let planTitle: string | null = null
        if (convId) {
          const owned = await ownedConversation(user.id, convId)
          if (!owned) return json({ error: 'conversation not found' }, { status: 404 })
          agentModel = owned.agentModel
          kind = owned.kind
          planTitle = owned.title
        }

        // Owner-aware gate: blocks another user from driving someone's personal
        // assistant (which would act as that owner — Google, memory, private soul).
        if (!(await canUseAgentModel(user.id, user.role, agentModel))) {
          return json({ error: 'forbidden: no access to this agent' }, { status: 403 })
        }

        // Tier routing: validate against the agent's defined aliases, then
        // request `<base>-<tier>` — the agent's own gateway resolves the alias.
        const routedModel = await routedModelFor(agentModel, parsed.data.tier)
        if (!routedModel) return json({ error: `unknown tier "${parsed.data.tier}" for ${agentModel}` }, { status: 400 })

        // Validate attachments belong to real uploads before stamping them.
        const attachments = await resolveAttachments(parsed.data.attachmentIds ?? [])
        const title = titleFrom(content || attachments[0]?.filename || 'chat')
        if (!convId) {
          convId = await createConversation(user.id, agentModel, title, kind)
          planTitle = title
        }

        // Claude-style flow: while a reply is streaming, new messages QUEUE
        // into history instead of interrupting — the completing turn (or the
        // continuation below, if the stream just ended) picks them up.
        const inFlight = await activeStreamingAssistant(convId)
        const queued = inFlight !== null || parsed.data.queue === true

        // Record this turn (history is built AFTER for normal turns, so the
        // new message isn't duplicated into the prior list).
        const prior = queued ? [] : await priorMessages(convId)
        const userSeq = await nextSeq(convId)
        const userMsgId = await insertUserMessage(convId, userSeq, content, attachments)
        await touchConversation(convId, title)

        // Plan turns feed the ambient activity brain (owner-scoped) and notify
        // @mentioned teammates who can read the plan's document. Never blocking.
        const senderLabel = user.name ?? user.email ?? 'someone'
        const planMeta = kind === 'plan' ? { ownerUserId: user.id, title: planTitle } : null
        if (kind === 'plan' && content.trim()) {
          void indexActivity({
            sourceType: 'plan',
            sourceId: userMsgId,
            title: `Plan (${planTitle || 'Untitled'}) · ${senderLabel}`,
            text: content,
            payload: { planId: convId, planOwnerId: user.id },
            href: '/plan',
          }).catch(() => {})
          void notifyPlanMentions(convId, { id: user.id, label: senderLabel }, content, planTitle).catch(() => {})
        }

        if (queued) {
          // If the stream ended between the client's view and this landing,
          // nothing would pick the message up — chain the next turn ourselves.
          if (!inFlight) {
            void continueConversation(convId, { agentModel, tier: parsed.data.tier ?? null, plan: planMeta }).catch(() => {})
          }
          return json({ queued: true, conversationId: convId }, { status: 202 })
        }

        // Give a vision-capable model the actual images (data URLs work even
        // when the agent can't reach Talaria over the network). Text-only
        // history stays plain strings.
        const imageUrls = (
          await Promise.all(attachments.filter((a) => isImage(a.mime)).map((a) => attachmentAsDataUrl(a.id)))
        ).filter((u): u is string => !!u)
        const userContent =
          imageUrls.length > 0
            ? [
                ...(content ? [{ type: 'text', text: content }] : []),
                ...imageUrls.map((url) => ({ type: 'image_url', image_url: { url } })),
              ]
            : content
        const messages = [...prior, { role: 'user' as const, content: userContent as unknown as string }]
        const assistantId = await insertStreamingAssistant(convId, userSeq + 1)

        const upstream = await proxyChat({ model: routedModel, messages })
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
        void persistAssistantStream(toStore, assistantId, convId, {
          agentModel,
          promptChars,
          tier: parsed.data.tier ?? null,
          plan: planMeta,
        })
        return new Response(toClient, { status: upstream.status, headers })
      },
    },
  },
})
