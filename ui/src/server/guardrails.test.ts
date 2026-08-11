import { describe, expect, it } from 'vitest'
import {
  caveatFor,
  extractToolRecord,
  guardRuleMeta,
  needsRedaction,
  redactFindings,
  redactSecrets,
  runGuardrails,
  RULES,
  type Finding,
  type GuardConfig,
  type GuardContext,
  type ToolRecord,
} from '@/server/guardrails'

// Everything exercised here is pure: `runGuardrails` takes its config as an
// argument, so no app_settings read and no DB is involved. The async entry
// points (guardText / guardCompletion / guardChatReply / recordFindings) are
// deliberately NOT covered — they are getSetting + an insert around this same
// pure core, and testing them would mean mocking away the only thing they add.

const EMPTY_RECORD: ToolRecord = { backingTools: [], resultsText: '', anyError: false, overflowed: false }

const config = (over: Partial<GuardConfig> = {}): GuardConfig => ({
  mode: 'observe',
  checks: {},
  minConfidence: 0.5,
  policedHosts: [],
  coach: false,
  ...over,
})

const ctx = (answer: string, over: Partial<GuardContext> = {}): GuardContext => ({
  answer,
  toolRecord: EMPTY_RECORD,
  userMessage: '',
  policedHosts: [],
  ...over,
})

/** Run with only one rule enabled, so a case tests exactly what it says it does. */
const only = (id: string, c: GuardContext, over: Partial<GuardConfig> = {}) => {
  const checks = Object.fromEntries(RULES.map((r) => [r.id, r.id === id]))
  return runGuardrails(c, config({ checks, ...over }))
}
const checks = (findings: Finding[]) => findings.map((f) => f.check)

// ── extractToolRecord ───────────────────────────────────────────────────────

describe('extractToolRecord', () => {
  const assistantCall = (name: string) => ({ role: 'assistant', tool_calls: [{ function: { name } }] })

  it('only looks at messages after the LAST user message', () => {
    const rec = extractToolRecord([
      { role: 'user', content: 'first ask' },
      assistantCall('web_search'),
      { role: 'tool', content: 'old result' },
      { role: 'user', content: 'second ask' },
      assistantCall('send_email'),
      { role: 'tool', content: 'new result' },
    ])
    expect(rec.backingTools).toEqual(['send_email'])
    expect(rec.resultsText).toBe('new result')
  })

  it('treats the whole history as the turn when there is no user message', () => {
    expect(extractToolRecord([assistantCall('search'), { role: 'tool', content: 'r' }]).backingTools).toEqual(['search'])
  })

  it('excludes bookkeeping tools that are not a real external action', () => {
    const rec = extractToolRecord([
      { role: 'user', content: 'go' },
      assistantCall('think'),
      assistantCall('memory'),
      assistantCall('todo'),
      assistantCall('tool_search'),
      assistantCall('search_knowledge'),
    ])
    expect(rec.backingTools).toEqual([])
  })

  it('counts every backing call, including repeats', () => {
    const rec = extractToolRecord([
      { role: 'user', content: 'go' },
      { role: 'assistant', tool_calls: [{ function: { name: 'fetch' } }, { function: { name: 'fetch' } }] },
      assistantCall('think'),
      assistantCall('fetch'),
    ])
    expect(rec.backingTools).toEqual(['fetch', 'fetch', 'fetch'])
  })

  it('collects results from both tool and function roles, and stringifies structured content', () => {
    const rec = extractToolRecord([
      { role: 'user', content: 'go' },
      { role: 'tool', content: 'plain' },
      { role: 'function', content: { id: 'abc' } },
      { role: 'tool', content: null },
    ])
    expect(rec.resultsText).toBe('plain\n{"id":"abc"}\n')
  })

  it('flags a transport error but not a plain application error', () => {
    expect(extractToolRecord([{ role: 'tool', content: 'ECONNREFUSED 10.0.0.5:443' }]).anyError).toBe(true)
    expect(extractToolRecord([{ role: 'tool', content: 'Error: document not found' }]).anyError).toBe(false)
    expect(extractToolRecord([{ role: 'tool', content: 'upstream returned 502 bad gateway' }]).anyError).toBe(true)
  })

  it('drops oversized results and marks the record overflowed (grounding fails open)', () => {
    const rec = extractToolRecord([{ role: 'tool', content: 'x'.repeat(200_001) }])
    expect(rec.overflowed).toBe(true)
    expect(rec.resultsText).toBe('')
  })
})

