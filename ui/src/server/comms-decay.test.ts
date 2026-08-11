import { beforeEach, describe, expect, it, vi } from 'vitest'
import { concluderHarness } from '@/server/harness/defs/concluder'
import { distillerHarness } from '@/server/harness/defs/distiller'
import type { HarnessResult } from '@/server/harness/run'
import { NO_TOOLS, type CheckResult, type EvalContext } from '@/server/harness/define'

// THE INVARIANT UNDER TEST, and the reason this file exists at all:
// `distillConversation` has three outcomes and two of them mean "archived
// NOTHING". Counting either as an archive is how the substance of a
// conversation is destroyed — the sweep reports eight chats distilled, the
// scrollback goes away, and nothing was ever written to the brain. This module
// has been burned by exactly that bug once already (see the block comment above
// `DistillOutcome`), and the port to `runHarness` moved the two failure signals
// into one result object, so the mapping is now something that can be got wrong
// in a NEW way: `model === null` and `value === null` are different events and
// must land on different outcomes.
//
// Everything the module touches is faked, `runHarness` included, so every
// assertion here is about THIS file's accounting and nothing else. The runner's
// own behavior is covered by harness/run.test.ts.

const sqlLog: string[] = []
const harnessCalls: Array<{ input: unknown; ctx: { caller: string; userId?: string } }> = []
let messageRows: Array<{ role: string; content: string }> = []
let idleRows: Array<{ id: string; userId: string; agentModel: string; title: string | null }> = []
let channelMessages: Array<{ status: string; content: string; authorType: string; author: string }> = []
let harnessResult: Partial<HarnessResult<string>> = {}

/** A postgres.js-shaped tagged template that answers by inspecting the query
 *  text. Keyed on the text rather than call order on purpose: a query this file
 *  stops issuing then stops being answered, instead of silently inheriting the
 *  shape meant for the one before it. */
const sql = (strings: TemplateStringsArray): Promise<unknown> => {
  const text = strings.join(' ').replace(/\s+/g, ' ').trim()
  sqlLog.push(text)
  if (text.includes('from conversations')) return Promise.resolve(idleRows)
  if (text.includes('from messages')) return Promise.resolve(messageRows)
  return Promise.resolve([])
}

const runHarness = vi.fn(async (_def: unknown, input: unknown, ctx: { caller: string; userId?: string }): Promise<HarnessResult<string>> => {
  harnessCalls.push({ input, ctx })
  return {
    value: 'looks fine',
    model: 'pl-main',
    step: 'pin',
    widened: false,
    repairs: 0,
    schemaValid: true,
    answered: true,
    findings: [],
    raw: 'looks fine',
    latencyMs: 1,
    escalate: false,
    ...harnessResult,
  }
})

vi.mock('@/server/db/pg', () => ({ db: async () => sql }))
vi.mock('@/server/harness/run', () => ({ runHarness }))
vi.mock('@/server/gateway', () => ({ describeAgent: (id: string) => ({ label: id }) }))
vi.mock('@/server/scheduler', () => ({ registerJob: () => {} }))
vi.mock('@/server/retrieval/sources', () => ({ indexActivity: async () => {}, indexPersonal: async () => {} }))
vi.mock('@/server/artifacts', () => ({
  agentCategoryFolder: async () => 'folder',
  createArtifact: async () => ({ id: 'artifact' }),
  saveArtifact: async () => {},
}))
vi.mock('@/server/channels', () => ({
  archiveChannel: async () => {},
  insertChannelMessage: async () => {},
  listChannelAgents: async () => ['nomad'],
  listChannelMessages: async () => channelMessages,
}))

const { concludeRelay, distillOutcome, sweepIdleChats } = await import('@/server/comms-decay')

