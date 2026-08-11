// REDACTION IS GROUNDED AGAINST THE INPUT — the measured cases, asserted.
//
// The validation round measured `redactSecrets` rewriting ordinary content:
// Luhn-valid ORDER IDs and IMEIs became `[redacted card number]` (an IMEI is
// Luhn-valid by construction, so 100% of them), `XXX-XX-XXXX` part numbers
// became `[redacted SSN]`, and a space-separated numeric list concatenated into
// a card match. Roughly one in ten arbitrary 13-19 digit identifiers.
//
// Both harms are asserted here, because they are different harms with different
// victims: the artifact Talaria writes (a distillation the assistant later
// retrieves from) and the `guard_findings` row filed against the MODEL, which is
// half the model-fitness page. The test that separates a false positive from a
// real one is the same in both cases — was this span already in the input?
//
// Everything below is pure except the two `recordFindings` cases, which are the
// only way to assert "no row is filed" rather than "no finding is returned".
import { describe, expect, it, vi } from 'vitest'
import {
  caveatFor,
  groundingTextOf,
  needsRedaction,
  recordFindings,
  redactSecrets,
  RULES,
  runGuardrails,
  type Finding,
  type GuardConfig,
  type GuardContext,
} from '@/server/guardrails'
import { guardAgentWrite, type AgentWriteDeps } from '@/server/agent-writes'

const inserted = vi.hoisted(() => [] as unknown[][])
vi.mock('@/server/db/pg', () => ({
  db: async () => (_sql: TemplateStringsArray, ...values: unknown[]) => {
    inserted.push(values)
    return []
  },
}))

// ── Fixtures ────────────────────────────────────────────────────────────────

/** A real IMEI. Luhn-valid BY CONSTRUCTION, which is why every single one of
 *  them read as a payment card before grounding existed. */
const IMEI = '490154203237518'
/** A Luhn-valid order number — the shape ~10% of long identifiers happen to
 *  have, and the exact case the validation report opened with. */
const ORDER = '4012888888881881'
/** A card the model produced out of nowhere. */
const INVENTED_CARD = '4242424242424242'
const PART_NO = '482-19-7734'
const TOKEN = `ghp_${'a'.repeat(36)}`

const config = (over: Partial<GuardConfig> = {}): GuardConfig => ({
  mode: 'observe',
  checks: {},
  minConfidence: 0.5,
  policedHosts: [],
  coach: false,
  ...over,
})

const ctx = (answer: string, inputText: string, over: Partial<GuardContext> = {}): GuardContext => ({
  answer,
  toolRecord: { backingTools: [], resultsText: '', anyError: false, overflowed: false },
  userMessage: '',
  policedHosts: [],
  inputText,
  ...over,
})

/** One rule at a time, so a case tests exactly what its name says. */
const only = (id: string, c: GuardContext) =>
  runGuardrails(c, config({ checks: Object.fromEntries(RULES.map((r) => [r.id, r.id === id])) }))

const TRANSCRIPT = [
  'Priya: my order 4012888888881881 still has not arrived.',
  'Priya: the handset it was for has IMEI 490154203237518.',
  'Priya: the replacement part is 482-19-7734.',
].join('\n')

// ── The finding half ────────────────────────────────────────────────────────

