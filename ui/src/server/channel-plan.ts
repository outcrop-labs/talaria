// Plan chat: turn a channel conversation into ticket proposals. A chosen
// channel agent reads the transcript and drafts structured tickets; a human
// reviews, edits, and creates them (into inbox — planning never assigns).
import { describeAgent, proxyChat } from './gateway'
import { parseAgentStream } from '@/lib/sse-parse'
import { estimateTokens, recordUsage } from './usage'
import { listChannelMessages } from './channels'
import { priorMessages } from './conversations'
import { planDocFor } from './plan-doc'

export interface TicketProposal {
  title: string
  description: string
  priority: 'low' | 'medium' | 'high' | 'urgent'
  effort: 'xs' | 's' | 'm' | 'l' | 'xl' | null
}

const PRIORITIES = new Set(['low', 'medium', 'high', 'urgent'])
const EFFORTS = new Set(['xs', 's', 'm', 'l', 'xl'])

const PLAN_PROMPT = `You are a planning assistant. Break the discussed work into concrete, actionable tickets.
When a plan document is provided, it is the curated source of truth — draft tickets from it and use the transcript only for supporting context; the raw chat never overrides the document.

Respond with ONLY a JSON array — no prose before or after, no markdown fence. Each element:
{"title": "imperative, <= 80 chars", "description": "markdown body with enough context that someone who didn't read the chat can act on it", "priority": "low|medium|high|urgent", "effort": "xs|s|m|l|xl"}

Rules: 2-10 tickets. Each independently actionable. Don't invent work nobody discussed. Capture decisions and constraints (and any @mentioned people) in the descriptions.`

/** Extract a JSON array of proposals from model output. Tries EVERY '['
 *  candidate (prose like "[DONE]" or markdown links before the real array must
 *  not break extraction); field limits mirror the boards API so review-modal
 *  creates can't 400. */
export function extractProposals(text: string): TicketProposal[] {
  for (let start = text.indexOf('['); start !== -1; start = text.indexOf('[', start + 1)) {
    const arr = parseArrayAt(text, start)
    if (!arr) continue
    const proposals = arr
      .filter((x): x is Record<string, unknown> => !!x && typeof x === 'object' && !Array.isArray(x))
      .map((x) => ({
        title: String(x.title ?? '').slice(0, 300),
        description: String(x.description ?? '').slice(0, 20_000),
        priority: PRIORITIES.has(String(x.priority)) ? (String(x.priority) as TicketProposal['priority']) : 'medium',
        effort: EFFORTS.has(String(x.effort)) ? (String(x.effort) as Exclude<TicketProposal['effort'], null>) : null,
      }))
      .filter((p) => p.title.trim().length > 0)
    if (proposals.length) return proposals
  }
  return []
}

/** Parse a balanced JSON array starting at `start`, or null. */
function parseArrayAt(text: string, start: number): unknown[] | null {
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
        const v = JSON.parse(text.slice(start, i + 1)) as unknown
        return Array.isArray(v) ? v : null
      } catch {
        return null
      }
    }
  }
  return null
}

/** Draft ticket proposals from a transcript. Shared by the channel Plan button
 *  and the first-class Plan surface. */
async function planFromTranscript(
  transcript: string,
  agentModel: string,
  routedModel: string,
  refId: string,
  source: 'channel' | 'chat',
  /** The plan's living document — the authoritative source when present. */
  planDoc?: string,
): Promise<{ proposals: TicketProposal[]; raw: string }> {
  if (!transcript.trim() && !planDoc?.trim()) return { proposals: [], raw: '' }

  const parts = [
    ...(planDoc?.trim() ? [`Plan document (source of truth):\n\n${planDoc}`] : []),
    ...(transcript.trim() ? [`Transcript:\n\n${transcript}`] : []),
  ]
  const messages = [
    { role: 'system', content: PLAN_PROMPT },
    { role: 'user', content: parts.join('\n\n---\n\n') },
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
    source,
    refId,
    tier: routedModel !== agentModel ? routedModel.slice(agentModel.length + 1) : null,
    promptTokens: usage?.promptTokens ?? estimateTokens(messages.reduce((n, m) => n + m.content.length, 0)),
    completionTokens: usage?.completionTokens ?? estimateTokens(text.length),
    estimated: !usage,
  }).catch(() => {})

  return { proposals: extractProposals(text), raw: text }
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
  return planFromTranscript(transcript, agentModel, routedModel, channelId, 'channel')
}

/** Draft tickets from a plan conversation (the Plan surface). The plan's living
 *  document, when it has content, is the primary source; chat is context. */
export async function planFromConversation(
  conversationId: string,
  agentModel: string,
  routedModel: string,
): Promise<{ proposals: TicketProposal[]; raw: string }> {
  const label = describeAgent(agentModel).label
  const msgs = await priorMessages(conversationId)
  const transcript = msgs
    .filter((m) => m.content)
    .map((m) => `${m.role === 'assistant' ? label : 'User'}: ${m.content}`)
    .join('\n\n')
  const doc = await planDocFor(conversationId)
  return planFromTranscript(transcript, agentModel, routedModel, conversationId, 'chat', doc?.body)
}