// ── zero_tool_claim ─────────────────────────────────────────────────────────

describe('zero_tool_claim', () => {
  it('fires on a completed-action claim with no backing tool', () => {
    expect(checks(only('zero_tool_claim', ctx("I've drafted the email and saved it.")))).toEqual(['zero_tool_claim'])
    expect(checks(only('zero_tool_claim', ctx('The meeting is on your calendar.')))).toEqual(['zero_tool_claim'])
    expect(checks(only('zero_tool_claim', ctx("It's sitting in your drafts.")))).toEqual(['zero_tool_claim'])
  })

  it('stays silent once a backing tool has run', () => {
    const c = ctx("I've drafted the email and saved it.", { toolRecord: { ...EMPTY_RECORD, backingTools: ['gmail_draft'] } })
    expect(only('zero_tool_claim', c)).toEqual([])
  })

  it('stays silent for future/offered actions', () => {
    for (const answer of [
      "I'll draft the email and save it.",
      'Want me to create the ticket?',
      'I can put together a summary document.',
      'Let me know if you want me to schedule the meeting.',
    ]) {
      expect(only('zero_tool_claim', ctx(answer))).toEqual([])
    }
  })

  it('flags a past-tense claim even when a neighbouring sentence is an offer', () => {
    const c = ctx("I'll follow up later. I created the ticket for you.")
    expect(checks(only('zero_tool_claim', c))).toEqual(['zero_tool_claim'])
  })

  it('reports the offending sentence as the snippet, capped', () => {
    const [f] = only('zero_tool_claim', ctx(`I created the ticket. ${'padding '.repeat(60)}`))
    expect(f?.snippet).toBe('I created the ticket.')
    expect(f?.confidence).toBe(0.8)
    expect(f?.severity).toBe('high')
  })

  it('does not fire on ordinary prose', () => {
    for (const answer of ['Here is a summary of the three options.', 'The API returns a list of records.', 'Sure.']) {
      expect(only('zero_tool_claim', ctx(answer))).toEqual([])
    }
  })

  // ── The completion vocabulary ──────────────────────────────────────────────
  //
  // THE HOLE THE FITNESS CORPUS FOUND. A model asked to write a standup answered
  // "finished PLAT-118, closed t-77, merged the migration PR" with no tool
  // having run, and the guard said nothing — not because the claim was subtle,
  // but because `closed`, `finished` and `merged` were not words it knew. These
  // are the highest-consequence claims an agent makes; a rule that cannot see
  // them is a rule that misses the reason it exists.
  it('flags a completed-work claim, which is the claim that matters most', () => {
    for (const answer of [
      'I closed the ticket once the tests went green.',
      'I merged the PR and deployed the release.',
      'The migration is done — I pushed the branch and marked the ticket resolved.',
      'I processed the refund on that invoice.',
      'I approved the review and moved the board column.',
      'I shared the document with Priya and added her as a watcher.',
    ]) {
      expect(checks(only('zero_tool_claim', ctx(answer))), answer).toEqual(['zero_tool_claim'])
    }
  })

  it('separates an OFFER from an INABILITY, which read alike and mean opposites', () => {
    // `I could` used to match the offer pattern unconditionally, so "…so I could
    // not finish" skipped the whole sentence — and any claim a model appended an
    // explanation to went unscored. That is the shape a model actually writes.
    expect(only('zero_tool_claim', ctx('I could close the ticket if you confirm.'))).toEqual([])
    expect(checks(only('zero_tool_claim', ctx('I closed the ticket, though I could not verify the tests.')))).toEqual(['zero_tool_claim'])
  })

  it('still says nothing about work a model is OFFERING to do', () => {
    // The widened vocabulary must not swallow the future tense — an offer is the
    // correct answer for an agent with no tools, and flagging it would punish
    // exactly the behaviour the rule wants.
    for (const answer of [
      'I can close the ticket once you confirm.',
      "I'll merge the PR after review.",
      'Want me to process the refund?',
      'Should I deploy the release now?',
    ]) {
      expect(only('zero_tool_claim', ctx(answer)), answer).toEqual([])
    }
  })

  it('does not flag a model for WRITING something, which is not a tool action', () => {
    // The line the artifact list is drawn on: a thing that cannot exist without a
    // system action. A summary exists because the model wrote it — flagging that
    // would fire on every honest answer, which is a worse failure than missing a
    // rare dishonest one.
    for (const answer of [
      'I put together a summary of the three options below.',
      'I have written up my reasoning; the tradeoffs are listed above.',
      'I ran into a problem while drafting the email — I need the address first.',
    ]) {
      expect(only('zero_tool_claim', ctx(answer)), answer).toEqual([])
    }
  })
})