describe('a span that came out of the input', () => {
  it('files no finding for an order number the transcript contained', () => {
    const answer = `Priya is chasing order ${ORDER} and wants a delivery date.`
    expect(only('pii_leak', ctx(answer, TRANSCRIPT))).toEqual([])
  })

  it('files no finding for an IMEI from the transcript — the 100% false-positive shape', () => {
    expect(only('pii_leak', ctx(`The handset (IMEI ${IMEI}) shipped on the 3rd.`, TRANSCRIPT))).toEqual([])
  })

  it('files no finding for an SSN-shaped part number from the transcript', () => {
    expect(only('pii_leak', ctx(`Part ${PART_NO} is on back order.`, TRANSCRIPT))).toEqual([])
  })

  it('flags the SAME order number when nothing in the input contains it', () => {
    // The other half of the pair, and the reason grounding is a test rather than
    // a weakening: with no input to ground against, this is a model emitting
    // digits nobody gave it, which is exactly what the rule is for.
    const answer = `Priya is chasing order ${ORDER} and wants a delivery date.`
    const findings = only('pii_leak', ctx(answer, 'Priya: my parcel still has not arrived.'))
    expect(findings.map((f) => f.check)).toEqual(['pii_leak'])
    expect(findings[0]?.grounded).toBeUndefined()
  })

  it('flags a card the model invented, even in a turn full of grounded identifiers', () => {
    // The reason the detectors return the first UNGROUNDED hit rather than the
    // first hit: the order number is scanned first and would otherwise mask the
    // real leak sitting two clauses later.
    const answer = `Order ${ORDER} is on its way. Charge the balance to ${INVENTED_CARD}.`
    const findings = only('pii_leak', ctx(answer, TRANSCRIPT))
    expect(findings.map((f) => f.check)).toEqual(['pii_leak'])
    expect(findings[0]?.snippet).toBe('card: 424242…')
  })

  it('needs the WHOLE span in the input, not a prefix of it', () => {
    expect(only('pii_leak', ctx(`Order ${ORDER} shipped.`, 'Priya: my order 40128888 still has not arrived.'))).toHaveLength(1)
  })
})

// ── Normalization ───────────────────────────────────────────────────────────

describe('normalization', () => {
  // The report's third case: CARD_RE spans separators, so a list of short ids
  // CONCATENATES into one 16-digit "card" that appears nowhere in the input in
  // that arrangement. A naive `input.includes(span)` misses the grounded case it
  // most needs to catch.
  const input = 'Pull the totals for lots 4012, 8888, 8888, 1881 please.'
  const answer = 'Lots 4012 8888 8888 1881 are all cleared.'

  it('grounds a span the input only holds across different separators', () => {
    expect(input.includes('4012 8888 8888 1881')).toBe(false) // the naive test fails
    expect(only('pii_leak', ctx(answer, input))).toEqual([])
    expect(redactSecrets(answer, input).text).toBe(answer)
  })

  it('still redacts the concatenated list when the input never held those digits', () => {
    expect(redactSecrets(answer, 'Pull the totals for the north lots please.').text).toContain('[redacted card number]')
  })

  it('grounds across a different digit grouping of the same number', () => {
    const spaced = `Refund the card 4012 8888 8888 1881 today.`
    expect(only('pii_leak', ctx(spaced, TRANSCRIPT))).toEqual([])
    expect(redactSecrets(spaced, TRANSCRIPT).text).toBe(spaced)
  })
})

// ── The redaction half ──────────────────────────────────────────────────────

describe('redactSecrets', () => {
  it('leaves a grounded identifier in the text it is about to persist', () => {
    const answer = `Priya is chasing order ${ORDER}; the handset is IMEI ${IMEI}, part ${PART_NO}.`
    expect(redactSecrets(answer, TRANSCRIPT)).toEqual({ text: answer, redacted: false })
  })

  it('redacts the same text when there is no input to ground it against', () => {
    const answer = `Priya is chasing order ${ORDER}; the handset is IMEI ${IMEI}, part ${PART_NO}.`
    const { text, redacted } = redactSecrets(answer)
    expect(redacted).toBe(true)
    expect(text).toContain('[redacted card number]')
    expect(text).toContain('[redacted SSN]')
  })

  it('scrubs an invented card while leaving the grounded order number intact', () => {
    const { text } = redactSecrets(`Order ${ORDER} is on its way. Charge the balance to ${INVENTED_CARD}.`, TRANSCRIPT)
    expect(text).toBe(`Order ${ORDER} is on its way. Charge the balance to [redacted card number].`)
  })
})

// ── The split, for secret_leak ──────────────────────────────────────────────

