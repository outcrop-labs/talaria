import { describe, expect, it } from 'vitest'
import { NO_TOOLS } from '@/server/harness/define'
import { librarianHarness, type LibrarianOkf } from '@/server/harness/defs/librarian'
import { runHarness, type TransportRequest } from '@/server/harness/run'

const clean = (raw: string): LibrarianOkf | null => {
  const out = librarianHarness.output
  if (out.kind !== 'text' || !out.clean) throw new Error('expected a text harness with a clean step')
  return out.clean(raw)
}

const BODY = ['A weekly release train.', '', '## Key facts', '- Cut is Thursday 17:00 UTC.'].join('\n')

describe('the clean step', () => {
  it('splits the body from the TAGS line', () => {
    const v = clean(`${BODY}\n\nTAGS: release, process, ops`)
    expect(v?.tags).toEqual(['release', 'process', 'ops'])
    expect(v?.body).toBe(BODY)
  })

  it('accepts the decorated forms a small model writes', () => {
    expect(clean(`${BODY}\n\n- **TAGS:** Release Process, OPS`)?.tags).toEqual(['release-process', 'ops'])
  })

  it('keeps the body when the model omits the line', () => {
    const v = clean(BODY)
    expect(v?.body).toBe(BODY)
    expect(v?.tags).toEqual([])
  })

  it('takes the LAST tags line, not one mentioned in the prose', () => {
    const v = clean(`Docs are tagged.\n\n## Key facts\n- The TAGS: prefix is the contract.\n\nTAGS: okf, tagging`)
    expect(v?.tags).toEqual(['okf', 'tagging'])
    expect(v?.body).toContain('The TAGS: prefix is the contract.')
  })

  it('caps at five, dedupes, and drops sentence-length tags', () => {
    const v = clean(`${BODY}\n\nTAGS: a, b, c, d, e, f, a, ${'x'.repeat(50)}`)
    expect(v?.tags).toEqual(['a', 'b', 'c', 'd', 'e'])
  })

  it('fails the contract when there is no body left', () => {
    expect(clean('TAGS: a, b')).toBeNull()
    expect(clean('   ')).toBeNull()
  })
})