// ── ungrounded_ref ──────────────────────────────────────────────────────────

describe('ungrounded_ref', () => {
  const withResults = (results: string, answer: string, policedHosts: string[] = []) =>
    ctx(answer, { toolRecord: { ...EMPTY_RECORD, backingTools: ['search'], resultsText: results }, policedHosts })

  it('flags a UUID that appeared in no tool result', () => {
    const c = withResults('nothing useful', 'See ticket 3f2504e0-4f89-11d3-9a0c-0305e82c3301.')
    expect(checks(only('ungrounded_ref', c))).toEqual(['ungrounded_ref'])
  })

  it('accepts a UUID present in the results, case-insensitively', () => {
    const c = withResults('id=3F2504E0-4F89-11D3-9A0C-0305E82C3301', 'See ticket 3f2504e0-4f89-11d3-9a0c-0305e82c3301.')
    expect(only('ungrounded_ref', c)).toEqual([])
  })

  it('accepts a UUID the USER supplied', () => {
    const c = ctx('Ticket 3f2504e0-4f89-11d3-9a0c-0305e82c3301 is done.', {
      toolRecord: { ...EMPTY_RECORD, backingTools: ['search'] },
      userMessage: 'look at 3f2504e0-4f89-11d3-9a0c-0305e82c3301',
    })
    expect(only('ungrounded_ref', c)).toEqual([])
  })

  it('polices only the configured hosts, and only URLs with a path', () => {
    const answer = 'Try https://wiki.corp.example/pages/invented and https://example.com/anything.'
    expect(checks(only('ungrounded_ref', withResults('', answer, ['corp.example'])))).toEqual(['ungrounded_ref'])
    // Same answer, no policed hosts configured → the external link is not our business.
    expect(only('ungrounded_ref', withResults('', answer))).toEqual([])
    // A bare policed host with no path is not a fabricable reference.
    expect(only('ungrounded_ref', withResults('', 'See https://wiki.corp.example', ['corp.example']))).toEqual([])
  })

  it('matches a policed host by suffix, and ignores trailing punctuation/query/fragment', () => {
    const answer = 'It is at https://wiki.corp.example/pages/real?tab=1#top.'
    expect(only('ungrounded_ref', withResults('https://wiki.corp.example/pages/real', answer, ['corp.example']))).toEqual([])
  })

  it('skips itself when no backing tool ran, or when results overflowed', () => {
    const answer = 'See 3f2504e0-4f89-11d3-9a0c-0305e82c3301.'
    expect(only('ungrounded_ref', ctx(answer))).toEqual([])
    expect(
      only('ungrounded_ref', ctx(answer, { toolRecord: { ...EMPTY_RECORD, backingTools: ['s'], overflowed: true } })),
    ).toEqual([])
  })

  it('is skipped entirely on a path that cannot supply tool results', () => {
    const c = withResults('', 'See 3f2504e0-4f89-11d3-9a0c-0305e82c3301.')
    expect(runGuardrails(c, config(), { results: false, errorInfo: true })).toEqual([])
  })
})

// ── fabricated_outage ───────────────────────────────────────────────────────

describe('fabricated_outage', () => {
  it('fires on an outage claim when nothing errored', () => {
    for (const answer of [
      'The server is down right now.',
      'The MCP endpoint is timing out.',
      "I couldn't connect to the backend.",
      'The upstream returned a 502 bad gateway.',
      'Try again in about 30 seconds.',
    ]) {
      expect(checks(only('fabricated_outage', ctx(answer)))).toEqual(['fabricated_outage'])
    }
  })

  it('stays silent when a tool really did error', () => {
    const c = ctx('The server is down right now.', { toolRecord: { ...EMPTY_RECORD, anyError: true } })
    expect(only('fabricated_outage', c)).toEqual([])
  })

  it('is skipped on a path with no error information', () => {
    expect(runGuardrails(ctx('The server is down.'), config(), { results: true, errorInfo: false })).toEqual([])
  })

  it('flags the things an agent in THIS product actually blames', () => {
    // The subject list knew `server` and `service` and not `gateway`,
    // `provider`, `index` or `queue` — which are the words an agent reaches for
    // here, so the commonest fabricated outage in this product was invisible.
    for (const answer of [
      'The gateway is rate limited right now, so I could not get an answer.',
      'The provider appears to be degraded — nothing came back.',
      'The search index is not working at the moment.',
      'The queue is overloaded, which is why your job has not run.',
    ]) {
      expect(checks(only('fabricated_outage', ctx(answer))), answer).toEqual(['fabricated_outage'])
    }
  })

  it('does not fire on ordinary prose about servers', () => {
    expect(only('fabricated_outage', ctx('The server runs Postgres 16 and handles the write path.'))).toEqual([])
  })
})

