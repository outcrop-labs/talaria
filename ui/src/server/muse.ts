// The drafting muse: users iteratively construct agent internals (souls,
// personalities, skills, memories, cron jobs) with an LLM. One server entry
// builds kind-aware messages and rides the Talaria gateway machinery (route
// resolution, provider keys, request defaults, metering) — so the muse
// respects the same model registry as everything else, on the user's
// preferred model.
import { gatewayModels, resolveRoute } from './llm-gateway'
import { getPreferredModel } from './users'

export type MuseKind = 'soul' | 'personality' | 'skill' | 'memory' | 'cron' | 'agent' | 'document'

const DOC_RULES =
  'Return ONLY the complete revised document — no commentary, no preamble, no code fences. ' +
  'Start from the current version when one is given: keep what works, change what the request asks, never silently drop sections.'

const SYSTEM: Record<MuseKind, string> = {
  soul:
    'You write SOUL.md files for Hermes agents — the markdown document that defines who an agent is: identity, personality, operating principles, and guardrails. ' +
    'Keep the heading structure, keep it tight and actionable, and preserve existing guardrails unless explicitly asked to change them. ' +
    DOC_RULES,
  personality:
    "You write the personality brief for someone's personal AI assistant: how it should come across — tone, priorities, pet peeves. " +
    'Plain prose, second person ("Be…"), a few sentences to a short paragraph; no headings. ' +
    DOC_RULES,
  skill:
    'You write SKILL.md playbooks that an AI agent follows for a recurring job. Markdown with a # title, a line on when to use it, and concrete numbered steps; ' +
    'be specific about tools/sources when the request names them. ' +
    DOC_RULES,
  memory:
    "You curate an AI agent's MEMORY.md — durable facts, preferences, and context it should remember. Terse bullet lines, grouped under short headings when helpful; " +
    'never invent facts — only reorganize, prune, or add what the request states. ' +
    DOC_RULES,
  cron:
    'You design a scheduled job for an AI agent. Reply with ONLY a JSON object — no fences, no commentary — shaped exactly: ' +
    '{"name": "<kebab-case-short-name>", "schedule": "<5-field cron expr or interval like \\"every 2h\\"/\\"30m\\">", "prompt": "<self-contained instruction the agent executes each run>"} ' +
    'Times are UTC. Prefer cron expressions for fixed times ("0 14 * * 1-5"), intervals for frequencies.',
  agent:
    'You design a complete Hermes AI agent from a purpose description. Reply with ONLY a JSON object — no fences, no commentary — shaped exactly:\n' +
    '{"name": "<short human first-name, e.g. \\"Rex\\">", "handle": "<lowercase alphanumeric, starts with a letter, 2-30 chars>", ' +
    '"department": "<lowercase-kebab function word, e.g. \\"release\\" or \\"research\\">", "role": "<human job title, e.g. \\"Release Manager\\">", ' +
    '"soul": "<the full SOUL.md markdown>", "skills": [{"name": "<kebab-case>", "content": "<full SKILL.md markdown>"}]}\n' +
    'The soul is the agent\'s defining document: a "# <Name> — <Role>" title, then "## Who you are" (identity + mission from the purpose), ' +
    '"## Voice & personality" (a distinct, likable working voice), and "## How you work" — which MUST keep these guardrails: ' +
    'keep humans in the loop (create and triage tickets, never assign or close them); prefer the local model tier for routine work, escalate deliberately; ask in the channel instead of guessing. ' +
    'Include 0–3 skills, only ones clearly implied by the purpose (each a # title, a when-to-use line, concrete numbered steps). ' +
    'When a current draft is given, revise it per the request instead of starting over — keep everything not asked about.',
  document: 'You help edit a markdown document. ' + DOC_RULES,
}

export interface MuseInput {
  kind: MuseKind
  instruction: string
  /** The document as it stands in the editor (context + revision base). */
  current?: string
  /** One line of situational context, e.g. the agent's name and role. */
  context?: string
  /** Prior muse turns in this session, for iterative refinement. */
  chat?: Array<{ role: 'user' | 'assistant'; content: string }>
}

export function buildMuseMessages(input: MuseInput): Array<{ role: string; content: string }> {
  let system = SYSTEM[input.kind]
  if (input.context) system += `\n\nContext: ${input.context}`
  if (input.current?.trim()) system += `\n\nCurrent version:\n<<<\n${input.current}\n>>>`
  else system += '\n\nThere is no current version yet — write one from scratch.'
  return [
    { role: 'system', content: system },
    ...(input.chat ?? []).slice(-12),
    { role: 'user', content: input.instruction },
  ]
}

/** The model powering a user's muse: their preference if it still routes,
 *  else the env default, else pl-main, else the first registered model. */
export async function museModelFor(userId: string): Promise<string | null> {
  const candidates = [await getPreferredModel(userId), process.env.TALARIA_COPILOT_MODEL ?? null, 'pl-main']
  for (const m of candidates) {
    if (m && (await resolveRoute(m))) return m
  }
  const first = (await gatewayModels()).find((m) => !m.qualified)
  return first?.id ?? null
}
