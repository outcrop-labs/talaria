// Confab guardrail — a cheap, model-agnostic STRUCTURAL check on model output,
// run at the gateway so it's drop-in across every model class. No LLM call, no
// added model tokens: it's regex over the answer + the turn's tool record
// (derived from the request messages, so no separate tool-trace export needed).
//
// Three checks, ported from the Hermes confab-guard plugin:
//   zero_tool_claim   — claims a completed action, but no external tool ran
//   ungrounded_ref    — cites a link/UUID that wasn't in any tool result
//   fabricated_outage — claims an outage, but no tool actually errored
//
// Default posture is OBSERVE: detect + record findings out-of-band, never touch
// the output or the model's context. annotate/strict are opt-in.

import { getSetting, setSetting } from './audit'
import { db } from './db/pg'

export type GuardMode = 'off' | 'observe' | 'annotate' | 'strict'
export type GuardCheck = 'zero_tool_claim' | 'ungrounded_ref' | 'fabricated_outage'

export interface GuardConfig {
  mode: GuardMode
  checks: Record<GuardCheck, boolean>
  /** Hosts whose links get grounding-checked (org-internal tools). UUIDs are
   *  always checked. Empty ⇒ only UUIDs are policed. */
  policedHosts: string[]
}

const DEFAULT_CONFIG: GuardConfig = {
  mode: 'observe',
  checks: { zero_tool_claim: true, ungrounded_ref: true, fabricated_outage: true },
  policedHosts: [],
}

export const getGuardConfig = async (): Promise<GuardConfig> => {
  const c = await getSetting<Partial<GuardConfig>>('guardrails_config', DEFAULT_CONFIG)
  return { ...DEFAULT_CONFIG, ...c, checks: { ...DEFAULT_CONFIG.checks, ...c.checks } }
}
export const setGuardConfig = (c: GuardConfig) => setSetting('guardrails_config', c)

// ── Message → tool record ────────────────────────────────────────────────────

export interface ToolRecord {
  /** Backing tools that ran this turn (excludes think/memory/todo/…). */
  backingTools: string[]
  /** Concatenated tool-result text this turn. */
  resultsText: string
  /** A tool returned a transport/availability error this turn. */
  anyError: boolean
  /** Results too large to fully inspect → skip the grounding check (fail open). */
  overflowed: boolean
}

// Tools that don't count as a real external action for the zero-tool check.
const NONBACKING = new Set(['memory', 'todo', 'think', 'skill_manage', 'session_search', 'tool_search', 'tool_describe', 'search_knowledge'])
const RESULTS_CAP = 200_000

type Msg = { role?: string; content?: unknown; tool_calls?: Array<{ function?: { name?: string } }> }

const asText = (content: unknown): string =>
  typeof content === 'string' ? content : content == null ? '' : JSON.stringify(content)

/** Derive the turn's tool record from the request messages: everything since the
 *  last user message. Works for any OpenAI-style tool loop, any model. */
export function extractToolRecord(messages: Msg[]): ToolRecord {
  let start = 0
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i]?.role === 'user') { start = i + 1; break }
  }
  const turn = messages.slice(start)
  const backingTools: string[] = []
  const results: string[] = []
  for (const m of turn) {
    if (m.role === 'assistant' && Array.isArray(m.tool_calls)) {
      for (const tc of m.tool_calls) {
        const name = tc.function?.name
        if (name && !NONBACKING.has(name)) backingTools.push(name)
      }
    }
    if (m.role === 'tool' || m.role === 'function') results.push(asText(m.content))
  }
  const resultsText = results.join('\n')
  return {
    backingTools,
    resultsText: resultsText.length > RESULTS_CAP ? '' : resultsText,
    anyError: TRANSPORT_ERROR_RE.test(resultsText),
    overflowed: resultsText.length > RESULTS_CAP,
  }
}

// ── Heuristics (ported faithfully from confab-guard) ─────────────────────────

const ARTIFACT =
  `draft|e-?mails?|messages?|repl(?:y|ies)|events?|meetings?|invites?|calendar|tickets?|work items?|tasks?|records?|contacts?|compan(?:y|ies)|deals?|opportunit(?:y|ies)|notes?|documents?|docs?|pages?|wiki|filters?|labels?|broadcasts?|posts?|comments?|files?|folders?|spreadsheets?|schedules?|bookings?|reminders?`
const DONE_VERB =
  `created|made|drafted|set up|saved|sent|queued|posted|added|updated|edited|filed|logged|scheduled|booked|archived|moved|assigned|uploaded|published|submitted|labell?ed|starred|deleted|removed|put together|wrote up|prepared|dropped`