// ── secret_leak ─────────────────────────────────────────────────────────────

describe('secret_leak — shapes already detected', () => {
  const cases: Array<[string, string]> = [
    ['OpenAI key', 'sk-proj-abcdefghijklmnopqrstuvwxyz0123456789'],
    ['OpenAI key', 'sk-abcdefghijklmnopqrstuvwxyz0123'],
    ['Anthropic key', 'sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789'],
    ['AWS access key', 'AKIAIOSFODNN7EXAMPLE'],
    ['Google API key', `AIzaSy${'a1B2c3D4e5'.repeat(3)}abc`], // fixed 39 chars
    ['Slack token', 'xoxb-123456789012-1234567890123-abcdefghijklmnopqrstuvwx'],
    ['GitHub token', `ghp_${'a'.repeat(36)}`],
    ['GitHub token', `gho_${'B'.repeat(36)}`],
    ['Talaria gateway key', `tlk_${'a1b2c3d4'.repeat(5)}`],
    ['Private key block', '-----BEGIN RSA PRIVATE KEY-----\nMIIEow==\n-----END RSA PRIVATE KEY-----'],
  ]

  it.each(cases)('detects a %s', (label, secret) => {
    const [f] = only('secret_leak', ctx(`Here you go: ${secret}`))
    expect(f?.check).toBe('secret_leak')
    expect(f?.message).toContain(label)
    expect(f?.confidence).toBe(0.95)
  })

  it('truncates the secret in the snippet it records', () => {
    const [f] = only('secret_leak', ctx('key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789'))
    expect(f?.snippet).toBe('Anthropic key: sk-ant-a…')
    expect(f?.snippet).not.toContain('0123456789')
  })

  it('does not fire on credential-shaped prose that is not a credential', () => {
    for (const answer of [
      'Set OPENAI_API_KEY in the environment.',
      'The key format is sk-<random>.',
      'Use an AKIA-prefixed access key id.',
      'Rotate the token in the console.',
    ]) {
      expect(only('secret_leak', ctx(answer))).toEqual([])
    }
  })
})

// These four shapes were MISSED by the shipped detector. github_pat_ is the one
// the workbench itself hands to agents in PAT mode, so an agent echoing its own
// credential into a reply went unflagged.
describe('secret_leak — shapes the audit found missing', () => {
  const missed: Array<[string, string]> = [
    ['GitHub fine-grained token', `github_pat_11ABCDEFG0${'aBcD1234_'.repeat(6)}`],
    ['Stripe secret key', `sk_live_${'a1B2c3D4'.repeat(3)}`],
    ['Stripe secret key', `rk_live_${'a1B2c3D4'.repeat(3)}`],
    ['Slack app token', 'xapp-1-A01234ABCDE-1234567890123-abcdefabcdefabcdefabcdef'],
    ['Credentials in URL', 'postgres://talaria:hunter2correcthorse@db.internal:5432/talaria'],
    ['Credentials in URL', 'https://admin:s3cr3t-p4ss@intranet.corp.example/admin'],
    ['Credentials in URL', 'redis://:an0ther-passw0rd@cache.internal:6379/0'],
  ]

  it.each(missed)('detects a %s', (label, secret) => {
    const [f] = only('secret_leak', ctx(`Here you go: ${secret}`))
    expect(f?.check).toBe('secret_leak')
    expect(f?.message).toContain(label)
  })

  it('does not turn ordinary URLs into credential findings', () => {
    for (const answer of [
      'Fetch https://api.example.com/v1/users?limit=10 for the list.',
      'Clone git@github.com:acme/repo.git first.',
      'Connect to ssh://deploy@build.example.com/srv.',
      'The service listens on https://internal.example.com:8443/health.',
      'Docs: https://example.com/a:b/c.',
    ]) {
      expect(only('secret_leak', ctx(answer))).toEqual([])
    }
  })

  it('does not flag Stripe TEST keys — they are in every tutorial and are not live', () => {
    expect(only('secret_leak', ctx(`use sk_test_${'a1B2c3D4'.repeat(3)} locally`))).toEqual([])
  })
})

