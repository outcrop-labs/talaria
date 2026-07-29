import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { proxyChat } from '@/server/gateway'
import { routedModelFor } from '@/server/fleet-agents'
import { parseBody, requireUser } from '@/server/api-guard'
import { hasPerm } from '@/server/permissions'
import { canUseAgentModel } from '@/server/users'
import {
  accessibleConversation,
  activeStreamingAssistant,
  createConversation,
  insertStreamingAssistant,
  insertUserMessage,
  listPlanMembers,
  nextSeq,
  priorMessages,
  touchConversation,
} from '@/server/conversations'
import { continueConversation, persistAssistantStream } from '@/server/chat-persist'
import { attachmentAsDataUrl, attachmentTextBlocks, resolveAttachments, isImage } from '@/server/uploads'
import { refBlocks, resolveRefs } from '@/server/refs'
import { notifyPlanMentions, PLAN_MODE_PROMPT } from '@/server/plan-doc'
import { indexActivity } from '@/server/retrieval/sources'

const Body = z.object({
  model: z.string().min(1),
  conversationId: z.string().uuid().optional(),
  // Content may be empty when the turn is attachments-only.
  content: z.string().max(100_000).default(''),
  /** Model tier (alias name) to route this turn to; omit for the main model. */
  tier: z.string().max(60).optional(),
  attachmentIds: z.array(z.string().uuid()).max(10).optional(),
  /** Knowledge docs / artifacts attached as references (ACL-checked). */
  refs: z.array(z.object({ type: z.enum(['kb-doc', 'artifact']), id: z.string().uuid() })).max(3).optional(),
  /** New conversations are 'chat' unless started from the Plan surface. */
  kind: z.enum(['chat', 'plan']).optional(),
  /** Explicit plan template pick (plan surface, new plan only); server ignores
   *  it for chats and for turns on an existing conversation. */
  templateId: z.string().uuid().nullish(),
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
        const user = await requireUser(request)
        if (user instanceof Response) return user

        const body = await parseBody(request, Body)
        if (body instanceof Response) return body
        const { model, conversationId, content } = body

        // Resolve the conversation (access-checked: yours, or a plan you're a
        // member of) or create a new one.
        let convId = conversationId ?? null
        let agentModel = model
        let kind: 'chat' | 'plan' = body.kind ?? 'chat'
        let planTitle: string | null = null
        let planOwnerId = user.id
        let multiVoice = false
        if (convId) {
          const conv = await accessibleConversation(user.id, convId)
          if (!conv) return json({ error: 'conversation not found' }, { status: 404 })
          agentModel = conv.agentModel
          kind = conv.kind
          planTitle = conv.title
          planOwnerId = conv.ownerUserId
          if (kind === 'plan') multiVoice = (await listPlanMembers(convId)).length > 1
        }

        // Owner-aware gate: blocks another user from driving someone's personal
        // assistant (which would act as that owner — Google, memory, private soul).
        if (!(await canUseAgentModel(user.id, user.role, agentModel))) {
          return json({ error: 'forbidden: no access to this agent' }, { status: 403 })
        }

        // Tier routing: validate against the agent's defined aliases, then
        // request `<base>-<tier>` — the agent's own gateway resolves the alias.
        const routedModel = await routedModelFor(agentModel, body.tier)
        if (!routedModel) return json({ error: `unknown tier "${body.tier}" for ${agentModel}` }, { status: 400 })

        // Validate attachments belong to real uploads before stamping them;
        // knowledge/artifact refs resolve to content-carrying chips.
        const uploads = await resolveAttachments(body.attachmentIds ?? [])
        const refChips = await resolveRefs(user, body.refs ?? [])
        const attachments = [...uploads, ...refChips]
        const title = titleFrom(content || attachments[0]?.filename || 'chat')
        if (!convId) {
          if (kind === 'plan' && !(await hasPerm(user, 'plans.create'))) {
            return json({ error: 'no permission to create plans' }, { status: 403 })
          }
          convId = await createConversation(user.id, agentModel, title, kind, body.templateId ?? null)
          planTitle = title
        }

        // Claude-style flow: while a reply is streaming, new messages QUEUE
        // into history instead of interrupting — the completing turn (or the
        // continuation below, if the stream just ended) picks them up.
        const inFlight = await activeStreamingAssistant(convId)
        const queued = inFlight !== null || body.queue === true

        // Record this turn (history is built AFTER for normal turns, so the
        // new message isn't duplicated into the prior list).
        const prior = queued ? [] : await priorMessages(convId)
        const userSeq = await nextSeq(convId)
        const userMsgId = await insertUserMessage(convId, userSeq, content, attachments, user.id)
        await touchConversation(convId, title)

        // Plan turns feed the ambient activity brain (plan-owner-scoped) and
        // notify @mentioned teammates who can read the plan's document.
        const senderLabel = user.name ?? user.email ?? 'someone'
        const planMeta = kind === 'plan' ? { ownerUserId: planOwnerId, title: planTitle } : null
        if (kind === 'plan' && content.trim()) {
          void indexActivity({
            sourceType: 'plan',
            sourceId: userMsgId,
            title: `Plan (${planTitle || 'Untitled'}) · ${senderLabel}`,
            text: content,
            payload: { planId: convId, planOwnerId },
            href: '/plan',
          }).catch(() => {})
          void notifyPlanMentions(convId, { id: user.id, label: senderLabel }, content, planTitle).catch(() => {})
        }

        if (queued) {
          // If the stream ended between the client's view and this landing,
          // nothing would pick the message up — chain the next turn ourselves.
          if (!inFlight) {
            void continueConversation(convId, { agentModel, tier: body.tier ?? null, plan: planMeta }).catch(() => {})
          }
          return json({ queued: true, conversationId: convId }, { status: 202 })
        }

        // Give a vision-capable model the actual images (data URLs work even
        // when the agent can't reach Talaria over the network). Text-only
        // history stays plain strings.
        const imageUrls = (
          await Promise.all(attachments.filter((a) => isImage(a.mime)).map((a) => attachmentAsDataUrl(a.id)))
        ).filter((u): u is string => !!u)
        // Multiplayer plans: prefix this turn with the sender's name (history
        // is already prefixed by priorMessages) so the agent tells voices
        // apart. Attached refs contribute their content blocks.
        const spokenContent = `${multiVoice && content ? `${senderLabel}: ${content}` : content}${refBlocks(refChips)}${await attachmentTextBlocks(uploads)}`
        const userContent =
          imageUrls.length > 0
            ? [
                ...(spokenContent.trim() ? [{ type: 'text', text: spokenContent }] : []),
                ...imageUrls.map((url) => ({ type: 'image_url', image_url: { url } })),
              ]
            : spokenContent
        // Plan turns carry the plan-mode harness: think and decide, read
        // freely, create NOTHING — tickets come from Draft tickets later.
        const messages = [
          ...(kind === 'plan' ? [{ role: 'system' as const, content: PLAN_MODE_PROMPT }] : []),
          ...prior,
          { role: 'user' as const, content: userContent as unknown as string },
        ]
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
          tier: body.tier ?? null,
          plan: planMeta,
        })
        return new Response(toClient, { status: upstream.status, headers })
      },
    },
  },
})