beforeEach(() => {
  sqlLog.length = 0
  harnessCalls.length = 0
  harnessResult = {}
  runHarness.mockClear()
  messageRows = [
    { role: 'user', content: 'ship the ledger migration on Friday' },
    { role: 'assistant', content: 'understood' },
  ]
  idleRows = [{ id: 'conv-1', userId: 'user-1', agentModel: 'nomad', title: 'ledger' }]
  channelMessages = [
    { status: 'complete', content: 'we ship CSV only', authorType: 'human', author: 'Priya' },
    { status: 'complete', content: 'endpoint is up', authorType: 'agent', author: 'nomad' },
  ]
})

const archivedAnything = () => sqlLog.some((q) => q.startsWith('update conversations set archived'))

// ── The mapping, on its own ──────────────────────────────────────────────────

describe('distillOutcome', () => {
  it('maps a null MODEL to no-model', () => {
    // Nothing is configured to summarize with. Every conversation in the batch
    // hits this, it is still true in an hour, and only a human can fix it — so
    // the sweep has to be able to escalate rather than shrug.
    expect(distillOutcome({ model: null, value: null })).toEqual({ ok: false, outcome: 'no-model' })
  })

  it('maps a null VALUE to empty-distillation, not to no-model', () => {
    // A model was asked and could not answer. This one conversation is retried
    // next pass and nobody is paged.
    expect(distillOutcome({ model: 'pl-main', value: null })).toEqual({ ok: false, outcome: 'empty-distillation' })
  })

  it('passes a real distillation through', () => {
    expect(distillOutcome({ model: 'pl-main', value: 'a summary' })).toEqual({ ok: true, value: 'a summary' })
  })

  it('a value with no model behind it is still no-model', () => {
    // The ambiguous case, and the model fact wins: "there is nothing to
    // summarize with" is what makes the whole batch fail and the only one of
    // the two an operator can act on.
    expect(distillOutcome({ model: null, value: 'somehow a value' })).toEqual({ ok: false, outcome: 'no-model' })
  })
})

// ── The sweep, over the mapping it depends on ────────────────────────────────

describe('sweepIdleChats', () => {
  it('archives when the distillation lands', async () => {
    const r = await sweepIdleChats()
    expect(r).toEqual({ considered: 1, archived: 1, skippedNoModel: 0, skippedEmptyDistillation: 0, failed: 0 })
    expect(archivedAnything()).toBe(true)
  })

  it('NEVER archives when no model resolved', async () => {
    harnessResult = { model: null, value: null, step: null, answered: false, schemaValid: false }
    const r = await sweepIdleChats()
    expect(r.skippedNoModel).toBe(1)
    expect(r.archived).toBe(0)
    // The whole point of the file: a conversation archived without its
    // distillation is a conversation deleted with no record of what was in it.
    expect(archivedAnything()).toBe(false)
  })

  it('NEVER archives when the distillation came back empty', async () => {
    harnessResult = { value: null, schemaValid: false }
    const r = await sweepIdleChats()
    expect(r.skippedEmptyDistillation).toBe(1)
    expect(r.skippedNoModel).toBe(0)
    expect(r.archived).toBe(0)
    expect(archivedAnything()).toBe(false)
  })

  it('accounts for every conversation it considered', async () => {
    // The property that makes the hourly log line checkable rather than
    // decorative: considered === archived + the three ways it did nothing.
    harnessResult = { value: null, schemaValid: false }
    const r = await sweepIdleChats()
    expect(r.archived + r.skippedNoModel + r.skippedEmptyDistillation + r.failed).toBe(r.considered)
  })

  it('runs the distiller as the chat OWNER, so their preferred model is in play', async () => {
    await sweepIdleChats()
    expect(harnessCalls[0]?.ctx).toEqual({ caller: 'platform:distiller:user-1', userId: 'user-1' })
  })

  it('archives an empty conversation without asking for a model at all', async () => {
    // A chat with nothing in it has no substance to lose, so the
    // never-archive-on-failure rule has nothing to protect here. Before the
    // harness port this row needed a model to resolve before it could archive,
    // which left it stuck for ever on an install that had none.
    messageRows = []
    const r = await sweepIdleChats()
    expect(runHarness).not.toHaveBeenCalled()
    expect(r.archived).toBe(1)
    expect(archivedAnything()).toBe(true)
  })

  it('one bad conversation does not abandon the rest of the batch', async () => {
    idleRows = [
      { id: 'conv-1', userId: 'user-1', agentModel: 'nomad', title: null },
      { id: 'conv-2', userId: 'user-2', agentModel: 'nomad', title: null },
    ]
    runHarness.mockImplementationOnce(async () => {
      throw new Error('gateway 503')
    })
    const r = await sweepIdleChats()
    expect(r).toMatchObject({ considered: 2, archived: 1, failed: 1 })
  })
})

