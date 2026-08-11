// WHAT THE PLAN BUTTON SAYS WHEN IT FAILS, which is the half a harness test
// cannot cover: `runHarness` reports a failure in HARNESS terms and returns it
// rather than throwing, and this adapter decides which of two user-facing
// sentences the route reaches for. It had two branches for three outcomes, so a
// transport failure — a restarting agent container, a 429, a socket reset —
// came back as `200 { note: 'nothing to plan yet' }` on a channel full of work.
// Pre-port `proxyChat` threw `gateway error 502` and both routes turned it into
// a 502 carrying that sentence.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { HarnessResult } from '@/server/harness/run'
import type { TicketProposal } from '@/server/harness/defs/channel-plan'

let result: Partial<HarnessResult<TicketProposal[]>> = {}

const runHarness = vi.fn(async (): Promise<HarnessResult<TicketProposal[]>> => ({
  value: [],
  model: 'nomad',
  step: 'pin',
  refused: false,
  widened: false,
  repairs: 0,
  schemaValid: true,
  answered: true,
  findings: [],
  raw: null,
  latencyMs: 1,
  escalate: false,
  ...result,
}))

vi.mock('@/server/harness/run', () => ({ runHarness }))
vi.mock('@/server/gateway', () => ({ describeAgent: (id: string) => ({ label: id }) }))
vi.mock('@/server/channels', () => ({
  listChannelMessages: async () => [{ status: 'complete', content: 'we agreed on Postgres', authorType: 'human', author: 'Priya' }],
}))
vi.mock('@/server/conversations', () => ({ priorMessages: async () => [] }))
vi.mock('@/server/plan-doc', () => ({ planDocFor: async () => null, planTier: () => null }))
vi.mock('@/server/templates', () => ({ resolveTemplate: async () => null, templatePrompt: () => '' }))
vi.mock('@/server/workflows', () => ({ routingContext: async () => null }))

const { planFromChannel } = await import('@/server/channel-plan')

beforeEach(() => {
  result = {}
  runHarness.mockClear()
})

const plan = () => planFromChannel('chan-1', 'nomad', 'nomad')

describe('planFromTranscript', () => {
  it('hands back the proposals a run produced', async () => {
    result = { value: [{ title: 'Migrate the ledger', description: 'd', priority: 'high', effort: 'm', tags: [], dependsOn: [] }], raw: '[…]' }
    expect((await plan()).proposals).toHaveLength(1)
  })

  it('reports NOTHING TO PLAN when the model answered with an empty list', async () => {
    // A transcript with nothing plannable in it draws no tickets, and that is a
    // correct answer rather than a failure — the model spoke, so `raw` is set.
    result = { value: [], raw: '[]' }
    const { proposals, raw } = await plan()
    expect(proposals).toEqual([])
    expect(raw).toBe('[]')
  })

  it('keeps "did not return parseable tickets" for a model that answered badly', async () => {
    // The contract failed but the model DID answer, which is the case
    // `answered` tells apart. The route's note is right and a 502 would be wrong.
    result = { value: null, answered: true, raw: 'Sure! I will get right on that.', schemaValid: false, error: 'the JSON value was opened but never closed' }
    const { proposals, raw } = await plan()
    expect(proposals).toEqual([])
    expect(raw).toBe('Sure! I will get right on that.')
  })

  it('throws the runner’s own sentence when the model was never reached', async () => {
    // `answered: false` is the whole test now — a run that never got a reply.
    // `raw` is set alongside it deliberately: a stream that dies after three
    // tokens leaves a partial behind, and the old `raw === null` test read that
    // as a model that answered badly and swallowed the gateway's sentence.
    result = { value: null, answered: false, raw: 'Sure, I’ll', schemaValid: false, error: 'harness "channel-plan" could not reach "nomad": persona gateway 502' }
    await expect(plan()).rejects.toThrow(/persona gateway 502/)
  })
})