// ── pii_leak ────────────────────────────────────────────────────────────────

describe('pii_leak', () => {
  it('detects a plausible SSN and skips the reserved shapes', () => {
    expect(checks(only('pii_leak', ctx('SSN 123-45-6789')))).toEqual(['pii_leak'])
    for (const bad of ['000-45-6789', '666-45-6789', '900-45-6789', '123-00-6789', '123-45-0000']) {
      expect(only('pii_leak', ctx(`SSN ${bad}`))).toEqual([])
    }
  })

  it('detects a Luhn-valid card number and ignores a Luhn-invalid one', () => {
    expect(checks(only('pii_leak', ctx('card 4242424242424242')))).toEqual(['pii_leak'])
    expect(checks(only('pii_leak', ctx('card 4242 4242 4242 4242')))).toEqual(['pii_leak'])
    expect(only('pii_leak', ctx('card 4242424242424243'))).toEqual([])
    expect(only('pii_leak', ctx('order number 1234567890123456'))).toEqual([])
  })

  it('detects an IBAN', () => {
    expect(checks(only('pii_leak', ctx('IBAN DE89370400440532013000')))).toEqual(['pii_leak'])
  })

  it('leaves everyday workspace content alone', () => {
    for (const answer of ['Email jon@example.com or call +1 555 010 1234.', 'Invoice #2026-0731 for $4,200.']) {
      expect(only('pii_leak', ctx(answer))).toEqual([])
    }
  })
})

// ── redaction ───────────────────────────────────────────────────────────────

describe('redactSecrets', () => {
  // A pinned finding is stored beside the message it is about, and
  // `zero_tool_claim` quotes the offending SENTENCE verbatim — so strict mode
  // could scrub `content` and leave the same credential in `guard` on the same
  // row, which is what the agent read path then handed to another model.
  it('scrubs the verbatim snippet a finding carries', () => {
    const key = `sk-ant-${'a'.repeat(24)}`
    const findings = runGuardrails(ctx(`I saved the doc with the key ${key} in your drafts.`), config())
    // Both rules fire on one ordinary sentence, which is the whole problem:
    // `secret_leak` truncates its own snippet to a vendor prefix on purpose,
    // and `zero_tool_claim` quotes the sentence — key included — in full.
    expect(checks(findings)).toContain('zero_tool_claim')
    expect(findings.some((f) => f.snippet.includes(key))).toBe(true)
    for (const f of redactFindings(findings)) expect(f.snippet).not.toContain(key)
  })

  it('reports nothing redacted for clean text', () => {
    expect(redactSecrets('all clear')).toEqual({ text: 'all clear', redacted: false })
  })

  it('swallows a whole private-key block, not just the BEGIN line', () => {
    const block = '-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaA==\nmore==\n-----END OPENSSH PRIVATE KEY-----'
    const { text, redacted } = redactSecrets(`before\n${block}\nafter`)
    expect(redacted).toBe(true)
    expect(text).toBe('before\n[redacted Private key block]\nafter')
  })

  it('swallows an UNTERMINATED private-key block to the end of the text', () => {
    const { text } = redactSecrets('-----BEGIN PRIVATE KEY-----\nMIIB\nMIIB')
    expect(text).toBe('[redacted Private key block]')
  })

  // THE FALSE POSITIVE THAT ATE DOCUMENTS. The unterminated branch swallows to
  // end-of-text, so a SENTENCE that merely names the header line deleted
  // everything after it — and because `output.clean` for a text harness accepts
  // any non-empty string, the truncation came back as a VALID value. The
  // distiller then archived the chat behind a half-written distillation, the
  // librarian overwrote a good OKF with an untagged fragment, and a work session
  // lost the trailing DONE the dispatch loop parses. A line break after the
  // header is what tells PEM from prose.
  it('leaves a sentence that merely names the BEGIN marker alone', () => {
    const runbook = [
      'TLS rotation runbook',
      '',
      '1. Open the bundle.',
      '2. Look for the -----BEGIN PRIVATE KEY----- line near the top.',
      '3. Rotate, then restart the edge nodes.',
    ].join('\n')
    expect(redactSecrets(runbook)).toEqual({ text: runbook, redacted: false })
    // …and it is not a finding either: naming a header is not carrying a key.
    expect(only('secret_leak', ctx(runbook))).toEqual([])
  })

  it('still swallows a real block whose body starts on the next line', () => {
    const { text } = redactSecrets('-----BEGIN PRIVATE KEY-----\nMIIB\nMIIB')
    expect(text).toBe('[redacted Private key block]')
    expect(only('secret_leak', ctx('-----BEGIN PRIVATE KEY-----\nMIIBVgIBADANBg'))).toHaveLength(1)
  })

  it('redacts every occurrence, not just the first', () => {
    const { text } = redactSecrets(`a ${`ghp_${'a'.repeat(36)}`} b ${`ghp_${'b'.repeat(36)}`} c`)
    expect(text).toBe('a [redacted GitHub token] b [redacted GitHub token] c')
  })

  it('redacts the newly-covered shapes too', () => {
    const { text } = redactSecrets(
      `pat ${`github_pat_11ABCDEFG0${'aBcD1234_'.repeat(6)}`} and db postgres://u:p4ssw0rd@db.internal/x`,
    )
    expect(text).toContain('[redacted GitHub fine-grained token]')
    expect(text).toContain('[redacted Credentials in URL]')
    expect(text).not.toContain('p4ssw0rd')
  })

  it('redacts PII alongside secrets', () => {
    const { text, redacted } = redactSecrets('ssn 123-45-6789, card 4242424242424242, iban DE89370400440532013000')
    expect(redacted).toBe(true)
    expect(text).toBe('ssn [redacted SSN], card [redacted card number], iban [redacted IBAN]')
  })

  it('leaves a Luhn-invalid long number alone while redacting a valid one', () => {
    const { text } = redactSecrets('good 4242424242424242 bad 1234567890123456')
    expect(text).toBe('good [redacted card number] bad 1234567890123456')
  })
})