describe('a credential that was in the input', () => {
  // THE DELIBERATE ASYMMETRY, both halves. A key the user pasted is not evidence
  // the MODEL confabulated one — so no row is filed against it and no reader is
  // told the model leaked something. But it is still a live credential, and the
  // copy Talaria is about to write into a title, an index or a notification is a
  // NEW copy with a different audience and a longer life, so it is removed.
  const pasted = `Here is my token ${TOKEN}, use it to check the PR.`
  const answer = `I used ${TOKEN} and the PR is green.`

  it('files no finding — the claim would be about the model, and it is not the model', () => {
    const findings = only('secret_leak', ctx(answer, pasted))
    expect(findings.map((f) => f.check)).toEqual(['secret_leak'])
    expect(findings[0]?.grounded).toBe(true)
    expect(caveatFor(findings)).toBe('')
    inserted.length = 0
    return recordFindings(findings, { caller: 'platform:titler', model: 'qwen3-14b', endpoint: 'pl-main', mode: 'observe' }).then(() => {
      expect(inserted).toEqual([])
    })
  })

  it('STILL redacts the persisted copy — the finding survives only to say so', () => {
    const findings = only('secret_leak', ctx(answer, pasted))
    // The gate every redacting caller uses. It has to keep saying yes, or the
    // split silently becomes "grounded credentials are written down forever".
    expect(needsRedaction(findings)).toBe(true)
    expect(redactSecrets(answer, pasted).text).toBe('I used [redacted GitHub token] and the PR is green.')
  })

  it('files the finding and discloses it when the credential is the model’s own', async () => {
    const findings = only('secret_leak', ctx(answer, 'Please check whether the PR is green.'))
    expect(findings[0]?.grounded).toBeUndefined()
    expect(caveatFor(findings)).toContain('secret leak')
    inserted.length = 0
    await recordFindings(findings, { caller: 'platform:titler', model: 'qwen3-14b', endpoint: 'pl-main', mode: 'observe' })
    expect(inserted).toHaveLength(1)
  })

  it('reports the credential the model invented over the one it was handed', () => {
    const mine = `ghp_${'b'.repeat(36)}`
    const findings = only('secret_leak', ctx(`I used ${TOKEN}, then minted ${mine}.`, pasted))
    expect(findings[0]?.grounded).toBeUndefined()
    expect(findings[0]?.snippet).toBe('GitHub token: ghp_bbbb…')
  })
})

// ── The channel the runner supplies ─────────────────────────────────────────

describe('GuardContext.inputText', () => {
  it('falls back to userMessage, so a path that only has the user turn still grounds', () => {
    const answer = `Order ${ORDER} shipped.`
    const c: GuardContext = {
      answer,
      toolRecord: { backingTools: [], resultsText: '', anyError: false, overflowed: false },
      userMessage: `where is order ${ORDER}?`,
      policedHosts: [],
    }
    expect(only('pii_leak', c)).toEqual([])
  })

  it('is built from everything a model did NOT write', () => {
    // The laundering hazard, closed at the one place every caller goes through:
    // `runHarness` repairs by appending the model's own rejected reply and
    // re-asking, so grounding the second attempt against the sent history would
    // ground it against the first attempt's confabulation.
    const text = groundingTextOf([
      { role: 'system', content: 'You are triaging tickets.' },
      { role: 'user', content: 'any news?' },
      { role: 'assistant', content: `card ${INVENTED_CARD}` },
      { role: 'tool', content: `{"order":"${ORDER}"}` },
    ])
    expect(text).toContain(ORDER)
    expect(text).toContain('triaging tickets')
    expect(text).not.toContain(INVENTED_CARD)
  })

  it('grounds against tool results and system prompts, not just the user’s own sentence', () => {
    // An order number reaches a model through a rendered ticket or a CRM lookup
    // at least as often as through the user's typing, and grounding asks "did
    // the model make this up", not "did the human type it".
    const c = ctx(`Order ${ORDER} shipped.`, `[crm lookup] { "order": "${ORDER}", "status": "in transit" }`, { userMessage: 'any news?' })
    expect(only('pii_leak', c)).toEqual([])
  })
})

// ── The agent-write door ────────────────────────────────────────────────────

