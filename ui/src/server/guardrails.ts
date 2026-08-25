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
// WHAT THE MODE GOVERNS: DISCLOSURE. One org-wide setting, and what it decides
// is how loudly a finding is told to the people reading model output.
//   off       nothing runs, nothing is recorded.
//   observe   (the default) detect + record findings out-of-band. The reader of
//             the reply is told nothing.
//   annotate  additionally surface findings to the humans reading the reply — a
//             caveat on the chat/channel message, appended to public-API
//             responses (see caveatFor).
//   strict    annotate, plus: the RELAY call sites — chat persistence, channel
//             replies, the public gateway route, the Muse stream — scrub
//             secrets and PII out of what they persist or have not yet handed
//             on.
//
// WHAT THE MODE DOES NOT GOVERN: a harness's own `redact`. A harness declares
// `guard: { redact: true }` (harness/define.ts) about ITS OWN VALUE — a chat
// title, a document summary, an assistant memory, a judge's verdict on a ticket
// — and `runHarness` honors that declaration in every mode except `off`. That is
// the intended contract, not strict-mode behavior leaking downward. The two
// knobs answer different questions: the mode answers "how much does this org
// want to hear about its models", and `redact` answers "may this particular
// credential end up in a row Talaria wrote itself". The second question has one
// answer at every mode, which is no — 20 of the 23 harnesses declare `redact`
// and a default install runs in `observe`, so reading the mode there would mean
// the out-of-the-box configuration is the one that writes live credentials into
// the rows it keeps forever.
//
// Redaction is not a way to hide a finding, and that is what keeps the two
// consistent: the finding is recorded either way, so the disclosure the mode
// governs is unaffected by whether the stored value was scrubbed.
//
// WHAT NEITHER KNOB GOVERNS: whether the span was the MODEL'S at all. A shape
// that already appears in the turn's INPUT is the user's own data coming back,
// so it is not evidence about the model — and each rule declares for itself
// what that changes, on `Rule.groundable`. The split as it stands, which the
// grounding section below argues at length:
//
//   pii_leak    'finding+redaction'  a grounded hit files nothing AND is not
//                                    rewritten. The detectors are shape matches
//                                    over strings business data legitimately
//                                    has (Luhn-valid IMEIs, XXX-XX-XXXX part
//                                    numbers), so a grounded hit is most likely
//                                    not PII, and scrubbing it makes a summary
//                                    disagree with the document it summarized.
//   secret_leak 'finding'            a grounded hit files nothing but IS still
//                                    redacted out of the copy Talaria writes.
//                                    `ghp_` + 36 chars is not an order number:
//                                    a grounded secret is a real key that was
//                                    really pasted, and the artifact's copy is a
//                                    NEW copy with a longer life.
//   the other three                  no `groundable`, so never grounded: a
//                                    zero-tool claim is a sentence the model
//                                    wrote, and finding it in the input would
//                                    mean the model quoted the user back, which
//                                    is not exoneration.
//
// GROUNDING IS A PROPERTY OF THE CALL, NOT OF THE ORG. A caller supplies the
// turn's input (`GuardContext.inputText`, `guardText`'s and `redactSecrets`'s
// second argument, `groundingTextOf` for a message list) and grounds; a caller
// that supplies nothing grounds nothing and behaves exactly as it did before
// grounding existed. THE TWO HALVES MUST BE HANDED THE SAME MATERIAL: a caller
// that grounds its findings and then redacts without grounding gets the worse
// of both — the finding is correctly suppressed and the user's own data is
// rewritten anyway.
//
// THE CARDINAL INVARIANT, in every mode: flagged CONTENT never re-enters a
// model's context. The opt-in coach flag delivers only templated counts + fixed
// advice into agent souls at render time (see guardCoachingFor).

import { SECRET_PATTERNS } from './secret-vault'
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

/** THE WHOLE INPUT of a request, for `GuardContext.inputText` — everything that
 *  was put in front of the model this turn EXCEPT what a model said.
 *
 *  System prompts and TOOL RESULTS are in deliberately: a customer's order
 *  number reaches a model through the rendered ticket or through a CRM lookup at
 *  least as often as through the user's own sentence, and grounding is asking
 *  "did the model make this up", not "did the human type it".
 *
 *  ASSISTANT TURNS ARE OUT, and that is the load-bearing line rather than
 *  tidiness. Grounding material must be material the model did not write, or a
 *  model launders an invented card by emitting it twice — which is not
 *  hypothetical: `runHarness` REPAIRS by appending the model's own rejected
 *  reply to the message list and re-asking, so a caller that grounded against
 *  the sent history would ground the second attempt against the first one's
 *  confabulation. Every caller passes its whole message list through here rather
 *  than deciding this again.
 *
 *  The tail is kept when the history is enormous, because a turn's own material
 *  is at the end of its prompt and an unbounded normalize on every guarded reply
 *  is a cost with no ceiling. Losing the head only ever means grounding LESS,
 *  which is the pre-grounding behavior. */
