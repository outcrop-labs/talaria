// Plan chat: turn a channel conversation into ticket proposals. A chosen
// channel agent reads the transcript and drafts structured tickets; a human
// reviews, edits, and creates them (into inbox — planning never assigns).
import { describeAgent, proxyChat } from './gateway'
import { parseAgentStream } from '@/lib/sse-parse'
import { estimateTokens, recordUsage } from './usage'
import { listChannelMessages } from './channels'

export interface TicketProposal {
  title: string
  description: string
  priority: 'low' | 'medium' | 'high' | 'urgent'
  effort: 'xs' | 's' | 'm' | 'l' | 'xl' | null
}

const PRIORITIES = new Set(['low', 'medium', 'high', 'urgent'])
const EFFORTS = new Set(['xs', 's', 'm', 'l', 'xl'])

const PLAN_PROMPT = `You are a planning assistant. Read the conversation transcript and break the discussed work into concrete, actionable tickets.

Respond with ONLY a JSON array — no prose before or after, no markdown fence. Each element:
{"title": "imperative, <= 80 chars", "description": "markdown body with enough context that someone who didn't read the chat can act on it", "priority": "low|medium|high|urgent", "effort": "xs|s|m|l|xl"}

Rules: 2-10 tickets. Each independently actionable. Don't invent work nobody discussed. Capture decisions and constraints from the chat in the descriptions.`

/** Extract the first JSON array in a blob of model output (tolerates fences
 *  and prose around it). */
export function extractProposals(text: string): TicketProposal[] {
  const start = text.indexOf('[')
  if (start === -1) return []
  // Walk to the matching close bracket (strings may contain brackets).
  let depth = 0
  let inStr = false
  let esc = false
  for (let i = start; i < text.length; i++) {
    const ch = text[i]
    if (esc) {
      esc = false
      continue
    }
    if (ch === '\\') {
      esc = inStr
      continue
    }
    if (ch === '"') inStr = !inStr
    if (inStr) continue
    if (ch === '[') depth++
    if (ch === ']' && --depth === 0) {
      try {
        const arr = JSON.parse(text.slice(start, i + 1)) as unknown[]
        return arr
          .filter((x): x is Record<string, unknown> => !!x && typeof x === 'object')
          .map((x) => ({
            title: String(x.title ?? '').slice(0, 300),
            description: String(x.description ?? ''),
            priority: PRIORITIES.has(String(x.priority)) ? (String(x.priority) as TicketProposal['priority']) : 'medium',
            effort: EFFORTS.has(String(x.effort)) ? (String(x.effort) as Exclude<TicketProposal['effort'], null>) : null,
          }))
          .filter((p) => p.title.trim().length > 0)
      } catch {
        return []
      }
    }
  }
  return []
}

export async function planFromChannel(
  channelId: string,
  agentModel: string,
  routedModel: string,
): Promise<{ proposals: TicketProposal[]; raw: string }> {
  const history = await listChannelMessages(channelId, -1, 80)
  const transcript = history
    .filter((m) => m.status === 'complete' && m.content)
    .map((m) => `${m.authorType === 'agent' ? describeAgent(m.author).label : m.author}: ${m.content}`)
    .join('\n\n')
  if (!transcript) return { proposals: [], raw: '' }

  const messages = [
    { role: 'system', content: PLAN_PROMPT },
    { role: 'user', content: `Transcript of #channel:\n\n${transcript}` },
  ]
  const upstream = await proxyChat({ model: routedModel, messages })
  if (!upstream.ok || !upstream.body) throw new Error(`gateway error ${upstream.status}`)

  let text = ''
  let usage: { promptTokens: number; completionTokens: number } | null = null
  for await (const ev of parseAgentStream(upstream.body)) {
    if (ev.type === 'content') text += ev.text
    else if (ev.type === 'usage') usage = ev
  }
  void recordUsage({
    agentModel,
    source: 'channel',
    refId: channelId,
    tier: routedModel !== agentModel ? routedModel.slice(agentModel.length + 1) : null,
    promptTokens: usage?.promptTokens ?? estimateTokens(messages.reduce((n, m) => n + m.content.length, 0)),
    completionTokens: usage?.completionTokens ?? estimateTokens(text.length),
    estimated: !usage,
  }).catch(() => {})

  return { proposals: extractProposals(text), raw: text }
}
