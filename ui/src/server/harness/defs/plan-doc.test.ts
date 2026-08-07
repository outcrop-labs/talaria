// The plan document's contract, and above all its DATA-LOSS guard: the model is
// asked for the whole document and its reply replaces one, so every assertion
// here is really the same assertion — a bad reply must never become the plan.
import { describe, expect, it } from 'vitest'
import { cleanPlanDoc, planDocHarness, planDocRegression } from '@/server/harness/defs/plan-doc'
import { runHarness, type TransportRequest } from '@/server/harness/run'

const DOC = [
  '# Plan — Ledger migration',
  '',
  '## Goal',
  'Move the ledger store off SQLite before the quarter ends so the digest and the usage rollups stop contending for one writer.',
  '',
  '## Scope',
  '- The ledger tables only. Usage events move in a later pass.',
  '- No change to the public API surface, and no change to the digest schedule.',
  '',
  '## Decisions',
  '- Postgres over SQLite. Locked; revisited twice and settled.',
  '',
  '## Open questions',
  '- Do we need a read-only window or a full stop?',
].join('\n')

describe('the clean step', () => {
  it('unwraps a document the model fenced', () => {
    expect(cleanPlanDoc('```markdown\n# Plan\n\n## Goal\nShip it.\n```')).toBe('# Plan\n\n## Goal\nShip it.')
  })

  it('drops the narration persona agents write above the heading', () => {
    expect(cleanPlanDoc("Sure — I'll update the plan now.\n\n# Plan\n\n## Goal\nShip it.")).toBe('# Plan\n\n## Goal\nShip it.')
  })

  it('drops that narration even when it mentions a channel or a PR number', () => {
    // The "is there already a heading above this one" test was a substring
    // search for '#', so any '#' inside the narration disabled the strip — and
    // "#platform" and "PR #42" are the two most likely things for an
    // engineering persona to open with. The narration was then saved as the
    // document's first line and indexed into the activity brain.
    const doc = '# Plan\n\n## Goal\nShip it.'
    for (const lead of ['Updating the plan for PR #42 now.', 'Posted this in #platform too.', 'Done — see #123 for context.']) {
      expect(cleanPlanDoc(`${lead}\n\n${doc}`), lead).toBe(doc)
    }
  })

  it('still keeps a document that genuinely opens with a different heading level', () => {
    const doc = '## Context\nWhy this exists.\n\n# Plan\n\n## Goal\nShip it.'
    expect(cleanPlanDoc(doc)).toBe(doc)
  })

  it('keeps a long preamble, because that is somebody’s prose and not narration', () => {
    // The 400-character bound is the line between "the model narrated" and "the
    // document opens with a paragraph". Slicing past it would make this function
    // the cause of the data loss it exists to prevent.
    const preamble = `${'Context that belongs to the document. '.repeat(15)}\n\n# Plan\n\n## Goal\nShip it.`
    expect(cleanPlanDoc(preamble)).toBe(preamble.trim())
  })

  it('fails the contract on an empty reply, which is what keeps the old document', () => {
    expect(cleanPlanDoc('')).toBeNull()
    expect(cleanPlanDoc('   \n\n  ')).toBeNull()
    expect(cleanPlanDoc('```\n\n```')).toBeNull()
  })
})

