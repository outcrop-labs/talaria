import { describe, expect, it } from 'vitest'
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

  it('its own eval fixtures pass on a well-formed reply', () => {
    const good: LibrarianOkf = { body: `${BODY}\n- Ana owns the rota.`, tags: ['release', 'ops'] }
    for (const e of librarianHarness.evals ?? []) expect(e.check(good)).toBeNull()
  })

  it('its eval fixtures catch the failures they exist for', () => {
    const first = librarianHarness.evals?.[0]
    const second = librarianHarness.evals?.[1]
    expect(first?.check({ body: 'Just prose.', tags: ['a'] })).toContain('Key facts')
    expect(first?.check({ body: BODY, tags: [] })).toContain('no TAGS line')
    expect(second?.check({ body: `${BODY}\n- This page is a DRAFT.`, tags: ['a'] })).toContain('lifecycle commentary')
  })
})
