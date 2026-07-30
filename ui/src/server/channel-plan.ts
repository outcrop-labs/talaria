// Plan chat: turn a channel conversation into ticket proposals. A chosen
// channel agent reads the transcript and drafts structured tickets; a human
// reviews, edits, and creates them (into inbox — planning never assigns).
import { describeAgent, proxyChat } from './gateway'
import { parseAgentStream } from '@/lib/sse-parse'
import { estimateTokens, recordUsage } from './usage'
import { listChannelMessages } from './channels'
import { priorMessages } from './conversations'
import { planDocFor } from './plan-doc'
import { resolveTemplate, templatePrompt, type Template } from './templates'
import { routingContext } from './workflows'

export interface TicketProposal {
  title: string
  description: string
  priority: 'low' | 'medium' | 'high' | 'urgent'
  effort: 'xs' | 's' | 'm' | 'l' | 'xl' | null
  /** Zero-based indices of proposals in the SAME batch this one is blocked by. */
  dependsOn: number[]
  /** Routing labels — chosen to trip a workflow's match rules so dispatch
   *  classification fires when the ticket is later approved to an agent. */
  tags: string[]
}

const PRIORITIES = new Set(['low', 'medium', 'high', 'urgent'])
const EFFORTS = new Set(['xs', 's', 'm', 'l', 'xl'])

const PLAN_PROMPT = `You are a planning assistant. Break the discussed work into concrete, actionable tickets.
When a plan document is provided, it is the curated source of truth — draft tickets from it and use the transcript only for supporting context; the raw chat never overrides the document.

Respond with ONLY a JSON array — no prose before or after, no markdown fence. Each element:
{"title": "imperative, <= 80 chars", "description": "markdown body with enough context that someone who didn't read the chat can act on it", "priority": "low|medium|high|urgent", "effort": "xs|s|m|l|xl", "dependsOn": [zero-based indices of tickets in THIS array that must finish first], "tags": ["optional routing labels"]}

Rules: 2-10 tickets. Each independently actionable. Don't invent work nobody discussed. Capture decisions and constraints (and any @mentioned people) in the descriptions. Use dependsOn only for real ordering constraints — most tickets have none.
When a workflow map is provided and a ticket clearly falls under one of its workflows, add that workflow's matching label(s) to tags and end the description with one line: "Routing: <workflow> → <agent>". Most tickets won't match — then omit tags and the routing line entirely; never force a fit.`

/** Extract a JSON array of proposals from model output. Tries EVERY '['
 *  candidate (prose like "[DONE]" or markdown links before the real array must
 *  not break extraction); field limits mirror the boards API so review-modal
 *  creates can't 400. */
export function extractProposals(text: string): TicketProposal[] {
  for (let start = text.indexOf('['); start !== -1; start = text.indexOf('[', start + 1)) {
    const arr = parseArrayAt(text, start)
    if (!arr) continue
    const objs = arr.filter((x): x is Record<string, unknown> => !!x && typeof x === 'object' && !Array.isArray(x))
    // Dropping titleless entries shifts positions — remap dependsOn through the
    // original→kept index map so references stay valid.
    const kept = objs.map((x, i) => ({ x, i })).filter(({ x }) => String(x.title ?? '').trim().length > 0)
    const newIndex = new Map(kept.map(({ i }, n) => [i, n]))
    const proposals = kept.map(({ x, i }) => ({
      title: String(x.title ?? '').slice(0, 300),
      description: String(x.description ?? '').slice(0, 20_000),
      priority: PRIORITIES.has(String(x.priority)) ? (String(x.priority) as TicketProposal['priority']) : 'medium',
      effort: EFFORTS.has(String(x.effort)) ? (String(x.effort) as Exclude<TicketProposal['effort'], null>) : null,
      tags: (Array.isArray(x.tags) ? x.tags : [])
        .map((t) => String(t).trim().slice(0, 40))
        .filter(Boolean)
        .slice(0, 5),
      dependsOn: [
        ...new Set(
          (Array.isArray(x.dependsOn) ? x.dependsOn : [])
            .map((d) => newIndex.get(Number(d)))
            .filter((d): d is number => d !== undefined && d !== newIndex.get(i)),
        ),
      ],
    }))
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
  opts: {
    /** The plan's living document — the authoritative source when present. */
    planDoc?: string
    /** The resolved ticket template — descriptions must follow its skeleton. */
    template?: Template | null
  } = {},
): Promise<{ proposals: TicketProposal[]; raw: string }> {
  if (!transcript.trim() && !opts.planDoc?.trim()) return { proposals: [], raw: '' }

  const system = opts.template ? `${PLAN_PROMPT}\n\n${templatePrompt(opts.template, 'ticket descriptions')}` : PLAN_PROMPT
  const routing = await routingContext().catch(() => '')
  const parts = [
    ...(routing ? [`Workflow map (match rules → skills → agents):\n${routing}`] : []),
    ...(opts.planDoc?.trim() ? [`Plan document (source of truth):\n\n${opts.planDoc}`] : []),
    ...(transcript.trim() ? [`Transcript:\n\n${transcript}`] : []),
  ]
  const messages = [
    { role: 'system', content: system },
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

/** Template context for a draft: where the tickets will land + any explicit
 *  pick. Resolution (explicit → agent → board default → none) happens here. */
export interface DraftTemplateCtx {
  boardId?: string | null
  templateId?: string | null
}

export async function planFromChannel(
  channelId: string,
  agentModel: string,
  routedModel: string,
  tpl: DraftTemplateCtx = {},
): Promise<{ proposals: TicketProposal[]; raw: string }> {
  const history = await listChannelMessages(channelId, -1, 80)
  const transcript = history
    .filter((m) => m.status === 'complete' && m.content)
    .map((m) => `${m.authorType === 'agent' ? describeAgent(m.author).label : m.author}: ${m.content}`)
    .join('\n\n')
  const template = await resolveTemplate('ticket', { explicitId: tpl.templateId, agentModel, boardId: tpl.boardId })
  return planFromTranscript(transcript, agentModel, routedModel, channelId, 'channel', { template })
}

/** Draft tickets from a plan conversation (the Plan surface). The plan's living
 *  document, when it has content, is the primary source; chat is context. */
export async function planFromConversation(
  conversationId: string,
  agentModel: string,
  routedModel: string,
  tpl: DraftTemplateCtx = {},
): Promise<{ proposals: TicketProposal[]; raw: string }> {
  const label = describeAgent(agentModel).label
  const msgs = await priorMessages(conversationId)
  const transcript = msgs
    .filter((m) => m.content)
    .map((m) => `${m.role === 'assistant' ? label : 'User'}: ${m.content}`)
    .join('\n\n')
  const doc = await planDocFor(conversationId)
  const template = await resolveTemplate('ticket', { explicitId: tpl.templateId, agentModel, boardId: tpl.boardId })
  return planFromTranscript(transcript, agentModel, routedModel, conversationId, 'chat', { planDoc: doc?.body, template })
}