// ── The relay side, which throws instead of swallowing ───────────────────────

describe('concludeRelay', () => {
  it('returns the summary', async () => {
    await expect(concludeRelay('chan-1', 'user-1', 'pilot-export')).resolves.toBe('looks fine')
  })

  it('says NO MODEL rather than "came back empty" when nothing resolved', async () => {
    // Two failures, two sentences, and somebody is watching a spinner. This
    // caller maps them by hand because they are USER-FACING COPY on the conclude
    // button — `onFailure: 'throw'` would now cover both, and would show the
    // person a sentence written for a `harness_runs` row.
    harnessResult = { model: null, value: null, step: null, answered: false, schemaValid: false }
    await expect(concludeRelay('chan-1', 'user-1', 'pilot-export')).rejects.toThrow(/no model configured to summarize with/)
  })

  it('says CAME BACK EMPTY when a model answered with nothing', async () => {
    harnessResult = { value: null, schemaValid: false }
    await expect(concludeRelay('chan-1', 'user-1', 'pilot-export')).rejects.toThrow(/came back empty/)
  })

  it('says what the GATEWAY said when the model never answered at all', async () => {
    // Three outcomes, not two. `runHarness` also returns for a transport
    // failure, and folding that into "came back empty — try again" told a user
    // whose provider was rate limiting to retry into the same rate limit, while
    // the only sentence that explains it sat on the `harness_runs` row.
    // `answered` is how the two are told apart. It used to be `raw === null`,
    // which a stream that died three tokens in would have answered wrongly —
    // hence the partial here. Pre-port this button showed the gateway's message.
    harnessResult = { value: null, answered: false, raw: 'Here is the', schemaValid: false, error: 'harness "concluder" could not reach "pl-main": gateway completion 429: rate limit exceeded' }
    await expect(concludeRelay('chan-1', 'user-1', 'pilot-export')).rejects.toThrow(/429: rate limit exceeded/)
  })

  it('refuses a relay with no messages before it spends a model call', async () => {
    channelMessages = []
    await expect(concludeRelay('chan-1', 'user-1', 'pilot-export')).rejects.toThrow(/nothing to conclude/)
    expect(runHarness).not.toHaveBeenCalled()
  })

  it('runs the concluder as the CONCLUDING user', async () => {
    await concludeRelay('chan-1', 'user-1', 'pilot-export')
    expect(harnessCalls[0]?.ctx).toEqual({ caller: 'platform:concluder:user-1', userId: 'user-1' })
  })
})

// ── The eval fixtures themselves ─────────────────────────────────────────────
//
// An eval whose check never fails is a green cell in the fitness matrix that
// means nothing, and nobody finds that out until they trust it. These run each
// declared check against a hand-written good answer and a hand-written bad one,
// so the fixtures are known to DISCRIMINATE before any model is scored on them.

const checksOf = <I, O>(def: { evals?: Array<{ name: string; input: I; check: (v: O, ctx: EvalContext) => CheckResult }> }) =>
  (def.evals ?? []).map((e) => ({ ...e, check: (v: O) => e.check(v, NO_TOOLS) }))