const INPUT_CAP = 200_000
export function groundingTextOf(messages: Array<{ role?: string; content?: unknown }>): string {
  const joined = messages
    .filter((m) => m.role !== 'assistant')
    .map((m) => asText(m.content))
    .join('\n')
  return joined.length > INPUT_CAP ? joined.slice(joined.length - INPUT_CAP) : joined
}

// ── Heuristics (ported faithfully from confab-guard) ─────────────────────────

// THE VOCABULARY IS THE RULE. `zero_tool_claim` is a phrase matcher, so what it
// can SEE is exactly this list — and the fitness corpus found the hole by
// accident: a model asked to write a standup answered "finished PLAT-118, closed
// t-77, merged the migration PR" with no tool having run, and the guard said
// nothing. Not because the claim was subtle, but because `closed`, `finished`
// and `merged` were not words it knew.
//
// WHAT GOES IN, AND WHAT DELIBERATELY DOES NOT. An artifact here must be a thing
// that CANNOT EXIST WITHOUT A SYSTEM ACTION — a ticket, a deploy, a refund. A
// model that says "I put together a summary" has put together a summary: it is
// in the reply. Adding summary-shaped nouns would flag models for writing prose,
// which is the one thing they are unambiguously allowed to do, and that class of
// false positive is worse than a missed claim because it fires on every honest
// answer rather than on a rare dishonest one.
//
// SAME TEST FOR VERBS: `ran` is missing on purpose. "I ran into a problem while
// drafting the email" pairs `ran` with `email` inside the window and would fire
// on a model reporting a difficulty — the opposite of a confabulation.
const ARTIFACT =
  `draft|e-?mails?|messages?|repl(?:y|ies)|events?|meetings?|invites?|calendar|tickets?|work items?|tasks?|records?|contacts?|compan(?:y|ies)|deals?|opportunit(?:y|ies)|notes?|documents?|docs?|pages?|wiki|filters?|labels?|broadcasts?|posts?|comments?|files?|folders?|spreadsheets?|schedules?|bookings?|reminders?` +
  // Engineering and workspace objects a Talaria agent actually acts on. Every
  // one of these requires a tool call to come into existence or to change.
  `|pull requests?|PRs?|branch(?:es)?|commits?|deploys?|deployments?|releases?|migrations?|rollbacks?` +
  `|refunds?|invoices?|charges?|subscriptions?|payments?` +
  `|boards?|columns?|sprints?|milestones?|dependenc(?:y|ies)|watchers?|reviews?|approvals?` +
  `|channels?|threads?|workflows?|integrations?|webhooks?`
const DONE_VERB =
  `created|made|drafted|set up|saved|sent|queued|posted|added|updated|edited|filed|logged|scheduled|booked|archived|moved|assigned|uploaded|published|submitted|labell?ed|starred|deleted|removed|put together|wrote up|prepared|dropped` +
  // Completion verbs. These are how an agent reports finished WORK, which is the
  // claim that matters most and the one the corpus caught us missing.
  `|closed|completed|finished|resolved|fixed|merged|pushed|deployed|shipped|released|reverted|rolled back|restored` +
  `|renamed|tagged|linked|attached|shared|invited|granted|revoked|approved|rejected|triaged|reassigned|escalated|marked` +
  `|processed|issued|refunded|cancell?ed|imported|exported|synced|migrated|pinned`
const CLAIM_VERB_ART = new RegExp(`\\b(?:${DONE_VERB})\\b[^.!?\\n]{0,40}?\\b(?:${ARTIFACT})\\b`, 'i')
const CLAIM_ART_STATE = new RegExp(
  `\\b(?:${ARTIFACT})\\b[^.!?\\n]{0,40}?\\b(?:is|are|has been|have been|'s)\\b[^.!?\\n]{0,30}?\\b(?:created|saved|sent|done|ready|in your (?:drafts?|calendar|inbox)|on your (?:board|calendar))\\b`,
  'i',
)
const CLAIM_LANDED = /\b(?:in|sitting in|added to|on)\s+your\s+(?:drafts?|calendar|board|inbox)\b/i
/** A sentence that OFFERS rather than claims. Skipped by every claim heuristic,
 *  because an agent with no tools saying "I can close the ticket once you
 *  confirm" is doing exactly the right thing.
 *
 *  `I can` AND `I could` CARRY A NEGATION EXCEPTION, and it was a real evasion.
 *  "The gateway is rate limited, so I could not get an answer" is past-tense
 *  INABILITY, not a future offer — but `I could` matched, the whole sentence was
 *  skipped as an offer, and the fabricated outage in the first half of it went
 *  unscored. Any claim a model appends "so I couldn't finish" to was invisible,
 *  which is the shape a model actually writes when it is explaining itself. */
