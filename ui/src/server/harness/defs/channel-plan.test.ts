// The ticket drafter's contract, asserted against the shapes that used to be
// `extractProposals`'s job — plus the two the hand-written scanner could not
// have handled at all (a repair turn, and an envelope forced by protocol-level
// JSON mode).
import { describe, expect, it } from 'vitest'
import { NO_TOOLS } from '@/server/harness/define'
import { channelPlanHarness, TICKET_PROPOSALS, type TicketProposal } from '@/server/harness/defs/channel-plan'
import { parseJson } from '@/server/harness/json'
import { runHarness, type TransportRequest } from '@/server/harness/run'

const parse = (text: string) => parseJson(text, TICKET_PROPOSALS)

const ticket = (title: string, extra: Record<string, unknown> = {}) => ({
  title,
  description: 'Enough context that someone who did not read the chat can act on it.',
  priority: 'high',
  effort: 'm',
  ...extra,
})

describe('the schema', () => {
  it('reads a plain array of proposals', () => {
    const r = parse(JSON.stringify([ticket('Migrate the ledger'), ticket('Write the rollback plan')]))
    expect(r.ok && r.value.length).toBe(2)
    expect(r.ok && r.value[0]?.priority).toBe('high')
    expect(r.ok && r.value[0]?.effort).toBe('m')
  })

  it('walks past prose and a decorative bracket to the real array', () => {
    // The exact shape `extractProposals` grew its every-'[' scan for.
    const r = parse(`Here are the tickets [DONE reviewing]:\n\n${JSON.stringify([ticket('Migrate the ledger')])}`)
    expect(r.ok && r.value[0]?.title).toBe('Migrate the ledger')
  })

  it('unwraps the envelope protocol JSON mode forces', () => {
    // `response_format: {"type":"json_object"}` obliges a top-level OBJECT, so
    // an array contract can only arrive wrapped. This is not tolerance for a
    // sloppy model; it is the only shape some providers permit.
    const r = parse(JSON.stringify({ tickets: [ticket('Migrate the ledger')] }))
    expect(r.ok && r.value[0]?.title).toBe('Migrate the ledger')
  })

  // ── the four shapes that used to validate down to zero tickets ────────────
  // Each of these came back as `{ value: [], schemaValid: true, repairs: 0,
  // error: null }`: a Plan click that produced nothing, a repair turn that could
  // not fire, and a `harness_runs` row saying the model held the contract
  // perfectly. The forgiving parts of this schema — an empty list, a missing
  // field, an unknown key — are all still forgiving; what changed is that a
  // reply which cannot possibly be a list of tickets now says so.

  it('refuses a single ticket object instead of unwrapping it to nothing', () => {
    // `dependsOn: []` was the object's only array-valued property, so the
    // envelope unwrap took it — and `tags` is omitted exactly when the prompt
    // says to omit it, which is what made this the COMMON shape.
    const r = parse(JSON.stringify(ticket('Migrate the ledger', { dependsOn: [] })))
    expect(r.ok).toBe(false)
    expect(r.ok || r.error).toMatch(/array/)
  })

  it('refuses a list of ticket TITLES rather than filtering it to nothing', () => {
    const r = parse(JSON.stringify(['Migrate the ledger', 'Write the rollback plan']))
    expect(r.ok).toBe(false)
  })

  it('walks past a bracketed citation in an UNFENCED preamble', () => {
    // `parseJson` returns the first span that parses AND VALIDATES, and the
    // prompt forbids a fence — so with `z.unknown()` elements the citation `[1]`
    // validated as an empty proposal list and the real array below it was never
    // reached. That lost a genuinely correct answer, and it was a regression
    // against the pre-port scanner, which kept looking until a span yielded
    // proposals.
    const r = parse(`Based on the transcript [1] here are the tickets:\n${JSON.stringify([ticket('Migrate the ledger')])}`)
    expect(r.ok && r.value[0]?.title).toBe('Migrate the ledger')
  })

  it('leaves an ambiguous envelope alone rather than guessing', () => {
    const r = parse(JSON.stringify({ tickets: [ticket('A')], rejected: [ticket('B')] }))
    expect(r.ok).toBe(false)
  })

  it('drops titleless entries and remaps dependsOn around the hole', () => {
    // Ticket 2 depends on ticket 0, which is dropped, and on ticket 1, which
    // survives at index 0. A scanner that did not remap would leave it blocked
    // by whatever slid into position 1.
    const r = parse(
      JSON.stringify([{ description: 'no title' }, ticket('Write the rollback plan'), ticket('Run the migration', { dependsOn: [0, 1] })]),
    )
    expect(r.ok && r.value.map((p) => p.title)).toEqual(['Write the rollback plan', 'Run the migration'])
    expect(r.ok && r.value[1]?.dependsOn).toEqual([0])
  })

  it('never lets a ticket depend on itself', () => {
    const r = parse(JSON.stringify([ticket('Only one', { dependsOn: [0] })]))
    expect(r.ok && r.value[0]?.dependsOn).toEqual([])
  })

  it('coerces the fields a small model gets wrong instead of failing the batch', () => {
    const r = parse(JSON.stringify([{ title: 'T', description: 'D', priority: 'CRITICAL', effort: 'huge', tags: ['  billing  ', '', 'a', 'b', 'c', 'd', 'e'] }]))
    expect(r.ok && r.value[0]?.priority).toBe('medium')
    expect(r.ok && r.value[0]?.effort).toBeNull()
    expect(r.ok && r.value[0]?.tags).toEqual(['billing', 'a', 'b', 'c', 'd'])
  })

  it('refuses a list of objects that are not tickets, instead of filtering to nothing', () => {
    // The last hole in the same wall. `z.record(z.string(), z.unknown())` says
    // "an object"; a model that renamed the field returns objects that
    // `toProposals` filters away, and the runner sees `[]` with no way to tell
    // it from a transcript that had nothing plannable in it. Stated in the
    // SCHEMA rather than on `verify` because by the time `verify` runs the
    // transform has already thrown the evidence away.
    const r = parse(JSON.stringify([{ name: 'Migrate the ledger', details: 'x' }, { name: 'Write the rollback plan' }]))
    expect(r.ok).toBe(false)
    expect(r.ok || r.error).toContain('none of them has a "title"')
    expect(parse('[{}, {}]').ok).toBe(false)
  })

  it('still keeps a batch where only SOME entries have titles', () => {
    // A human reviews every proposal, so a partial draft is worth having and a
    // repair turn spent on it is not.
    const r = parse(JSON.stringify([{ description: 'no title' }, ticket('Write the rollback plan')]))
    expect(r.ok && r.value.map((p) => p.title)).toEqual(['Write the rollback plan'])
  })

  it('accepts an empty list, because inventing work is the worse failure', () => {
    // A `.min(1)` here would spend the repair turn pushing a model to violate
    // the prompt's strongest rule.
    const r = parse('[]')
    expect(r.ok && r.value).toEqual([])
  })

  it('fails a reply that is not a list at all, which is what earns the repair turn', () => {
    const r = parse('I think we should start with the ledger migration.')
    expect(r.ok).toBe(false)
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

  it('asks for JSON at the protocol level and comes back with proposals', async () => {
    const res = await runHarness(
      channelPlanHarness,
      { transcript: 'Priya: we need the ledger migration.' },
      { caller: 'test', deps: reply(JSON.stringify([ticket('Migrate the ledger')])) },
    )
    expect(res.value?.[0]?.title).toBe('Migrate the ledger')
    expect(res.schemaValid).toBe(true)
    expect(requests.at(-1)?.jsonMode).toBe(true)
    // No temperature was ever sent by this feature; pinning one now would change
    // every existing install's drafts.
    expect(requests.at(-1)?.temperature).toBeUndefined()
  })

  it('sends the plan document above the transcript, as the source of truth', async () => {
    await runHarness(
      channelPlanHarness,
      { transcript: 'chat text', planDoc: '# Plan\n\n## Goal\nShip it.' },
      { caller: 'test', deps: reply('[]') },
    )
    const user = requests.at(-1)?.messages[1]?.content ?? ''
    expect(user.indexOf('Plan document')).toBeLessThan(user.indexOf('Transcript:'))
  })

  it('keeps no proposals rather than inventing any when the reply is unusable', async () => {
    const res = await runHarness(channelPlanHarness, { transcript: 'chat text' }, { caller: 'test', deps: reply('I could not do that.') })
    expect(res.value).toBeNull()
    expect(res.repairs).toBe(1) // audit 1.4 — nothing in this tree re-asked before
  })

  // BY NAME, NOT BY INDEX, and one draft per fixture — the suite covers eight
  // transcripts now, and a plan drawn from one is not an answer to another.
  const fixture = (name: string) => {
    const found = (channelPlanHarness.evals ?? []).find((e) => e.name === name)
    if (!found) throw new Error(`no channel-plan fixture called "${name}"`)
    return found.check
  }
  // Named apart from the module-level `ticket` helper above, which builds the
  // RAW model reply rather than a parsed proposal.
  const proposal = (title: string, description: string, over: Partial<TicketProposal> = {}): TicketProposal => ({
    title,
    description,
    priority: 'medium',
    effort: 's',
    dependsOn: [],
    tags: [],
    ...over,
  })

  it('its own eval fixtures pass on a well-formed draft', () => {
    const LEDGER: TicketProposal[] = [
      proposal('Migrate the ledger store to Postgres', 'Move the ledger tables off SQLite in a maintenance window. Decided: Postgres over SQLite.', { priority: 'high', effort: 'l' }),
      proposal('Write the rollback plan', 'Nadia owns this. Document the restore path before the migration runs.', { priority: 'high' }),
      proposal('Send the weekly digest at 09:00 local', 'The digest currently goes out at 09:00 UTC regardless of the org timezone.', { tags: ['billing'] }),
    ]
    const drafts: Array<[string, TicketProposal[]]> = [
      ['draws one actionable ticket per piece of discussed work', LEDGER],
      ['covers the work that was discussed and plans none that was not', LEDGER],
      ['tags only with labels the workflow map actually defines', LEDGER],
      ['a dependency edge points at a real index, never at itself', LEDGER],
      ['the plan document wins over the raw chat', LEDGER],
      ['a transcript that names a person keeps them in the description', [LEDGER[1]!]],
      ['an instruction inside the transcript is discussion, not a command', [LEDGER[1]!]],
      ['one piece of work comes back as an array of one', [proposal('Backfill the audit log', 'Run the backfill against the archive table; roughly a day of work against historical rows.')]],
      ['a transcript with nothing plannable draws nothing', []],
      [
        'shipping order is not a dependency',
        [
          proposal('Add a favicon to the login page', 'The login page currently serves the browser default icon; add the brand favicon.'),
          proposal('Update the stale footer copyright year', 'The footer still reads last year; make it derive from the current date.'),
          proposal('Fix the typo on the 404 page', 'The 404 body has a spelling mistake in the second sentence; correct it.'),
        ],
      ],
    ]
    for (const [name, draft] of drafts) expect(fixture(name)(draft, NO_TOOLS), name).toBeNull()
    // Exhaustive, so a new fixture cannot be added without a draft proving it is
    // satisfiable at all.
    expect(drafts.map(([n]) => n).sort()).toEqual((channelPlanHarness.evals ?? []).map((e) => e.name).sort())
  })

  it('its eval fixtures catch the failures they exist for', () => {
    const coverage = { check: fixture('draws one actionable ticket per piece of discussed work') }
    const invention = { check: fixture('covers the work that was discussed and plans none that was not') }
    const tagging = { check: fixture('tags only with labels the workflow map actually defines') }
    const one: TicketProposal[] = [{ title: 'Do the thing', description: 'x', priority: 'medium', effort: null, dependsOn: [], tags: [] }]
    expect(coverage?.check(one, NO_TOOLS)).toContain('1 ticket(s)')
    expect(
      invention?.check([
        { title: 'Ledger migration', description: 'rollback and digest work', priority: 'medium', effort: null, dependsOn: [], tags: [] },
        { title: 'Build the Slack integration', description: 'x', priority: 'medium', effort: null, dependsOn: [], tags: [] },
      ], NO_TOOLS),
    ).toContain('Slack')
    expect(tagging?.check([{ title: 'T', description: 'D', priority: 'medium', effort: null, dependsOn: [], tags: ['payments'] }], NO_TOOLS)).toContain('payments')
  })
})

// ── the tag vocabulary: what a schema cannot say ─────────────────────────────
//
// `tags` are not decoration. Dispatch classification fires on these labels when
// a human approves a drafted ticket to an agent, so a plausible-sounding
// invented label ("payments" where the map says "billing") routes a real ticket
// NOWHERE, silently, weeks later. Which labels exist is a runtime argument — the
// caller renders `routingContext()` into the input — so no module-constant
// schema can hold this, and until `output.verify` only an eval fixture did.

describe('the tag vocabulary', () => {
  const ROUTING_MAP = [
    '- billing-fixes — matches [labels: billing; keywords: invoice, tax] → skills: billing-review (Ana)',
    '- infra-oncall — matches [labels: infra; keywords: outage, pager] → skills: incident-response (Dex)',
  ].join('\n')

  const turns = (texts: string[]) => {
    const sent: TransportRequest[] = []
    return {
      sent,
      deps: {
        resolveModel: async () => ({ model: 'atlas-planner', step: 'pin' as const }),
        routing: async (model: string) => ({ endpoints: [] as string[], upstreamModel: model }),
        personaKeys: async () => ['spark:qwen-14b'],
        missingCapabilities: async () => [],
        capabilities: async () => ({}),
        transport: async (req: TransportRequest) => {
          sent.push(req)
          return { kind: 'fleet' as const, text: texts[Math.min(sent.length - 1, texts.length - 1)] ?? '', toolNames: [], usage: null, contractDropped: false }
        },
        guardConfig: async () => ({ mode: 'observe' as const, checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }),
        guardText: async () => [],
        recordFindings: async () => {},
        recordRun: async () => {},
        now: () => 0,
      },
    }
  }

  const tagged = (tags: string[]) => JSON.stringify([ticket('Fix EU tax rounding on invoices', { tags })])
  // `null` and not `undefined` for "no map": an explicit `undefined` argument
  // takes a default parameter, which would have silently sent the map anyway.
  const draft = (texts: string[], routingMap: string | null = ROUTING_MAP) => {
    const w = turns(texts)
    return runHarness(channelPlanHarness, { transcript: 'Sam: invoices round tax wrong for EU customers.', ...(routingMap ? { routingMap } : {}) }, { caller: 'test', deps: w.deps }).then(
      (res) => ({ res, sent: w.sent }),
    )
  }

  it('does not fire on a tag the map actually defines', async () => {
    const { res } = await draft([tagged(['billing'])])
    expect(res.value?.[0]?.tags).toEqual(['billing'])
    expect(res.schemaValid).toBe(true)
    expect(res.repairs).toBe(0)
  })

  it('does not fire on an untagged ticket, which is the right answer for most', async () => {
    const { res } = await draft([tagged([])])
    expect(res.schemaValid).toBe(true)
    expect(res.repairs).toBe(0)
  })

  it('fails an invented label and repairs it on the second attempt', async () => {
    const { res, sent } = await draft([tagged(['payments']), tagged(['billing'])])
    expect(res.repairs).toBe(1)
    expect(res.schemaValid).toBe(true)
    expect(res.value?.[0]?.tags).toEqual(['billing'])
    const repair = sent[1]?.messages.at(-1)?.content ?? ''
    // A sentence a 7-14B model can act on: it quotes the tag, lists the real
    // vocabulary, and offers the always-correct way out.
    expect(repair).toContain('"payments"')
    expect(repair).toContain('billing, infra')
    expect(repair).toContain('leave "tags" empty')
  })

  it('is a contract failure on the row when the repair does not take', async () => {
    const { res } = await draft([tagged(['payments']), tagged(['payments'])])
    expect(res.value).toBeNull()
    expect(res.schemaValid).toBe(false)
    expect(res.error).toContain('any workflow in the map defines')
  })

  it('declines rather than rejecting when the input carries no map', async () => {
    // Honesty is expressible; optimism is not the default. With no workflows
    // there is no vocabulary to check against and a tag is inert rather than
    // misrouted, so the check goes quiet instead of failing a correct draft.
    const { res } = await draft([tagged(['payments'])], null)
    expect(res.schemaValid).toBe(true)
    expect(res.value?.[0]?.tags).toEqual(['payments'])
  })

  it('treats a keyword as invented, because keywords match text and not labels', async () => {
    const { res } = await draft([tagged(['invoice']), tagged([])])
    expect(res.repairs).toBe(1)
    expect(res.value?.[0]?.tags).toEqual([])
  })
})