describe('the regression guard', () => {
  it('saves a faithful rewrite that folds in what changed', () => {
    const next = DOC.replace('## Open questions', '## Next steps\n- Nadia owns the rollback plan.\n\n## Open questions')
    expect(planDocRegression(DOC, next)).toBeNull()
  })

  it('saves the first document, which has nothing to lose', () => {
    expect(planDocRegression('', '# Plan\n\n## Goal\nShip it.')).toBeNull()
  })

  it('refuses a gutted rewrite that keeps a structure of its own', () => {
    const gutted = '# Plan — Ledger migration\n\n## Summary\nWe are moving the ledger to Postgres and there is an open question about the window, plus a rollback plan to write before anything runs in production.'
    expect(planDocRegression(DOC, gutted)).toContain('sections')
  })

  it('refuses a truncated rewrite — sections gone AND shorter than what it was given', () => {
    const truncated = DOC.slice(0, DOC.indexOf('## Decisions'))
    expect(planDocRegression(DOC, truncated)).toContain('shorter')
  })

  it('refuses a hollowed rewrite that kept the headings and threw away the substance', () => {
    const hollow = '# Plan — Ledger migration\n\n## Goal\n\n## Scope\n\n## Decisions\n\n## Open questions'
    expect(planDocRegression(DOC, hollow)).toContain('characters')
  })

  it('allows a heading promoted to a different level', () => {
    // A tidy-up is not damage, and the document is versioned either way.
    expect(planDocRegression(DOC, DOC.replace('## Scope', '### Scope'))).toBeNull()
  })

  it('allows one resolved section to retire when the document still grows', () => {
    const next = DOC.replace('## Open questions\n- Do we need a read-only window or a full stop?', '## Next steps\n- Full stop, 20 minutes, Thursday 02:00 UTC. Nadia owns the rollback plan and Dex runs the migration itself.')
    expect(planDocRegression(DOC, next)).toBeNull()
  })
})

describe('the definition', () => {
  const requests: TransportRequest[] = []
  const reply = (text: string) => ({
    resolveModel: async () => ({ model: 'atlas-planner', step: 'pin' as const }),
    routing: async (model: string) => ({ endpoints: [] as string[], upstreamModel: model }),
    personaKeys: async () => ['spark:qwen-14b'],
    missingCapabilities: async () => [],
    capabilities: async () => ({}),
    transport: async (req: TransportRequest) => {
      requests.push(req)
      return { kind: 'fleet' as const, text, toolNames: [], usage: null, contractDropped: false }
    },
    guardConfig: async () => ({ mode: 'observe' as const, checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }),
    guardText: async () => [],
    recordFindings: async () => {},
    recordRun: async () => {},
    now: () => 0,
  })

  const input = { current: DOC, transcript: 'User: Nadia takes the rollback plan.' }

  it('runs end to end and never asks for protocol JSON', async () => {
    const next = `${DOC}\n\n## Next steps\n- Nadia owns the rollback plan.`
    const res = await runHarness(planDocHarness, input, { caller: 'test', deps: reply(next) })
    expect(res.value).toBe(next)
    expect(res.schemaValid).toBe(true)
    expect(requests.at(-1)?.jsonMode).toBe(false)
    expect(requests.at(-1)?.temperature).toBeUndefined()
  })

  it('hands the model its current document and does not repair a text contract', async () => {
    const res = await runHarness(planDocHarness, input, { caller: 'test', deps: reply('   ') })
    // A FAILED GENERATION RETURNS NOTHING — which is what makes the caller keep
    // the document it already had. Anything else here is data loss.
    expect(res.value).toBeNull()
    expect(res.repairs).toBe(0)
    expect(requests.at(-1)?.messages[1]?.content).toContain('Current document:')
  })

  it('declares the failure policy the document depends on', () => {
    expect(planDocHarness.onFailure).toBe('null')
  })

  it('its own eval fixtures pass on a faithful rewrite', () => {
    const good = [
      '# Plan — Ledger migration',
      '',
      '## Goal',
      'Move the ledger store off SQLite before the quarter ends.',
      '',
      '## Scope',
      '- The ledger tables only.',
      '',
      '## Decisions',
      '- Postgres over SQLite. Locked.',
      '- Nadia owns the rollback plan.',
      '',
      '## Open questions',
      '- Read-only window or a full stop? Deferred to Thursday.',
    ].join('\n')
    for (const e of planDocHarness.evals ?? []) expect(e.check(good), e.name).toBeNull()
  })

  it('its eval fixtures catch the failures they exist for', () => {
    const [retention, folding, shape] = [planDocHarness.evals?.[0], planDocHarness.evals?.[1], planDocHarness.evals?.[2]]
    expect(retention?.check('# Plan\n\n## Goal\nShip the ledger migration.')).toBeTruthy()
    expect(folding?.check('# Plan\n\n## Goal\nShip it.\n\n## Open questions\n- The window is undecided.')).toContain('Nadia')
    expect(shape?.check('Here is the updated plan:\n\n# Plan')).toContain('title heading')
  })
})