const FUTURE =
  /\b(?:I'?ll|I will|I can(?!not\b|'?t\b|\s+not\b)|I could(?!n'?t\b|\s+not\b)|I'?d|I am going to|I'?m going to|going to|want me to|shall I|should I|would you like|do you want|ready to|happy to|I plan to|next I'?ll|let me know if)\b/i
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

// SAME WIDENING, SAME REASON. The pieces an agent in this product actually
// blames — the gateway, the provider, the search index, the queue — were not
// subjects the rule knew, so "the provider is rate limited" (when nothing
// errored) read as ordinary prose.
const SUBJECT =
  `server|service|endpoint|API|MCP|tool|connection|backend|host|database|it|they|things` +
  `|gateway|provider|upstream|model|integration|webhook|index|queue|worker|search|sync`
const OUTAGE_STATE =
  `down|offline|unreachable|unavailable|not responding|won'?t respond|timing out|timed out|erroring|throwing (?:connection )?errors|stuck(?: in a recovery loop)?|in a recovery loop|flaky|went (?:down|unreachable|offline)|having (?:issues|problems|trouble)|acting up|recovering|coming back up|back up` +
  `|failing|broken|not working|refusing|rate[- ]?limited|throttled|degraded|out of service|overloaded|at capacity`
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

// ── Grounding: is this span the MODEL'S, or the user's own data coming back? ──
//
// THE MEASURED PROBLEM, not a theoretical one. Every detector below matches a
// SHAPE, and business identifiers share those shapes: an IMEI is Luhn-valid BY
// CONSTRUCTION so 100% of them read as payment cards, `XXX-XX-XXXX` part numbers
// read as SSNs, and because CARD_RE spans separators a space-separated list of
// short numeric ids CONCATENATES into one 16-digit "card". Around one in ten
// arbitrary 13-19 digit identifiers is Luhn-valid by chance. Tightening the
// regexes cannot move that floor: the strings really are the same shape.
//
// Two harms followed, and the second is the one this codebase cares most about.
// `redactSecrets` rewrote persisted and INDEXED content — a distillation the
// assistant later retrieves from, an OKF document, a chat title — turning a
// customer's order number into `[redacted card number]`. And every hit filed a
// `guard_findings` row against the MODEL, when `guard_findings.model` is one
// half of the model-fitness page. Two harnesses already narrow their rule sets
// specifically to avoid libelling the model they run on; that is a workaround
// for this, one harness at a time.
//
// THE TEST. If the span also appears in the turn's INPUT, the user handed it to
// the model. It is not something the model produced, so it is not evidence about
// the model, and rewriting it corrupts a summary of the user's own words. A span
// that appears NOWHERE in the input is digits nobody gave the model, which is
// the case both the finding and the redaction were built for.
//
// WHAT GROUNDING GOVERNS IS DECLARED PER RULE (`Rule.groundable`), because the
// two effects are different claims:
//   · a FINDING is a claim ABOUT THE MODEL, so a grounded span must never file
//     one.
//   · REDACTION is a claim about WHAT TALARIA WRITES DOWN, and where the span
//     came from does not settle that by itself — see `secret_leak` in RULES for
//     the one rule that splits them.
//
// NORMALIZATION IS LOAD-BEARING. A naive `input.includes(span)` misses the case
// it most needs to catch: the concatenated numeric list matched ACROSS the
// separators, so the span carries a run of digits the input never held in that
// arrangement. Both sides are stripped to alphanumerics and lowercased first,
// which is also what grounds `4111-1111-1111-1111` in an answer against
// `4111 1111 1111 1111` in the transcript it was summarizing.

/** Past this the tail of a very large input stops being grounding material.
 *  That is the conservative direction (unchanged behavior: flag and redact), and
 *  the normalized haystack is memoized because `guardAgentFields` asks the same
 *  question once per field and `redactSecrets` once per call. */