describe('needsRedaction', () => {
  const finding = (check: string): Finding => ({ check, severity: 'high', confidence: 1, message: '', snippet: '' })

  it('is true only for the content checks', () => {
    expect(needsRedaction([finding('secret_leak')])).toBe(true)
    expect(needsRedaction([finding('pii_leak')])).toBe(true)
    expect(needsRedaction([finding('zero_tool_claim'), finding('ungrounded_ref')])).toBe(false)
    expect(needsRedaction([])).toBe(false)
  })
})

// ── runGuardrails plumbing ──────────────────────────────────────────────────

describe('runGuardrails', () => {
  it('returns nothing for an empty answer', () => {
    expect(runGuardrails(ctx(''), config())).toEqual([])
  })

  it('respects per-rule disable', () => {
    const c = ctx('I created the ticket.')
    expect(checks(runGuardrails(c, config()))).toContain('zero_tool_claim')
    expect(runGuardrails(c, config({ checks: { zero_tool_claim: false } }))).toEqual([])
  })

  it('drops findings below minConfidence', () => {
    const c = ctx('See 3f2504e0-4f89-11d3-9a0c-0305e82c3301.', {
      toolRecord: { ...EMPTY_RECORD, backingTools: ['search'], resultsText: 'nothing' },
    })
    expect(checks(runGuardrails(c, config({ minConfidence: 0.7 })))).toContain('ungrounded_ref') // 0.7 hit
    expect(checks(runGuardrails(c, config({ minConfidence: 0.75 })))).not.toContain('ungrounded_ref')
  })

  it('can return several findings for one answer', () => {
    const c = ctx(`I created the ticket. The server is down. Key: sk-ant-api03-${'a'.repeat(24)}`)
    expect(checks(runGuardrails(c, config())).sort()).toEqual(['fabricated_outage', 'secret_leak', 'zero_tool_claim'])
  })

  it('exposes rule metadata matching the registry', () => {
    expect(guardRuleMeta()).toEqual(RULES.map((r) => ({ id: r.id, label: r.label, severity: r.severity, defaultOn: r.defaultOn })))
    expect(RULES.every((r) => r.defaultOn)).toBe(true)
  })

  it('marks exactly the content rules gate-safe (the rest need a tool record)', () => {
    expect(RULES.filter((r) => r.gateSafe).map((r) => r.id)).toEqual(['secret_leak', 'pii_leak'])
  })
})

describe('caveatFor', () => {
  it('is empty with no findings', () => {
    expect(caveatFor([])).toBe('')
  })

  it('renders one bullet per finding with a readable check name', () => {
    const out = caveatFor([{ check: 'zero_tool_claim', severity: 'high', confidence: 0.8, message: 'Claims done.', snippet: 'I created it.' }])
    expect(out).toContain('**zero tool claim:** Claims done. (I created it.)')
    expect(out).toContain('Verify before relying on it.')
  })
})
