// Agent replies in group channels. When a human's message @mentions a channel
// agent (by model id or friendly label), that agent replies: we build the
// channel transcript as gateway history, stream the completion, and persist it
// into the channel (throttled flushes + Redis publish) so every member watches
// it type in real time. Runs detached from the sender's request.
import { describeAgent, proxyChat } from './gateway'
import { parseAgentStream } from '@/lib/sse-parse'
import {
  insertChannelMessage,
  listChannelAgents,
  listChannelMessages,
  updateChannelMessage,
  type ChannelMessage,
} from './channels'

/** Channel agents @mentioned in the text — matched on model id ("@dex-developer")
 *  or label ("@Dex"), case-insensitive. */
export function mentionedAgents(content: string, channelAgents: string[]): string[] {
  const mentions = new Set(
    [...content.matchAll(/@([a-z0-9][a-z0-9-]*)/gi)].map((m) => m[1]!.toLowerCase()),
  )
  if (mentions.size === 0) return []
  return channelAgents.filter((model) => {
    const { label } = describeAgent(model)
    return mentions.has(model.toLowerCase()) || mentions.has(label.toLowerCase())
  })
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
  for (const model of mentioned) {
    const history = await listChannelMessages(channelId, -1, 60)
    const row = await insertChannelMessage(channelId, 'agent', model, '', 'streaming')
    void streamReply(channelId, row.id, model, [
      { role: 'system', content: systemPrompt(model, channelName, agents) },
      ...transcriptFor(model, history),
    ]).catch(() => updateChannelMessage(channelId, row.id, '', 'error'))
  }
}

async function streamReply(
  channelId: string,
  messageId: string,
  model: string,
  messages: Array<{ role: string; content: string }>,
): Promise<void> {
  const upstream = await proxyChat({ model, messages })
  if (!upstream.ok || !upstream.body) {
    await updateChannelMessage(channelId, messageId, `(gateway error ${upstream.status})`, 'error')
    return
  }
  let content = ''
  let lastFlush = 0
  try {
    for await (const ev of parseAgentStream(upstream.body)) {
      if (ev.type === 'content') content += ev.text
      const now = Date.now()
      if (now - lastFlush > 400) {
        lastFlush = now
        await updateChannelMessage(channelId, messageId, content, 'streaming')
      }
    }
    await updateChannelMessage(channelId, messageId, content, 'complete')
  } catch {
    await updateChannelMessage(channelId, messageId, content, 'error')
  }
}
