import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { Uuid } from '@/lib/api-schema'
import { proxyChat } from '@/server/gateway'
import { routedModelFor } from '@/server/fleet-agents'
import { effortsForModel } from '@/server/model-efforts'
import { personaConfiguredEffort } from '@/server/harness/persona'
import { parseBody, requireUser } from '@/server/api-guard'
import { hasPerm } from '@/server/permissions'
import { canUseAgentModel } from '@/server/users'
import type { ConversationKind } from '@/server/conversations'
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
import { markAgentTurn } from '@/server/research-origin'
import { listResearchMembers, RESEARCH_MODE_PROMPT, researchRunForConversation } from '@/server/research'
import { attachmentAsDataUrl, attachmentTextBlocks, resolveAttachments, isImage } from '@/server/uploads'
import { refBlocks, resolveRefs } from '@/server/refs'
import { notifyPlanMentions, PLAN_MODE_PROMPT, planRoutingBlock } from '@/server/plan-doc'
import { indexActivity } from '@/server/retrieval/sources'
import { HANDLE_TURN_NOTE, mentionsHandle } from '@/server/workspace-secrets'

const Body = z.object({
  model: z.string().min(1),
  conversationId: Uuid.optional(),
  // Content may be empty when the turn is attachments-only.
  content: z.string().max(100_000).default(''),
  /** Model tier (alias name) to route this turn to; omit for the main model. */
  tier: z.string().max(60).optional(),
  /** Reasoning effort for this turn, from the levels the routed model's
   *  metadata vouches for. Omitted = the model's own default, and the outbound
   *  request carries no effort field at all. Validated against the same
   *  metadata the composer's picker lists, so a mismatch is a stale client or
   *  a hand-built request — answered like the tier check below, not ignored. */
  effort: z.string().max(24).optional(),
  attachmentIds: z.array(Uuid).max(10).optional(),
  /** Knowledge docs / artifacts attached as references (ACL-checked). */
  refs: z.array(z.object({ type: z.enum(['kb-doc', 'artifact']), id: Uuid })).max(3).optional(),
  /** New conversations are 'chat' unless started from the Plan surface. */
  /** 'research' is a conversation ABOUT a report, on the Research surface —
   *  same multiplayer shape as a plan, shared through the RUN's members. */
  kind: z.enum(['chat', 'plan', 'research']).optional(),
  /** Explicit plan template pick (plan surface, new plan only); server ignores
   *  it for chats and for turns on an existing conversation. */
  templateId: Uuid.nullish(),
  /** Sent while a reply streams: queue into history, never interrupt. */
  queue: z.boolean().optional(),
})

const titleFrom = (s: string) => s.replace(/\s+/g, ' ').trim().slice(0, 80)

// POST /api/chat { model, conversationId?, content } → durable streaming chat.
// Persists the turn to Postgres (server owns history) and tees the gateway
// stream: one branch to the client, one drained to the DB so an in-progress
// reply survives a disconnect. Returns SSE + X-Conversation-Id / X-Message-Id.
export const Route = defineApi('/api/chat', {
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
    let kind: ConversationKind = body.kind ?? 'chat'
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
      // WHO ELSE IS IN THE ROOM. Both shared surfaces prefix a turn with the
      // speaker's name when there is more than one human, so the agent can tell
      // voices apart — research reads its roster off the RUN's members rather
      // than `conversation_members`, which is the same rule its access follows.
      if (kind === 'plan') multiVoice = (await listPlanMembers(convId)).length > 1
      else if (kind === 'research') {
        const runId = await researchRunForConversation(convId).catch(() => null)
        multiVoice = runId !== null && (await listResearchMembers(runId).catch(() => [])).length > 0
      }
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

    // Effort: the same rule the tier above follows, against the per-model
    // metadata the catalog refresh extracted (`effortsForModel` resolves a
    // persona id to the model actually serving it). A model that publishes no
    // levels supports no pick, so ANY effort on it is the error — "not
    // supported" and "not one of yours" read identically to the sender.
    //
    // No pick on a persona with a CONFIGURED default (the agent editor's pick
    // beside the model) runs at that default — re-validated here, so a level
    // that went stale between the admin saving it and the turn using it is
    // inert rather than a 400. An explicit pick always wins, including the
    // composer's `auto`-means-absent.
    const efforts = await effortsForModel(routedModel)
    let effort: string | undefined = body.effort
    if (effort !== undefined && !efforts.includes(effort)) {
      return json(
        { error: `unsupported effort "${effort}" for ${routedModel}${efforts.length ? ` (offered: ${efforts.join(', ')})` : ''}` },
        { status: 400 },
      )
    }
    if (effort === undefined) {
      const configured = await personaConfiguredEffort(routedModel).catch(() => null)
      if (configured && efforts.includes(configured)) effort = configured
    }

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
    // The effort pick rides the user's row: the queued-message contract. A
    // reply that is already streaming means this turn is covered later by
    // `continueConversation`, which re-reads exactly this stamp (and
    // re-validates it against the routed model) when it builds the next turn.
    const userMsgId = await insertUserMessage(convId, userSeq, content, attachments, user.id, effort ? { effort } : {})
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
    //
    // A HANDLE IN THE MESSAGE NEEDS EXPLAINING, and only when one is there.
    // Standing grants ride in the agent's soul, rendered with everything else it
    // is; a relay is minted mid-conversation for one errand, so the soul written
    // this morning has never heard of it — and an agent granted nothing at all is
    // told nothing at all, by design, so it has never heard of handles either.
    // Unexplained, it reads `«secret:relay-…»` as a typo and asks the human to
    // send the real value instead, which is exactly the paste the relay existed
    // to prevent. Costs a paragraph, on the turns that mention one.
    const messages = [
      ...(kind === 'plan' ? [{ role: 'system' as const, content: PLAN_MODE_PROMPT + (await planRoutingBlock().catch(() => '')) }] : []),
      ...(kind === 'research' ? [{ role: 'system' as const, content: RESEARCH_MODE_PROMPT }] : []),
      ...(mentionsHandle(spokenContent) ? [{ role: 'system' as const, content: HANDLE_TURN_NOTE }] : []),
      ...prior,
      { role: 'user' as const, content: userContent as unknown as string },
    ]
    const assistantId = await insertStreamingAssistant(convId, userSeq + 1)

    // WHERE THIS AGENT IS ANSWERING, recorded before the turn leaves for the
    // container and not after. Work the agent starts from inside this turn —
    // a research run, today — reaches Talaria as its own authenticated request
    // carrying nothing but an agent key, so this is the only place the two can
    // be tied together. See research-origin.ts for what it is for.
    void markAgentTurn(agentModel, convId).catch(() => {})

    const upstream = await proxyChat({
      model: routedModel,
      messages,
      ...(effort ? { reasoning_effort: effort } : {}),
    })
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
})