const GROUNDING_CAP = 1_000_000
/** A span shorter than this would ground against almost anything once the
 *  separators are gone. Every shape the detectors produce is longer (SSN 9
 *  digits, card 13+, IBAN 12+, credentials 20+), so the floor costs nothing and
 *  rules out a degenerate match. */
const GROUNDING_MIN = 8

const normalizeForGrounding = (s: string): string => s.toLowerCase().replace(/[^a-z0-9]+/g, '')

let haystackKey: string | null = null
let haystackValue = ''
function groundingHaystack(input: string): string {
  if (input === haystackKey) return haystackValue
  haystackKey = input
  haystackValue = normalizeForGrounding(input.length > GROUNDING_CAP ? input.slice(0, GROUNDING_CAP) : input)
  return haystackValue
}

/** Did this span come out of the turn's own input? Pure, and the only grounding
 *  comparison in the file — the detectors and the redactor all ask it here so
 *  that "the same span" has exactly one meaning. */
export function isGrounded(span: string, input: string | null | undefined): boolean {
  if (!input) return false
  const needle = normalizeForGrounding(span)
  if (needle.length < GROUNDING_MIN) return false
  return groundingHaystack(input).includes(needle)
}

/** The text a rule grounds against. `inputText` is everything the caller put IN
 *  FRONT of the model this turn — the rendered system prompt, the transcript,
 *  the tool results — and `userMessage` is the narrow fallback for the paths
 *  that only have the last user turn (a chat stream). Optional by design: a
 *  caller that supplies neither gets exactly the pre-grounding behavior, so no
 *  path can be broken by not having been updated yet. */
export const groundingInput = (ctx: GuardContext): string => ctx.inputText ?? ctx.userMessage

/** Every match of a pattern, whether or not it was written global. Grounding
 *  needs all of them: the FIRST card-shaped span in a reply is routinely the
 *  order number from the transcript, and returning it would mask a real one
 *  later in the same answer. */
function* allMatches(re: RegExp, text: string): Generator<string> {
  const scan = new RegExp(re.source, re.flags.includes('g') ? re.flags : `${re.flags}g`)
  for (let m = scan.exec(text); m; m = scan.exec(text)) yield m[0]
}

// ── Secret leak ──────────────────────────────────────────────────────────────
// High-value security check: an agent should never emit a live credential.
/** The first UNGROUNDED credential in the text, or — when every one of them came
 *  out of the input — the first grounded one, marked. Never just the first hit:
 *  a reply that quotes the key the user pasted AND emits one of its own must
 *  report the one the model invented. */