const CLAIM_VERB_ART = new RegExp(`\\b(?:${DONE_VERB})\\b[^.!?\\n]{0,40}?\\b(?:${ARTIFACT})\\b`, 'i')
const CLAIM_ART_STATE = new RegExp(
  `\\b(?:${ARTIFACT})\\b[^.!?\\n]{0,40}?\\b(?:is|are|has been|have been|'s)\\b[^.!?\\n]{0,30}?\\b(?:created|saved|sent|done|ready|in your (?:drafts?|calendar|inbox)|on your (?:board|calendar))\\b`,
  'i',
)
const CLAIM_LANDED = /\b(?:in|sitting in|added to|on)\s+your\s+(?:drafts?|calendar|board|inbox)\b/i
const FUTURE =
  /\b(?:I'?ll|I will|I can|I could|I'?d|I am going to|I'?m going to|going to|want me to|shall I|should I|would you like|do you want|ready to|happy to|I plan to|next I'?ll|let me know if)\b/i
const SENT_SPLIT = /(?<=[.!?\n])\s+/

function firstSentence(text: string, test: (s: string) => boolean): string | null {
  for (const sent of text.split(SENT_SPLIT)) {
    const s = sent.trim()
    if (!s || FUTURE.test(s)) continue
    if (test(s)) return s
  }
  return null
}

const claimsCompletedAction = (text: string) =>
  firstSentence(text, (s) => CLAIM_VERB_ART.test(s) || CLAIM_ART_STATE.test(s) || CLAIM_LANDED.test(s))

const SUBJECT =
  `server|service|endpoint|API|MCP|tool|connection|backend|host|database|it|they|things`
const OUTAGE_STATE =
  `down|offline|unreachable|unavailable|not responding|won'?t respond|timing out|timed out|erroring|throwing (?:connection )?errors|stuck(?: in a recovery loop)?|in a recovery loop|flaky|went (?:down|unreachable|offline)|having (?:issues|problems|trouble)|acting up|recovering|coming back up|back up`