describe('distiller evals', () => {
  const good = ['- Ledger store: Postgres over SQLite (locked)', '- Ledger migration ships Friday', '- Nadia owns the rollback plan'].join('\n')

  // BY NAME, NOT BY INDEX. These used to be `[0]` and `[1]`, which silently
  // re-pointed at different fixtures the moment the suite grew — and the failure
  // read as "the distiller check is wrong" rather than "this test is holding the
  // wrong fixture".
  const named = (name: string) => {
    const found = checksOf(distillerHarness).find((e) => e.name === name)
    if (!found) throw new Error(`no distiller fixture called "${name}"`)
    return found.check
  }
  const planted = () => named('keeps the planted decisions and drops the planted pleasantries')

  it('passes a faithful distillation of the transcript it is about', () => {
    // Only the fixtures that RUN on `FIXTURE`: the suite covers several
    // transcripts now, and a distillation of one is not an answer to another.
    expect(planted()(good)).toBeNull()
    expect(named('is shorter than the conversation it distills')(good)).toBeNull()
  })

  it('fails one that lost a decision', () => {
    expect(planted()('- Ledger store: Postgres over SQLite')).toMatch(/friday|nadia/i)
  })

  it('fails one that kept the pleasantries', () => {
    expect(planted()(`${good}\n- Hope you had a good weekend`)).toMatch(/pleasantries/)
  })

  it('fails a "distillation" that just restates the transcript', () => {
    // A RATIO, NOT A RAW COMPARISON. The fixture's transcript is a real DM now
    // (~2.5k characters), so "restated it" means "came back at most of that
    // size" rather than "came back longer than it" — a model that hands back 90%
    // of the conversation has restated it however you measure.
    const restated = `${'User: Morning! Hope you had a good weekend. Postgres Friday Nadia rollback ledger. '.repeat(40)}`
    const verdict = named('is shorter than the conversation it distills')(restated)
    expect(typeof verdict === 'string' && verdict).toMatch(/restated the conversation/)
  })

  it('accepts a real distillation of a real conversation', () => {
    // THE OTHER HALF, and the reason the fixtures were rewritten. The old
    // transcript was 375 characters, so a correct WIDENED answer — headings, as
    // the widened prompt demands — could legitimately exceed it and be scored a
    // failure for obeying the instruction we gave it.
    const real = [
      '## Decisions',
      '- Ledger store: Postgres over SQLite. Locked.',
      '- Ledger migration ships Friday, ahead of the release cut.',
      '- Connection pooler (pgbouncer) deferred to its own ticket, no date.',
      '- Ledger UI is explicitly out of scope this quarter.',
      '## Outcomes',
      '- Nadia owns the rollback plan; Nomad to send her the runbook link.',
    ].join('\n')
    expect(named('is shorter than the conversation it distills')(real)).toBeNull()
  })
})

describe('concluder evals', () => {
  const good = ['## Decided', '- CSV only for the pilot, no XLSX', '## Produced', '- The export endpoint and its fixtures', '## Follow-ups', '- Customer-facing note before Thursday'].join('\n')

  // BY NAME, NOT BY INDEX — see the distiller block above. The suite covers
  // several relays now, and a summary of one is not an answer to another.
  const named = (name: string) => {
    const found = checksOf(concluderHarness).find((e) => e.name === name)
    if (!found) throw new Error(`no concluder fixture called "${name}"`)
    return found.check
  }

  it('passes a sectioned summary that carries all three', () => {
    // Only the fixtures that run on the pilot-export relay this summary is of.
    expect(named('comes back as sections rather than a paragraph')(good)).toBeNull()
    expect(named('carries the decision, the deliverable and the follow-up')(good)).toBeNull()
  })

  it('fails a summary that came back as prose', () => {
    expect(named('comes back as sections rather than a paragraph')('We agreed to ship CSV, the endpoint is done, and a note is due Thursday.')).toMatch(/prose/)
  })

  it('fails a summary that dropped the follow-up', () => {
    expect(named('carries the decision, the deliverable and the follow-up')('## Decided\n- CSV only\n## Produced\n- the endpoint')).toMatch(/follow-up/)
  })

  it('fails an empty summary', () => {
    expect(named('comes back as sections rather than a paragraph')('   ')).toMatch(/empty/)
  })
})