function detectSecret(text: string, input?: string): { label: string; snippet: string; grounded: boolean } | null {
  let grounded: { label: string; snippet: string; grounded: boolean } | null = null
  for (const { label, re } of SECRET_PATTERNS) {
    for (const raw of allMatches(re, text)) {
      const hit = { label, snippet: `${label}: ${raw.slice(0, 8)}…`, grounded: isGrounded(raw, input) }
      if (!hit.grounded) return hit
      grounded ??= hit
    }
  }
  return grounded
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

type PiiHit = { label: string; snippet: string; grounded: boolean }

/** Same rule as `detectSecret`: the first UNGROUNDED hit wins, in the SSN → card
 *  → IBAN priority the labels have always had, and a wholly grounded text
 *  reports its first hit marked rather than nothing — `runGuardrails` drops it,
 *  and keeping the shape uniform is what lets the drop live in one place. */
function detectPii(text: string, input?: string): PiiHit | null {
  let grounded: PiiHit | null = null
  const take = (raw: string, hit: Omit<PiiHit, 'grounded'>): PiiHit | null => {
    const full = { ...hit, grounded: isGrounded(raw, input) }
    if (!full.grounded) return full
    grounded ??= full
    return null
  }
  for (const ssn of allMatches(SSN_RE, text)) {
    const hit = take(ssn, { label: 'social security number', snippet: `SSN: ${ssn.slice(0, 6)}…` })
    if (hit) return hit
  }
  for (const card of allMatches(CARD_RE, text)) {
    if (!isCardNumber(card)) continue
    const hit = take(card, { label: 'payment card number', snippet: `card: ${card.replace(/[ -]/g, '').slice(0, 6)}…` })
    if (hit) return hit
  }
  for (const iban of allMatches(IBAN_RE, text)) {
    const hit = take(iban, { label: 'bank account (IBAN)', snippet: `IBAN: ${iban.slice(0, 6)}…` })
    if (hit) return hit
  }
  return grounded
}

/** PII redaction SKIPS a grounded span, per `pii_leak`'s `groundable` setting.
 *  This is the half of the decision that stops the guard rewriting the user's
 *  own words: the order number in the distillation is the one from the chat, and
 *  a summary in which it has become `[redacted card number]` is a worse artifact
 *  than the one it replaced.
 *
 *  `spread: 'broadcast'` REVERSES THAT, because the reasoning above is an
 *  audience argument — see `GuardContext.spread`. The order number is fine going
 *  back onto the chat it came from and is not fine going into a channel the
 *  chat's participants are not in. */
function redactPii(text: string, input: string | undefined, spread: 'contained' | 'broadcast' = 'contained'): string {
  if (spread === 'broadcast') input = undefined
  return text
    .replace(new RegExp(SSN_RE.source, 'g'), (m) => (isGrounded(m, input) ? m : '[redacted SSN]'))
    .replace(new RegExp(CARD_RE.source, 'g'), (m) => (isCardNumber(m) && !isGrounded(m, input) ? '[redacted card number]' : m))
    .replace(new RegExp(IBAN_RE.source, 'g'), (m) => (isGrounded(m, input) ? m : '[redacted IBAN]'))
}

/** Replace every detected credential AND ungrounded high-precision PII with a
 *  redaction marker. Two callers reach it on two different triggers, per the
 *  header: the relay sites scrub in STRICT mode, and `runHarness` scrubs
 *  whenever the harness declared `redact`, at any mode above off. Both apply it
 *  only to what Talaria persists or hasn't yet handed on — a live stream already
 *  showed the original, but the saved copy (and every transcript built from it)
 *  stays clean.
 *
 *  `input` is the turn's own input, and passing it is what stops this function
 *  rewriting the user's data back at them (see the grounding section). CREDENTIALS
 *  ARE REDACTED WHETHER OR NOT THEY ARE GROUNDED, which is the deliberate half of
 *  the split — `secret_leak` in RULES carries the argument. Omitting `input`
 *  grounds nothing, which is exactly the behavior every caller had before. */
export function redactSecrets(text: string, input?: string, spread: 'contained' | 'broadcast' = 'contained'): { text: string; redacted: boolean } {
  let out = text
  for (const { label, re, redactRe } of SECRET_PATTERNS) {
    const base = redactRe ?? re
    out = out.replace(new RegExp(base.source, base.flags.includes('g') ? base.flags : `${base.flags}g`), `[redacted ${label}]`)
  }
  out = redactPii(out, input, spread)
  return { text: out, redacted: out !== text }
}

/** Scrub the VERBATIM EXCERPT a finding carries. `snippet` is the one field on
 *  a finding that is model output rather than Talaria's own words, and the
 *  detectors that produce it are uneven about it: `secret_leak` and `pii_leak`
 *  truncate to a few characters on purpose, but `zero_tool_claim` (and
 *  `fabricated_outage`) return the offending SENTENCE verbatim, up to 240
 *  characters — and a sentence that both claims a completed action and carries a
 *  credential is an ordinary small-model reply.
 *
 *  So a strict-mode org could scrub `channel_messages.content` and leave the
 *  same key in `channel_messages.guard` on the same row. Anything that PINS
 *  findings to a row a human or an agent later reads goes through here first. */
export function redactFindings(findings: Finding[]): Finding[] {
  return findings.map((f) => (f.snippet ? { ...f, snippet: redactSecrets(f.snippet).text } : f))
}

/** Which findings warrant content redaction, on either trigger.
 *
 *  A GROUNDED FINDING STILL COUNTS, and that is the mechanism behind
 *  `secret_leak`'s split rather than an oversight: the finding is kept out of
 *  `guard_findings` and out of the caveat, and it survives in the returned array
 *  for exactly this predicate, so a credential the user pasted is still scrubbed
 *  from what Talaria writes down. `pii_leak` never reaches here grounded —
 *  `runGuardrails` drops those outright — so the two rules get the two different
 *  answers they declared. */
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
  /** The flagged span was already in the turn's input, so this finding is NOT
   *  evidence about the model. Only a rule with `groundable: 'finding'` can
   *  produce one, and it exists for exactly one reason: `needsRedaction` still
   *  has to say yes so the persisted copy gets scrubbed. It is never recorded
   *  (`recordFindings` drops it, so `guard_findings.model` stays a model fact)
   *  and never disclosed (`caveatFor` drops it). */
  grounded?: boolean
}