describe('the definition', () => {
  const requests: TransportRequest[] = []
  const deps = {
    resolveModel: async () => ({ model: 'pl-main', step: 'pin' as const }),
    routing: async (model: string) => ({ endpoints: ['spark'], upstreamModel: model }),
    missingCapabilities: async () => [],
    // Only a PROBE fact widens (run.ts step 3), so the shape matters here.
    capabilities: async () => ({ 'long-context': { value: true, source: 'probe' as const } }),
    transport: async (req: TransportRequest) => {
      requests.push(req)
      return { kind: 'gateway' as const, text: `${BODY}\n\nTAGS: release, ops`, toolNames: [], usage: null, contractDropped: false }
    },
    guardConfig: async () => ({ mode: 'observe' as const, checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }),
    guardText: async () => [],
    recordFindings: async () => {},
    recordRun: async () => {},
    now: () => 0,
  }

  it('runs end to end and never asks for protocol JSON', async () => {
    const res = await runHarness(librarianHarness, { title: 'Release train', body: 'body text' }, { caller: 'test', deps })
    expect(res.value).toEqual({ body: BODY, tags: ['release', 'ops'] })
    expect(res.schemaValid).toBe(true)
    expect(requests[0]?.jsonMode).toBe(false)
    expect(requests[0]?.temperature).toBe(0.2)
  })

  it('widens on a known long-context model by showing more of the document', async () => {
    const long = 'x'.repeat(20_000)
    const res = await runHarness(librarianHarness, { title: 'T', body: long }, { caller: 'test', deps })
    expect(res.widened).toBe(true)
    expect(requests.at(-1)?.messages[1]?.content).not.toContain('truncated')
  })

  // BY NAME, NOT BY INDEX, and only the fixtures this reply is an answer TO.
  // The suite covers eight documents now, and a summary of the release train is
  // not a summary of the expense policy — every fixture carries its own floor
  // terms, so "passes every fixture" stopped being a meaningful thing to assert.
  const named = (name: string) => {
    const found = (librarianHarness.evals ?? []).find((e) => e.name === name)
    if (!found) throw new Error(`no librarian fixture called "${name}"`)
    return found.check
  }
  const ordinary = () => named('ordinary reference document')
  const meta = () => named('document that talks about itself')

  it('its own eval fixtures pass on a well-formed reply', () => {
    // ONE REPLY PER FIXTURE, because each fixture is about a different document
    // and now says so: a summary of the release train is not an answer to the
    // severity-levels page, and the floor terms are what make that true.
    expect(ordinary()({ body: `${BODY}\n- Ana owns the rota.`, tags: ['release', 'ops'] }, NO_TOOLS)).toBeNull()
    expect(
      meta()(
        { body: '## Key facts\n- SEV1 is a full outage and pages immediately.\n- A SEV1 needs a postmortem within five working days.', tags: ['incidents', 'severity'] },
        NO_TOOLS,
      ),
    ).toBeNull()
  })

  it('its eval fixtures catch the failures they exist for', () => {
    expect(ordinary()({ body: 'Just prose.', tags: ['a'] }, NO_TOOLS)).toContain('Key facts')
    expect(ordinary()({ body: BODY, tags: [] }, NO_TOOLS)).toContain('no TAGS line')
    expect(meta()({ body: '## Key facts\n- SEV1 pages immediately.\n- This page is a DRAFT.', tags: ['a'] }, NO_TOOLS)).toContain('lifecycle commentary')
  })

  it('every fixture rejects a summary about nothing at all', () => {
    // The garbage floor, asserted here as well as in the sweep's own census: a
    // structurally perfect OKF that engages with no document must fail every
    // fixture in the suite, whichever document that fixture is about.
    const empty: LibrarianOkf = { body: '## Key facts\n- Nothing of note.', tags: ['misc'] }
    for (const e of librarianHarness.evals ?? []) expect(e.check(empty, NO_TOOLS)).not.toBeNull()
  })
})

describe('the contradictory-title fixture', () => {
  const fixture = librarianHarness.evals?.find((e) => e.name === 'a document that contradicts its own title')
  const okf = (over: Record<string, unknown> = {}) => ({
    summary: 'This page records how the team communicates and where durable information belongs.',
    body: [
      'This page records how the team communicates and where durable information belongs.',
      '',
      '## Key facts',
      '* Team communication happens in Talaria channels',
      '* Direct messages are for things that need one person',
      '* Anything durable goes in a knowledge doc',
    ].join('\n'),
    tags: ['communication', 'talaria'],
    ...over,
  })

  it('accepts a negation the fixture author did not think to list', () => {
    // THE BUG. The check listed three phrasings — `do not use|no longer|not use
    // slack` — and gemma answered "Slack is not used": correct, engaged with the
    // body, matching none of the three. It was told it had summarized the title.
    // A fixture only certain wordings can pass measures our prompt, not the
    // model.
    for (const phrasing of ['Slack is not used.', 'The org no longer uses Slack.', 'Slack was retired.', 'They do not use Slack.']) {
      expect(fixture?.check(okf({ body: `${okf().body}\n* ${phrasing}` }) as never, NO_TOOLS), phrasing).toBeNull()
    }
  })

  it('still catches a summary that reads the heading instead of the page', () => {
    // The real failure this fixture exists for: presenting Slack as the tool in
    // use, which is what the title says and what the body denies.
    const verdict = fixture?.check(okf({ body: `${okf().body}\n* The team coordinates in Slack channels` }) as never, NO_TOOLS)
    expect(verdict).toContain('presented Slack as the tool in use')
  })
})
