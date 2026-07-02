// Agent replies in group channels. When a human's message @mentions a channel
// agent (by model id or friendly label), that agent replies: we build the
// channel transcript as gateway history, stream the completion, and persist it
// into the channel (throttled flushes + Redis publish) so every member watches
// it type in real time. Runs detached from the sender's request.
import { describeAgent, proxyChat } from './gateway'
import { parseAgentStream } from '@/lib/sse-parse'
import { addNotification } from './notifications'
import { estimateTokens, recordUsage } from './usage'
import { routedModelFor } from './fleet-agents'
import {
  insertChannelMessage,
  listChannelAgents,
  listChannelMembers,
  listChannelMessages,
  updateChannelMessage,
  type ChannelMessage,
} from './channels'

/** Tokens a member answers to: email localpart, dashed full name, first name. */
export function userMentionTokens(name: string | null, email: string | null): string[] {
  const tokens = new Set<string>()
  const local = email?.split('@')[0]?.toLowerCase()
  if (local) tokens.add(local)
  const n = name?.trim().toLowerCase()
  if (n) {
    tokens.add(n.replace(/\s+/g, '-'))
    tokens.add(n.split(/\s+/)[0]!)
  }
  return [...tokens]
}

/** Notify channel members the message @mentions (never the sender). */
export async function notifyUserMentions(
  channelId: string,
  channelName: string,
  senderUserId: string,
  senderLabel: string,
  content: string,
): Promise<void> {
  const mentions = new Set(
    [...content.matchAll(/@([a-z0-9][a-z0-9-]*)/gi)].map((m) => m[1]!.toLowerCase()),
  )
  if (mentions.size === 0) return
  const members = await listChannelMembers(channelId)
  for (const m of members) {
    if (m.userId === senderUserId) continue
    if (!userMentionTokens(m.name, m.email).some((t) => mentions.has(t))) continue
    await addNotification(m.userId, {
      kind: 'mention',
      title: `${senderLabel} mentioned you in #${channelName}`,
      body: content.length > 200 ? `${content.slice(0, 200)}…` : content,
      href: '/channels',
    })
  }
}

/** Channel agents @mentioned in the text — matched on model id ("@dex-developer")
 *  or label ("@Dex"), case-insensitive. "@Dex:opus" requests a model tier; the
 *  first mention of an agent wins (one reply per agent per message). */
export function mentionedAgents(content: string, channelAgents: string[]): Array<{ model: string; tier: string | null }> {
  const mentions = [...content.matchAll(/@([a-z0-9][a-z0-9-]*)(?::([a-z0-9-]+))?/gi)].map((m) => ({
    token: m[1]!.toLowerCase(),
    tier: m[2]?.toLowerCase() ?? null,
  }))
  if (mentions.length === 0) return []
  const hits: Array<{ model: string; tier: string | null }> = []
  for (const model of channelAgents) {
    const { label } = describeAgent(model)
    const hit = mentions.find((m) => m.token === model.toLowerCase() || m.token === label.toLowerCase())
    if (hit) hits.push({ model, tier: hit.tier })
  }
  return hits
}

/** The channel transcript as OpenAI-style history from one agent's point of
 *  view: its own turns are `assistant`, everyone else speaks as `user` with a
 *  "Name:" prefix so the agent can tell voices apart. */
function transcriptFor(model: string, messages: ChannelMessage[]): Array<{ role: string; content: string }> {
  const turns: Array<{ role: string; content: string }> = []
  for (const m of messages) {
    if (m.status !== 'complete' || !m.content) continue
    if (m.authorType === 'agent' && m.author === model) {
      turns.push({ role: 'assistant', content: m.content })
    } else {
      const name = m.authorType === 'agent' ? describeAgent(m.author).label : m.author
      turns.push({ role: 'user', content: `${name}: ${m.content}` })
    }
  }
  return turns
}

function systemPrompt(model: string, channelName: string, channelAgents: string[]): string {
  const me = describeAgent(model)
  const others = channelAgents.filter((a) => a !== model).map((a) => describeAgent(a).label)
  return (
    `You are ${me.label} (${me.role}), a member of the group channel #${channelName}. ` +
    `Messages from others are prefixed with the sender's name; reply as yourself, without a prefix. ` +
    (others.length ? `Other agents in the channel: ${others.join(', ')}. ` : '') +
    `Keep replies conversational and channel-sized. You were @mentioned — answer that message.`
  )
}

/** Fire agent replies for a just-posted message. Detached: returns after the
 *  streaming rows exist; the streams drain in the background. */
export async function triggerAgentReplies(channelId: string, channelName: string, content: string): Promise<void> {
  const agents = await listChannelAgents(channelId)
  const mentioned = mentionedAgents(content, agents)
  for (const { model, tier } of mentioned) {
    // An unknown tier falls back to the agent's main model — a typo shouldn't
    // swallow the reply.
    const routed = (tier ? await routedModelFor(model, tier).catch(() => null) : null) ?? model
    const history = await listChannelMessages(channelId, -1, 60)
    const row = await insertChannelMessage(channelId, 'agent', model, '', 'streaming')
    void streamReply(channelId, row.id, model, routed, [
      { role: 'system', content: systemPrompt(model, channelName, agents) },
      ...transcriptFor(model, history),
    ]).catch(() => updateChannelMessage(channelId, row.id, '', 'error'))
  }
}

async function streamReply(
  channelId: string,
  messageId: string,
  model: string,
  routedModel: string,
  messages: Array<{ role: string; content: string }>,
): Promise<void> {
  const upstream = await proxyChat({ model: routedModel, messages })
  if (!upstream.ok || !upstream.body) {
    await updateChannelMessage(channelId, messageId, `(gateway error ${upstream.status})`, 'error')
    return
  }
  let content = ''
  let usage: { promptTokens: number; completionTokens: number } | null = null
  let lastFlush = 0
  const ledger = () => {
    const promptChars = messages.reduce((n, m) => n + m.content.length, 0)
    void recordUsage({
      agentModel: model,
      source: 'channel',
      refId: channelId,
      // Attribute tier turns to the ALIAS endpoint, mirroring /api/chat.
      tier: routedModel !== model ? routedModel.slice(model.length + 1) : null,
      promptTokens: usage?.promptTokens ?? estimateTokens(promptChars),
      completionTokens: usage?.completionTokens ?? estimateTokens(content.length),
      estimated: !usage,
    }).catch(() => {})
  }
  try {
    for await (const ev of parseAgentStream(upstream.body)) {
      if (ev.type === 'content') content += ev.text
      else if (ev.type === 'usage') usage = ev
      const now = Date.now()
      if (now - lastFlush > 400) {
        lastFlush = now
        await updateChannelMessage(channelId, messageId, content, 'streaming')
      }
    }
    await updateChannelMessage(channelId, messageId, content, 'complete')
    ledger()
  } catch {
    await updateChannelMessage(channelId, messageId, content, 'error')
    ledger()
  }
}