export interface GuardContext {
  answer: string
  toolRecord: ToolRecord
  userMessage: string
  policedHosts: string[]
  /** EVERYTHING THIS TURN PUT IN FRONT OF THE MODEL — the rendered system
   *  prompt, the transcript being summarized, the tool results — as one string.
   *  A rule grounds its hit against this (see the grounding section): a span
   *  that is already in here is the user's own data, not the model's invention.
   *
   *  OPTIONAL ON PURPOSE. `userMessage` is the narrow answer to the same
   *  question and every existing caller already supplies it, so a path that has
   *  not been taught to render its full input yet keeps working and simply
   *  grounds against less. The runner has the rendered messages and is the one
   *  caller that can supply the whole thing. */
  inputText?: string
  /** DOES THIS OUTPUT REACH A WIDER AUDIENCE THAN ITS SOURCE DID?
   *
   *  THE ARGUMENT `pii_leak`'s grounding rests on is an audience argument, and
   *  it is only true for a contained output: "that SSN is already sitting in the
   *  ticket this output summarizes, so scrubbing the summary removes nothing
   *  from Talaria". Perfectly sound — for a summary that goes back on the same
   *  ticket.
   *
   *  It stops being true the moment the output goes somewhere the source did
   *  not. A support transcript is visible to the people on that ticket; a
   *  message posted into #billing-triage is visible to the whole room and lands
   *  in the retrieval index behind it. The span is still "grounded" — it came
   *  from the input — and it is now in front of an audience that never had it.
   *
   *  The adversarial corpus found this exactly: asked to post a case into a
   *  channel, models copied the card number and the SSN into the message body,
   *  and production would have filed nothing and redacted nothing.
   *
   *  'contained' (the default) is the behaviour every existing caller had, so
   *  this only ever ADDS protection where a caller says it is warranted. */
  spread?: 'contained' | 'broadcast'
}

/** WHAT A GROUNDED HIT CHANGES, declared by the rule that produced it. Absent
 *  means the rule does not deal in spans at all (a zero-tool claim is a sentence
 *  the model wrote; finding it in the input would mean the model quoted the user
 *  back, which is not exoneration) and its hits are never grounded.
 *
 *    'finding'            drop the FINDING, keep the REDACTION. The span is not
 *                         evidence about the model, but Talaria still refuses to
 *                         write a second copy of it into a row it keeps.
 *    'finding+redaction'  drop both. The detector's shape is shared with
 *                         ordinary business data, so a grounded hit is most
 *                         likely not the thing the rule is named after — and
 *                         rewriting it damages the user's own content. */
export type Groundable = 'finding' | 'finding+redaction'

