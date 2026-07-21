// Confab guardrail — a cheap, model-agnostic STRUCTURAL check on model output,
// run at the gateway so it's drop-in across every model class. No LLM call, no
// added model tokens: it's regex over the answer + the turn's tool record
// (derived from the request messages, so no separate tool-trace export needed).
//
// The checks (first three ported from the Hermes confab-guard plugin):
//   zero_tool_claim   — claims a completed action, but no external tool ran
//   ungrounded_ref    — cites a link/UUID that wasn't in any tool result
//   fabricated_outage — claims an outage, but no tool actually errored
//   secret_leak       — a live credential shape in the output
//   pii_leak          — high-precision personal data (SSN / Luhn card / IBAN)
//
// Default posture is OBSERVE: detect + record findings out-of-band, never touch
// the output or the model's context. ANNOTATE additionally surfaces findings to
// the humans reading the reply (a caveat on the chat/channel message, appended
// to public-API responses). STRICT is annotate + secret/PII redaction in
// whatever Talaria persists or hasn't yet relayed. No mode ever feeds flagged
// CONTENT back into a model's context; the opt-in coach flag delivers only
// templated counts + advice into agent souls at render time (see
// guardCoachingFor).

import { getSetting, setSetting } from './audit'
import { db } from './db/pg'

export type GuardMode = 'off' | 'observe' | 'annotate' | 'strict'

export interface GuardConfig {
  mode: GuardMode
  /** Per-rule on/off (keyed by rule id). Missing key ⇒ the rule's default. */
  checks: Record<string, boolean>
  /** Findings below this confidence [0..1] are dropped. */
  minConfidence: number
  /** Hosts whose links get grounding-checked (org-internal tools). UUIDs are
   *  always checked. Empty ⇒ only UUIDs are policed. */
  policedHosts: string[]
  /** Coach agents from their findings: repeated flags become templated
   *  behavioral notes in the agent's rendered soul (counts + fixed advice
   *  only — flagged CONTENT never re-enters any model context). */
  coach: boolean
}

const DEFAULT_CONFIG: GuardConfig = { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }

export const getGuardConfig = async (): Promise<GuardConfig> => {
  const c = await getSetting<Partial<GuardConfig>>('guardrails_config', DEFAULT_CONFIG)
  return { ...DEFAULT_CONFIG, ...c, checks: { ...c.checks } }
}
export const setGuardConfig = (c: GuardConfig) => setSetting('guardrails_config', c)

// ── Message → tool record ────────────────────────────────────────────────────