describe('guardAgentWrite', () => {
  /** The real gate-safe rules, with the door's grounding material threaded in —
   *  `guardText`'s implementation minus its settings read. */
  const deps = (mode: 'observe' | 'strict'): Partial<AgentWriteDeps> => ({
    isAgent: async (name) => name === 'nomad',
    guardText: async (text, input) =>
      runGuardrails(ctx(text, input ?? ''), config({ mode, checks: Object.fromEntries(RULES.map((r) => [r.id, r.gateSafe === true])) })),
    guardConfig: async () => config({ mode }),
    recordFindings: async () => {},
  })

  it('does not flag or rewrite a triage outcome that quotes the reporter’s order number', async () => {
    // The measured case at the door that both persists AND indexes the text.
    const outcome = `Refunded order ${ORDER} for the handset with IMEI ${IMEI}.`
    const out = await guardAgentWrite('ticket-write', { agent: 'nomad' }, outcome, deps('strict'), { input: TRANSCRIPT })
    expect(out.findings).toEqual([])
    expect(out.text).toBe(outcome)
    expect(out.redacted).toBe(false)
  })

  it('flags and rewrites the same sentence when the ticket never mentioned those numbers', async () => {
    const outcome = `Refunded order ${ORDER} for the handset with IMEI ${IMEI}.`
    const out = await guardAgentWrite('ticket-write', { agent: 'nomad' }, outcome, deps('strict'), { input: 'Priya: the parcel never arrived.' })
    expect(out.findings.map((f) => f.check)).toEqual(['pii_leak'])
    expect(out.text).toContain('[redacted card number]')
  })

  it('still strips a credential the agent was handed, without filing it against the agent', async () => {
    const recorded: Finding[][] = []
    const out = await guardAgentWrite(
      'direct-message',
      { agent: 'nomad' },
      `the token is ${TOKEN}`,
      { ...deps('strict'), recordFindings: async (findings) => void recorded.push(findings) },
      { input: `use ${TOKEN} for the deploy` },
    )
    expect(out.text).toBe('the token is [redacted GitHub token]')
    expect(out.redacted).toBe(true)
    // The door hands the finding to `recordFindings`, which is the one place that
    // decides what reaches guard_findings — and it drops grounded ones there so
    // every caller written later inherits the rule.
    expect(recorded[0]?.every((f) => f.grounded === true)).toBe(true)
  })
})

// ── Audience: a grounded span is not grounded everywhere ─────────────────────
//
// `pii_leak` drops both the finding AND the redaction for a grounded span, and
// the argument is an AUDIENCE argument: "that SSN is already sitting in the
// ticket this output summarizes, so scrubbing the summary removes nothing from
// Talaria". Sound — for a summary going back onto the same ticket.
//
// It stops being sound when the output goes somewhere the source did not. The
// adversarial corpus caught it: asked to post a support case into a team
// channel, models copied the card number and the SSN into the message body, and
// production filed nothing and redacted nothing.

describe('spread: broadcast', () => {
  const TRANSCRIPT_WITH_PII = 'CUSTOMER: the charge on card 4242 4242 4242 4242 came through twice. My social is 123-45-6789.'
  const REPLY = 'Duplicate charge reported. Card 4242 4242 4242 4242, SSN 123-45-6789 — please pick up.'

  it('still drops the FINDING, because that reasoning survives', () => {
    // The span probably IS the user's own data read back — that half of the
    // argument is untouched, and a finding here would blame the model for
    // reading its input.
    const contained = only('pii_leak', ctx(REPLY, TRANSCRIPT_WITH_PII))
    expect(contained).toEqual([])
    const broadcast = only('pii_leak', ctx(REPLY, TRANSCRIPT_WITH_PII, { spread: 'broadcast' }))
    expect(broadcast.length).toBe(1)
    expect(broadcast.every((f) => f.grounded)).toBe(true)
  })

  it('KEEPS the redaction, because "it is already there" is false in the new room', () => {
    expect(needsRedaction(only('pii_leak', ctx(REPLY, TRANSCRIPT_WITH_PII)))).toBe(false)
    expect(needsRedaction(only('pii_leak', ctx(REPLY, TRANSCRIPT_WITH_PII, { spread: 'broadcast' })))).toBe(true)
  })

  it('scrubs the span on a broadcast and leaves it on a contained reply', () => {
    // The contained case is the one the exemption exists for: a summary in which
    // the order number has become `[redacted]` is a worse artifact than the one
    // it replaced.
    expect(redactSecrets(REPLY, TRANSCRIPT_WITH_PII).text).toBe(REPLY)
    const wide = redactSecrets(REPLY, TRANSCRIPT_WITH_PII, 'broadcast').text
    expect(wide).toContain('[redacted card number]')
    expect(wide).toContain('[redacted SSN]')
  })

  it('does not change what a CONTAINED path does — the default is unchanged', () => {
    // Every existing caller keeps its behaviour; this only ever adds protection
    // where a caller says the audience widened.
    expect(redactSecrets(REPLY, TRANSCRIPT_WITH_PII)).toEqual({ text: REPLY, redacted: false })
  })
})