const OUTAGE_PATTERNS = [
  new RegExp(`\\b(?:${SUBJECT})\\b[^.!?\\n]{0,40}?\\b(?:is|are|was|were|seems?|appears?|keeps?|been|being|currently|temporarily|still|right now|going|went)\\b[^.!?\\n]{0,30}?\\b(?:${OUTAGE_STATE})\\b`, 'i'),
  new RegExp(`\\b(?:${SUBJECT})\\b[^.!?\\n]{0,20}?\\b(?:${OUTAGE_STATE})\\b`, 'i'),
  /\b(?:can'?t|cannot|could ?n'?t|unable to|failed to|won'?t let me)\b[^.!?\n]{0,30}?\b(?:reach|connect(?: to)?|access|complete|proceed|touch|do that|delete)\b/i,
  /\bconnection (?:errors?|issues?|problems?|refused|reset|timed? ?out)\b|\b50[234]\b|\bbad gateway\b|\bgateway timeout\b/i,
  /\bauto[- ]?retry\b|\bretry (?:should be|will be|is) (?:available|possible)\b|\btry again in (?:about |~)?\d+\s*(?:second|minute|sec|min)s?\b|\bavailable (?:again )?in (?:about |~)?\d+\s*(?:second|minute|sec|min)s?\b/i,
]
const claimsInfraFailure = (text: string) => firstSentence(text, (s) => OUTAGE_PATTERNS.some((p) => p.test(s)))

// Transport/availability errors in a tool RESULT ground a real outage claim.
// App errors ("document not found") deliberately don't match.
const TRANSPORT_ERROR_RE =
  /econnrefused|econnreset|etimedout|enotfound|ehostunreach|connection (?:refused|reset|error|timed? ?out|aborted)|could ?n'?t connect|failed to (?:connect|fetch|reach)|network (?:error|unreachable)|socket hang up|timeout|timed out|unreachable|service unavailable|bad gateway|gateway timeout|\b50[234]\b|\b-32000\b|\bfetch failed\b|server (?:error|is down|unavailable)|temporarily unavailable/i

const URL_RE = /https?:\/\/[^\s<>"'`)\]}]+/gi
const UUID_RE = /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi

function urlHostTail(url: string): { host: string; tail: string } {
  let u = url.trim().replace(/[.,;:!?]+$/, '')
  u = u.replace(/^https?:\/\//i, '').split('#')[0]!.split('?')[0]!
  const slash = u.indexOf('/')
  const host = (slash === -1 ? u : u.slice(0, slash)).toLowerCase()
  return { host, tail: u.toLowerCase().replace(/\/+$/, '') }
}

/** Policed URLs (internal-host with a path) + UUIDs found in text, lowercased. */
function extractRefs(text: string, policedHosts: string[]): string[] {
  const refs: string[] = []
  for (const m of text.matchAll(URL_RE)) {
    const { host, tail } = urlHostTail(m[0])
    const policed = policedHosts.some((h) => host === h.toLowerCase() || host.endsWith(h.toLowerCase()))
    if (policed && tail.includes('/')) refs.push(tail)
  }
  for (const m of text.matchAll(UUID_RE)) refs.push(m[0].toLowerCase())
  return [...new Set(refs)]
}

function ungroundedRefs(text: string, haystack: string, policedHosts: string[]): string[] {
  const hay = haystack.toLowerCase().replace(/https?:\/\//g, '')
  return extractRefs(text, policedHosts).filter((r) => !hay.includes(r))
}

// ── Run ──────────────────────────────────────────────────────────────────────

export interface Finding {
  check: GuardCheck
  severity: 'low' | 'medium' | 'high'
  message: string
  snippet: string
}

export interface GuardContext {
  answer: string
  toolRecord: ToolRecord
  userMessage: string
}

/** Run the enabled checks. Pure — returns findings, mutates nothing. */
export function runGuardrails(ctx: GuardContext, config: GuardConfig): Finding[] {
  const out: Finding[] = []
  const { answer, toolRecord: tr } = ctx
  if (!answer) return out

  // Fabricated outage is orthogonal — check it whenever no real error occurred.
  if (config.checks.fabricated_outage && !tr.anyError) {
    const outage = claimsInfraFailure(answer)
    if (outage) out.push({ check: 'fabricated_outage', severity: 'high', message: 'Claims an outage/failure, but no tool returned an error this turn.', snippet: outage.slice(0, 240) })
  }

  if (tr.backingTools.length === 0) {
    // No external tool ran → a completion claim can't be backed.
    if (config.checks.zero_tool_claim) {
      const sentence = claimsCompletedAction(answer)
      if (sentence) out.push({ check: 'zero_tool_claim', severity: 'high', message: 'Claims a completed action, but no external tool ran this turn.', snippet: sentence.slice(0, 240) })
    }
  } else if (config.checks.ungrounded_ref && !tr.overflowed) {
    // Tools ran → links/ids in the answer should be grounded in their results.
    const haystack = `${tr.resultsText}\n${ctx.userMessage}`
    const ungrounded = ungroundedRefs(answer, haystack, config.policedHosts)
    if (ungrounded.length) out.push({ check: 'ungrounded_ref', severity: 'medium', message: 'Cites link(s)/id(s) that did not appear in any tool result this turn — may be fabricated.', snippet: ungrounded.slice(0, 8).join(', ') })
  }
  return out
}

/** A human-facing caveat for annotate mode (appended out-of-band, never re-fed
 *  into the model's context). */
export function caveatFor(findings: Finding[]): string {
  if (!findings.length) return ''
  const lines = findings.map((f) => `- **${f.check.replace(/_/g, ' ')}:** ${f.message}${f.snippet ? ` (${f.snippet})` : ''}`)
  return `\n\n---\n⚠️ **Unverified — confab guard flagged this response:**\n${lines.join('\n')}\nVerify before relying on it.`
}

// ── Findings store + observability ───────────────────────────────────────────

export async function recordFindings(
  findings: Finding[],
  meta: { caller: string; model: string; endpoint: string | null; mode: GuardMode },
): Promise<void> {
  if (!findings.length) return
  const sql = await db()
  for (const f of findings) {
    await sql`
      insert into guard_findings (caller, model, endpoint, mode, check_type, severity, message, snippet)
      values (${meta.caller}, ${meta.model}, ${meta.endpoint}, ${meta.mode}, ${f.check}, ${f.severity}, ${f.message}, ${f.snippet})
    `
  }
}

export interface GuardFindingRow {
  id: string
  caller: string
  model: string
  endpoint: string | null
  mode: string
  check: string
  severity: string
  message: string
  snippet: string
  createdAt: string
}

export async function listFindings(limit = 100): Promise<GuardFindingRow[]> {
  const sql = await db()
  return (await sql`
    select id, caller, model, endpoint, mode, check_type as "check", severity, message, snippet, created_at as "createdAt"
    from guard_findings order by created_at desc limit ${Math.min(Math.max(limit, 1), 500)}
  `) as unknown as GuardFindingRow[]
}

export async function guardStats(): Promise<{ total: number; byCheck: Record<string, number> }> {
  const sql = await db()
  const rows = (await sql`select check_type as check, count(*)::int as n from guard_findings group by check_type`) as unknown as Array<{ check: string; n: number }>
  const byCheck: Record<string, number> = {}
  let total = 0
  for (const r of rows) { byCheck[r.check] = r.n; total += r.n }
  return { total, byCheck }
}

/** The one-call entry point for the gateway: run guards (if enabled) on a
 *  finished completion, record findings, and return any annotate-mode caveat. */
export async function guardCompletion(input: {
  answer: string
  messages: unknown[]
  caller: string
  model: string
  endpoint: string | null
}): Promise<{ findings: Finding[]; caveat: string; mode: GuardMode }> {
  const config = await getGuardConfig()
  if (config.mode === 'off' || !input.answer) return { findings: [], caveat: '', mode: config.mode }
  const messages = input.messages as Msg[]
  const toolRecord = extractToolRecord(messages)
  let userMessage = ''
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i]?.role === 'user') { userMessage = asText(messages[i]!.content); break }
  }
  const findings = runGuardrails({ answer: input.answer, toolRecord, userMessage }, config)
  await recordFindings(findings, { caller: input.caller, model: input.model, endpoint: input.endpoint, mode: config.mode }).catch(() => {})
  const caveat = config.mode === 'annotate' || config.mode === 'strict' ? caveatFor(findings) : ''
  return { findings, caveat, mode: config.mode }
}