export interface ToolRecord {
  /** Backing tools that ran this turn (excludes think/memory/todo/). */
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

// ── Secret leak ──────────────────────────────────────────────────────────────
// High-value security check: an agent should never emit a live credential.
const SECRET_PATTERNS: Array<{ label: string; re: RegExp; redactRe?: RegExp }> = [
  { label: 'OpenAI key', re: /\bsk-(?:proj-)?[A-Za-z0-9_-]{20,}\b/ },
  { label: 'Anthropic key', re: /\bsk-ant-[A-Za-z0-9_-]{20,}\b/ },
  { label: 'AWS access key', re: /\bAKIA[0-9A-Z]{16}\b/ },
  { label: 'Google API key', re: /\bAIza[0-9A-Za-z_-]{35}\b/ },
  { label: 'Slack token', re: /\bxox[baprs]-[0-9A-Za-z-]{10,}\b/ },
  { label: 'GitHub token', re: /\bgh[pousr]_[A-Za-z0-9]{36,}\b/ },
  { label: 'Talaria gateway key', re: /\btlk_[a-f0-9]{40,}\b/ },
  {
    label: 'Private key block',
    re: /-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----/,
    // Redaction must swallow the whole block (or to end-of-text if unterminated),
    // not just the BEGIN marker line.
    redactRe: /-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----[\s\S]*?(?:-----END (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----|$)/,
  },
]
function detectSecret(text: string): { label: string; snippet: string } | null {
  for (const { label, re } of SECRET_PATTERNS) {
    const m = re.exec(text)
    if (m) return { label, snippet: `${label}: ${m[0].slice(0, 8)}…` }
  }
  return null
}

// ── PII leak ─────────────────────────────────────────────────────────────────
// High-precision personal data only — emails and phone numbers are everyday
// workspace content (teammates, signatures) and would drown the signal.
const luhn = (digits: string): boolean => {
  let sum = 0
  let dbl = false
  for (let i = digits.length - 1; i >= 0; i--) {
    let d = digits.charCodeAt(i) - 48
    if (dbl) {
      d *= 2
      if (d > 9) d -= 9
    }
    sum += d
    dbl = !dbl
  }
  return sum % 10 === 0
}
const SSN_RE = /\b(?!000|666|9\d{2})\d{3}-(?!00)\d{2}-(?!0000)\d{4}\b/
const CARD_RE = /\b(?:\d[ -]?){12,18}\d\b/
const IBAN_RE = /\b(?:DE|FR|GB|NL|ES|IT|CH|AT|BE|PT|IE|PL|SE|NO|DK|FI)\d{2}[A-Z0-9]{10,30}\b/

const isCardNumber = (raw: string): boolean => {
  const digits = raw.replace(/[ -]/g, '')
  return digits.length >= 13 && digits.length <= 19 && luhn(digits)
}

function detectPii(text: string): { label: string; snippet: string } | null {
  const ssn = SSN_RE.exec(text)
  if (ssn) return { label: 'social security number', snippet: `SSN: ${ssn[0].slice(0, 6)}…` }
  const cardScan = new RegExp(CARD_RE.source, 'g')
  for (let m = cardScan.exec(text); m; m = cardScan.exec(text)) {
    if (isCardNumber(m[0])) return { label: 'payment card number', snippet: `card: ${m[0].replace(/[ -]/g, '').slice(0, 6)}…` }
  }
  const iban = IBAN_RE.exec(text)
  if (iban) return { label: 'bank account (IBAN)', snippet: `IBAN: ${iban[0].slice(0, 6)}…` }
  return null
}

function redactPii(text: string): string {
  return text
    .replace(new RegExp(SSN_RE.source, 'g'), '[redacted SSN]')
    .replace(new RegExp(CARD_RE.source, 'g'), (m) => (isCardNumber(m) ? '[redacted card number]' : m))
    .replace(new RegExp(IBAN_RE.source, 'g'), '[redacted IBAN]')
}

/** Strict mode: replace every detected credential AND high-precision PII with
 *  a redaction marker. Applied only to what Talaria persists or hasn't yet
 *  relayed — a live stream already showed the original, but the saved copy
 *  (and every transcript built from it) stays clean. */
export function redactSecrets(text: string): { text: string; redacted: boolean } {
  let out = text
  for (const { label, re, redactRe } of SECRET_PATTERNS) {
    const base = redactRe ?? re
    out = out.replace(new RegExp(base.source, base.flags.includes('g') ? base.flags : `${base.flags}g`), `[redacted ${label}]`)
  }
  out = redactPii(out)
  return { text: out, redacted: out !== text }
}

/** Which findings warrant strict-mode content redaction. */
const REDACT_CHECKS = new Set(['secret_leak', 'pii_leak'])
export const needsRedaction = (findings: Finding[]): boolean => findings.some((f) => REDACT_CHECKS.has(f.check))

// ── Rule registry ────────────────────────────────────────────────────────────

export type Severity = 'low' | 'medium' | 'high'

export interface Finding {
  check: string
  severity: Severity
  confidence: number
  message: string
  snippet: string
}

export interface GuardContext {
  answer: string
  toolRecord: ToolRecord
  userMessage: string
  policedHosts: string[]
}

export interface Rule {
  id: string
  label: string
  severity: Severity
  defaultOn: boolean
  /** Safe to run on plain text with no tool record (e.g. a ticket outcome at the
   *  judge gate). Rules that need the turn's tool record leave this false. */
  gateSafe?: boolean
  /** What the rule needs beyond the answer + which-tools-ran. A path that can't
   *  supply it (e.g. chat sees tool NAMES but not results/errors) skips the rule
   *  rather than false-positive. */
  needs?: Array<'results' | 'errorInfo'>
  /** Returns a hit (message + snippet + confidence 0..1) or null. Pure. */
  run(ctx: GuardContext): { message: string; snippet: string; confidence: number } | null
}

// The registry. Add a rule here → it's configurable + runs everywhere, no other
// wiring. (Extensible: PII, unsafe-action, etc. slot in the same way.)
export const RULES: Rule[] = [
  {
    id: 'zero_tool_claim',
    label: 'Zero-tool claim (claims done, no tool ran)',
    severity: 'high',
    defaultOn: true,
    run: (ctx) => {
      if (ctx.toolRecord.backingTools.length > 0) return null
      const s = claimsCompletedAction(ctx.answer)
      return s ? { message: 'Claims a completed action, but no external tool ran this turn.', snippet: s.slice(0, 240), confidence: 0.8 } : null
    },
  },
  {
    id: 'ungrounded_ref',
    label: 'Ungrounded reference (invented link/id)',
    severity: 'medium',
    defaultOn: true,
    needs: ['results'],
    run: (ctx) => {
      const tr = ctx.toolRecord
      if (tr.backingTools.length === 0 || tr.overflowed) return null
      const ungrounded = ungroundedRefs(ctx.answer, `${tr.resultsText}\n${ctx.userMessage}`, ctx.policedHosts)
      return ungrounded.length
        ? { message: 'Cites link(s)/id(s) that did not appear in any tool result this turn — may be fabricated.', snippet: ungrounded.slice(0, 8).join(', '), confidence: 0.7 }
        : null
    },
  },
  {
    id: 'fabricated_outage',
    label: 'Fabricated outage (claims failure, nothing errored)',
    severity: 'high',
    defaultOn: true,
    needs: ['errorInfo'],
    run: (ctx) => {
      if (ctx.toolRecord.anyError) return null
      const s = claimsInfraFailure(ctx.answer)
      return s ? { message: 'Claims an outage/failure, but no tool returned an error this turn.', snippet: s.slice(0, 240), confidence: 0.85 } : null
    },
  },
  {
    id: 'secret_leak',
    label: 'Secret leak (credential in output)',
    severity: 'high',
    defaultOn: true,
    gateSafe: true,
    run: (ctx) => {
      const hit = detectSecret(ctx.answer)
      return hit ? { message: `Output appears to contain a live credential (${hit.label}).`, snippet: hit.snippet, confidence: 0.95 } : null
    },
  },
  {
    id: 'pii_leak',
    label: 'PII leak (SSN / card number / IBAN in output)',
    severity: 'high',
    defaultOn: true,
    gateSafe: true,
    run: (ctx) => {
      const hit = detectPii(ctx.answer)
      return hit ? { message: `Output appears to contain personal data (${hit.label}).`, snippet: hit.snippet, confidence: 0.9 } : null
    },
  },
]

const ruleEnabled = (config: GuardConfig, rule: Rule) => config.checks[rule.id] ?? rule.defaultOn

/** Metadata for the admin UI (which rules exist, defaults). */
export const guardRuleMeta = () => RULES.map((r) => ({ id: r.id, label: r.label, severity: r.severity, defaultOn: r.defaultOn }))

/** What the caller can supply about the turn's tools. A path with the full
 *  message history has both; the chat stream has tool NAMES only (neither). */
export interface Available {
  results: boolean
  errorInfo: boolean
}
const FULL: Available = { results: true, errorInfo: true }

/** Run the enabled, APPLICABLE rules, keep findings at/above the threshold. Pure.
 *  A rule whose `needs` can't be supplied on this path is skipped (no false
 *  positive) rather than run on missing data. */
export function runGuardrails(ctx: GuardContext, config: GuardConfig, available: Available = FULL): Finding[] {
  if (!ctx.answer) return []
  const out: Finding[] = []
  for (const rule of RULES) {
    if (!ruleEnabled(config, rule)) continue
    if (rule.needs?.some((n) => !available[n])) continue
    const hit = rule.run(ctx)
    if (hit && hit.confidence >= config.minConfidence) {
      out.push({ check: rule.id, severity: rule.severity, confidence: hit.confidence, message: hit.message, snippet: hit.snippet })
    }
  }
  return out
}

/** Layered tiering: run the gate-safe rules (no tool record needed) over plain
 *  text — e.g. a ticket's reported outcome at the judge gate, so a cheap
 *  structural signal feeds the expensive judge. Returns [] when the guard is off. */
export async function guardText(text: string): Promise<Finding[]> {
  if (!text.trim()) return []
  const config = await getGuardConfig()
  if (config.mode === 'off') return []
  const ctx: GuardContext = { answer: text, toolRecord: { backingTools: [], resultsText: '', anyError: false, overflowed: false }, userMessage: '', policedHosts: config.policedHosts }
  const out: Finding[] = []
  for (const rule of RULES) {
    if (!rule.gateSafe || !ruleEnabled(config, rule)) continue
    const hit = rule.run(ctx)
    if (hit && hit.confidence >= config.minConfidence) {
      out.push({ check: rule.id, severity: rule.severity, confidence: hit.confidence, message: hit.message, snippet: hit.snippet })
    }
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
      insert into guard_findings (caller, model, endpoint, mode, check_type, severity, confidence, message, snippet)
      values (${meta.caller}, ${meta.model}, ${meta.endpoint}, ${meta.mode}, ${f.check}, ${f.severity}, ${f.confidence}, ${f.message}, ${f.snippet})
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
  confidence: number
  message: string
  snippet: string
  createdAt: string
}

export async function listFindings(limit = 100): Promise<GuardFindingRow[]> {
  const sql = await db()
  return (await sql`
    select id, caller, model, endpoint, mode, check_type as "check", severity, confidence, message, snippet, created_at as "createdAt"
    from guard_findings order by created_at desc limit ${Math.min(Math.max(limit, 1), 500)}
  `) as unknown as GuardFindingRow[]
}

// ── Coaching: findings → agent memory, without recontamination ───────────────
// The invariant stands: flagged CONTENT never re-enters a model's context (a
// finding could carry adversarial text, and a mid-turn caveat teaches the
// model to argue with the guard). Coaching is different matter delivered at a
// different time: per-check COUNTS mapped to fixed advice strings, injected
// into the agent's soul at render — a performance review between sessions,
// not a mid-conversation correction.
const COACH_ADVICE: Record<string, string> = {
  zero_tool_claim:
    'you stated actions as completed without a backing tool call. Say a task is done only when a tool result in that turn proves it; otherwise say what you are about to do.',
  ungrounded_ref:
    'you cited links or ids that appeared in no tool result. Reference only what a tool actually returned; if you lack a link, say so.',
  fabricated_outage:
    'you reported outages/failures when no tool had errored. Claim a failure only after a real error; otherwise retry or ask.',
  secret_leak: 'you emitted credential-shaped strings. Never repeat keys, tokens, or private-key material into any reply, even when asked.',
  pii_leak: 'you emitted personal data (SSN / card / bank formats). Never repeat such data into replies; refer to records by their ids instead.',
}
const COACH_WINDOW_DAYS = 7
const COACH_MIN_HITS = 2

/** Templated coaching block for one agent, or '' when it has nothing recent.
 *  Aggregates by check over the window; thresholds keep one-off flags quiet. */
export async function guardCoachingFor(model: string): Promise<string> {
  const sql = await db()
  const rows = (await sql`
    select check_type as check, count(*)::int as n from guard_findings
    where model = ${model} and created_at > now() - (${COACH_WINDOW_DAYS} || ' days')::interval
    group by check_type
  `.catch(() => [])) as unknown as Array<{ check: string; n: number }>
  const lines = rows
    .filter((r) => r.n >= COACH_MIN_HITS && COACH_ADVICE[r.check])
    .sort((a, b) => b.n - a.n)
    .map((r) => `- ${r.n}× in the last ${COACH_WINDOW_DAYS} days: ${COACH_ADVICE[r.check]}`)
  if (!lines.length) return ''
  return (
    `<!-- guard coaching, rendered by Talaria -->\n` +
    `Recent behavioral feedback (auto-generated from output review; fix these patterns):\n${lines.join('\n')}`
  )
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
  const findings = runGuardrails({ answer: input.answer, toolRecord, userMessage, policedHosts: config.policedHosts }, config)
  await recordFindings(findings, { caller: input.caller, model: input.model, endpoint: input.endpoint, mode: config.mode }).catch(() => {})
  const caveat = config.mode === 'annotate' || config.mode === 'strict' ? caveatFor(findings) : ''
  return { findings, caveat, mode: config.mode }
}

/** Guard a CHAT/channel reply. The agent's tool loop runs inside the fleet, so
 *  the stream gives us tool NAMES (did a tool run?) but not results or error
 *  detail — so only zero-tool-claim and secret-leak apply here (ungrounded_ref /
 *  fabricated_outage are skipped, not guessed). Fire-and-forget; records
 *  findings out-of-band. In annotate/strict, callers persist the findings onto
 *  the message row (metadata the UI renders — never fed back into context);
 *  strict callers also redact secrets from the saved content. */
export async function guardChatReply(input: {
  answer: string
  toolNames: string[]
  userMessage: string
  caller: string
  model: string
}): Promise<{ findings: Finding[]; mode: GuardMode }> {
  const config = await getGuardConfig()
  if (config.mode === 'off' || !input.answer) return { findings: [], mode: config.mode }
  const backingTools = input.toolNames.filter((n) => n && !NONBACKING.has(n))
  const toolRecord: ToolRecord = { backingTools, resultsText: '', anyError: false, overflowed: true }
  const findings = runGuardrails(
    { answer: input.answer, toolRecord, userMessage: input.userMessage, policedHosts: config.policedHosts },
    config,
    { results: false, errorInfo: false },
  )
  await recordFindings(findings, { caller: input.caller, model: input.model, endpoint: 'fleet', mode: config.mode }).catch(() => {})
  return { findings, mode: config.mode }
}