export interface Rule {
  id: string
  label: string
  severity: Severity
  defaultOn: boolean
  /** See `Groundable`. A rule that sets this must return `grounded` from `run`;
   *  one that does not, never will. */
  groundable?: Groundable
  /** Safe to run on plain text with no tool record (e.g. a ticket outcome at the
   *  judge gate). Rules that need the turn's tool record leave this false. */
  gateSafe?: boolean
  /** What the rule needs beyond the answer + which-tools-ran. A path that can't
   *  supply it (e.g. chat sees tool NAMES but not results/errors) skips the rule
   *  rather than false-positive. */
  needs?: Array<'results' | 'errorInfo'>
  /** Returns a hit (message + snippet + confidence 0..1) or null. Pure.
   *
   *  `grounded` says the flagged span was already in `groundingInput(ctx)`. The
   *  rule reports the fact; `runGuardrails` decides what it costs, from
   *  `groundable` above — one place, so a new rule cannot invent a third
   *  meaning for it. */
  run(ctx: GuardContext): { message: string; snippet: string; confidence: number; grounded?: boolean } | null
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
      // THE ORIGINAL GROUNDING RULE, and the reason the section above is a
      // generalization rather than a new idea: this check has always asked "was
      // this span in the material?". It now asks it of the whole input rather
      // than the last user turn, which can only remove false positives — a UUID
      // that came in through a rendered system prompt is no more invented than
      // one the user typed.
      const ungrounded = ungroundedRefs(ctx.answer, `${tr.resultsText}\n${groundingInput(ctx)}`, ctx.policedHosts)
      return ungrounded.length
        ? { message: 'Cites link(s)/id(s) that did not appear in any tool result this turn. They may be fabricated.', snippet: ungrounded.slice(0, 8).join(', '), confidence: 0.7 }
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
    // THE SPLIT, AND THE ARGUMENT FOR IT. A credential that appears in the input
    // is not automatically safe to echo — a user pasting a key does not make
    // repeating it into an indexed artifact harmless — but it is also not
    // evidence that the MODEL confabulated one. Those are two different
    // questions and they get two different answers:
    //
    //   THE FINDING IS DROPPED. `guard_findings.model` is half the model-fitness
    //   page and `guardCoachingFor` writes repeated flags into an agent's soul.
    //   Filing "you emitted credential-shaped strings" against a model for
    //   quoting back the key its operator pasted teaches the wrong lesson and
    //   corrupts a number people make staffing decisions with.
    //
    //   THE REDACTION STANDS. The input's copy lives under whatever retention
    //   the conversation has; the artifact's copy is a NEW copy, in a title, a
    //   distillation, a search index or a notification, with a different
    //   audience and a longer life. Nobody needs the key text in a summary, so
    //   removing it costs the artifact nothing — which is precisely the trade
    //   that fails for `pii_leak` below, where the span is usually an order
    //   number the summary is ABOUT.
    //
    //   AND THE PRECISION IS DIFFERENT. `ghp_` + 36 characters is a credential,
    //   not an order id. Grounding a secret is therefore almost never a false
    //   positive being corrected — it is a real key that really was pasted,
    //   which is the case where refusing to write another copy is most obviously
    //   right.
    groundable: 'finding',
    run: (ctx) => {
      const hit = detectSecret(ctx.answer, groundingInput(ctx))
      return hit
        ? { message: `Output appears to contain a live credential (${hit.label}).`, snippet: hit.snippet, confidence: 0.95, grounded: hit.grounded }
        : null
    },
  },
  {
    id: 'pii_leak',
    label: 'PII leak (SSN / card number / IBAN in output)',
    severity: 'high',
    defaultOn: true,
    gateSafe: true,
    // BOTH EFFECTS DROP, because here a grounded hit is usually not PII at all.
    // These are pure shape matches over business data: IMEIs are Luhn-valid by
    // construction, `XXX-XX-XXXX` part numbers look like SSNs, and ~10% of
    // 13-19 digit identifiers pass Luhn by chance. When the same span is in the
    // input, the overwhelmingly likely reading is that the model repeated an
    // identifier out of the user's own document — and redacting it rewrites the
    // artifact into something less true than what it summarized, while the
    // finding blames the model for reading its input.
    //
    // Real personal data pasted by a user is the honest cost of this, and it is
    // a small one: that SSN is already sitting in the ticket or transcript this
    // output summarizes, so scrubbing the summary removes nothing from Talaria
    // and only makes the summary disagree with its source.
    groundable: 'finding+redaction',
    run: (ctx) => {
      const hit = detectPii(ctx.answer, groundingInput(ctx))
      return hit
        ? { message: `Output appears to contain personal data (${hit.label}).`, snippet: hit.snippet, confidence: 0.9, grounded: hit.grounded }
        : null
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

/** One rule against one context: the confidence threshold, then the grounding
 *  decision. THE ONLY PLACE `Groundable` IS READ, so `runGuardrails` and
 *  `guardText` — which are the same loop over two different rule subsets — can
 *  never drift on what a grounded span costs. Returns null when there is nothing
 *  to report. */
function evaluate(rule: Rule, ctx: GuardContext, config: GuardConfig): Finding | null {
  const hit = rule.run(ctx)
  if (!hit || hit.confidence < config.minConfidence) return null
  const finding: Finding = { check: rule.id, severity: rule.severity, confidence: hit.confidence, message: hit.message, snippet: hit.snippet }
  if (!hit.grounded) return finding
  // A grounded hit from a rule that never declared itself groundable is an
  // author mistake, and the safe reading of a mistake is the old behavior: an
  // ordinary finding. Grounding may only ever REMOVE a claim about the model,
  // never add one, and it may only do so where a rule asked for it.
  if (!rule.groundable) return finding
  // A BROADCAST KEEPS THE REDACTION. `finding+redaction` drops both because the
  // span is probably ordinary business data the model read out of its input —
  // that reasoning survives, so the FINDING still goes (this is not evidence
  // about the model). What does not survive is the "it is already there anyway"
  // half: it is not already in the room this is being posted into.
  if (rule.groundable === 'finding+redaction') return ctx.spread === 'broadcast' ? { ...finding, grounded: true } : null
  return { ...finding, grounded: true }
}

/** Run the enabled, APPLICABLE rules, keep findings at/above the threshold. Pure.
 *  A rule whose `needs` can't be supplied on this path is skipped (no false
 *  positive) rather than run on missing data. */
export function runGuardrails(ctx: GuardContext, config: GuardConfig, available: Available = FULL): Finding[] {
  if (!ctx.answer) return []
  const out: Finding[] = []
  for (const rule of RULES) {
    if (!ruleEnabled(config, rule)) continue
    if (rule.needs?.some((n) => !available[n])) continue
    const finding = evaluate(rule, ctx, config)
    if (finding) out.push(finding)
  }
  return out
}

/** Layered tiering: run the gate-safe rules (no tool record needed) over plain
 *  text — e.g. a ticket's reported outcome at the judge gate, so a cheap
 *  structural signal feeds the expensive judge. Returns [] when the guard is off.
 *
 *  `input` is the material the text was written FROM, when the caller has it —
 *  the transcript, the ticket the agent was working. Both gate-safe rules are
 *  groundable, so supplying it is the difference between flagging a model for
 *  repeating the customer's order number and not. Callers that genuinely have no
 *  such material (the judge gate holds only the outcome) pass nothing and get
 *  the old behavior. */
export async function guardText(text: string, input?: string): Promise<Finding[]> {
  if (!text.trim()) return []
  const config = await getGuardConfig()
  if (config.mode === 'off') return []
  const ctx: GuardContext = {
    answer: text,
    toolRecord: { backingTools: [], resultsText: '', anyError: false, overflowed: false },
    userMessage: '',
    policedHosts: config.policedHosts,
    ...(input ? { inputText: input } : {}),
  }
  const out: Finding[] = []
  for (const rule of RULES) {
    if (!rule.gateSafe || !ruleEnabled(config, rule)) continue
    const finding = evaluate(rule, ctx, config)
    if (finding) out.push(finding)
  }
  return out
}

/** A human-facing caveat for annotate mode (appended out-of-band, never re-fed
 *  into the model's context).
 *
 *  GROUNDED FINDINGS ARE NOT DISCLOSED. The caveat's wording is "the confab
 *  guard flagged this response", which is a claim about the model, and a
 *  grounded span is by definition not one — the only reason it survived
 *  `runGuardrails` at all is so the stored copy still gets scrubbed. */
export function caveatFor(findings: Finding[]): string {
  const shown = findings.filter((f) => !f.grounded)
  if (!shown.length) return ''
  const lines = shown.map((f) => `- **${f.check.replace(/_/g, ' ')}:** ${f.message}${f.snippet ? ` (${f.snippet})` : ''}`)
  return `\n\n---\n⚠️ **Unverified: confab guard flagged this response:**\n${lines.join('\n')}\nVerify before relying on it.`
}

// ── Findings store + observability ───────────────────────────────────────────

/** THE ONE DOOR TO `guard_findings`, and therefore the one place that decides
 *  what counts as a fact about a model.
 *
 *  Grounded findings do not go in. That column is read as a per-model
 *  confabulation rate by the fitness page and as behavioral feedback by
 *  `guardCoachingFor`, and a row filed because the model repeated an identifier
 *  out of its own input is not a fact about the model — it is a fact about the
 *  input. Dropping them here rather than at each of the four callers is what
 *  keeps that true for callers written later. */
export async function recordFindings(
  findings: Finding[],
  meta: { caller: string; model: string; endpoint: string | null; mode: GuardMode },
): Promise<void> {
  const filed = findings.filter((f) => !f.grounded)
  if (!filed.length) return
  const sql = await db()
  for (const f of filed) {
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
  const findings = runGuardrails(
    { answer: input.answer, toolRecord, userMessage, inputText: groundingTextOf(messages), policedHosts: config.policedHosts },
    config,
  )
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
  /** See `GuardContext.spread`. A CHANNEL IS A BROADCAST: the reply lands in
   *  front of everyone in the room and in the retrieval index behind it, which
   *  is not the audience its source material had. A DM back to the person who
   *  pasted the data is contained, and stays the default. */
  spread?: 'contained' | 'broadcast'
}): Promise<{ findings: Finding[]; mode: GuardMode }> {
  const config = await getGuardConfig()
  if (config.mode === 'off' || !input.answer) return { findings: [], mode: config.mode }
  const backingTools = input.toolNames.filter((n) => n && !NONBACKING.has(n))
  const toolRecord: ToolRecord = { backingTools, resultsText: '', anyError: false, overflowed: true }
  const findings = runGuardrails(
    { answer: input.answer, toolRecord, userMessage: input.userMessage, policedHosts: config.policedHosts, spread: input.spread ?? 'contained' },
    config,
    { results: false, errorInfo: false },
  )
  await recordFindings(findings, { caller: input.caller, model: input.model, endpoint: 'fleet', mode: config.mode }).catch(() => {})
  return { findings, mode: config.mode }
}
