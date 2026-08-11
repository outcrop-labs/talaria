// THE EMPTY REPORT. `researchSynthesisHarness` declares `onFailure: 'throw'`
// and its own comment says why: "the searches are already paid for and the run's
// only deliverable is this document". `onFailure` used to be consulted only
// after a CONTRACT failure — `runHarness` returned, and did not throw, for
// everything that happened before or during the call (nothing resolved, render
// threw, the transport died). So a persona gateway answering 502 mid-synthesis
// arrived at `synthesis.value ?? ''`, and the run saved an artifact containing
// nothing but the Sources list, marked itself `done`, indexed the empty report
// into the brain and notified the requester — with the gateway's sentence
// dropped. 'throw' now means every failure to produce a value, and the mock
// below honors it, so what this file asserts is the half that is still this
// pipeline's own: a throw out of the synthesis stage leaves NO artifact and
// marks the run errored with the gateway's sentence on it.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { HarnessResult } from '@/server/harness/run'

const queries: Array<{ text: string; values: unknown[] }> = []
const saved: Array<{ id: string; body: string }> = []
let synthesis: Partial<HarnessResult<string>> = {}

const RUN = {
  id: 'run-1',
  status: 'queued',
  mode: 'recon',
  question: 'what changed in postgres 17',
  agentModel: 'nomad',
  ownerUserId: 'user-1',
  requestedBy: 'user-1',
}

const sql = ((strings: TemplateStringsArray, ...values: unknown[]) => {
  const text = strings.join(' ').replace(/\s+/g, ' ').trim()
  queries.push({ text, values })
  if (text.includes('insert into research_runs')) return Promise.resolve([RUN])
  return Promise.resolve([])
}) as unknown as {
  (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown[]>
  json: (v: unknown) => unknown
  unsafe: (text: string, values?: unknown[]) => unknown
}
sql.json = (v: unknown) => v
// Two jobs, as in postgres.js: a bare fragment inside a template (the RETURNING
// column list) and a whole statement (`getResearchRun`'s row read).
sql.unsafe = (text: string) => (/^\s*select/i.test(text) ? Promise.resolve([RUN]) : text)

/** The search stages answer with one usable source; only the SYNTHESIS stage is
 *  the subject, so everything before it succeeds. */
const runHarness = vi.fn(async (def: { id: string; onFailure: unknown }, _input: unknown, ctx: { deps?: { transport?: unknown } }): Promise<HarnessResult<unknown>> => {
  const base = { model: 'nomad', step: 'pin' as const, widened: false, repairs: 0, schemaValid: true, answered: true, findings: [], raw: 'x', latencyMs: 1, escalate: false }
  if (def.id === 'research-search') {
    // The real search transport is what records sources; the adapter passes it
    // in, so calling it is how a fixture supplies one.
    await (ctx.deps?.transport as (r: unknown) => Promise<unknown>)?.({ model: 'sonar', messages: [], jsonMode: false, caller: 'r' })
    return { ...base, value: 'the vendor published a SOC 2 Type II [1]' }
  }
  if (def.id === 'research-queries') return { ...base, value: ['what changed'] }
  const result = { ...base, value: 'a report', ...synthesis } as HarnessResult<unknown>
  // THE POLICY THE REAL RUNNER APPLIES, reproduced rather than skipped. A mock
  // that hands back `value: null` for a harness declaring 'throw' is a mock of a
  // runner that does not exist, and it would let this file go on passing if the
  // guarantee were removed — which is the exact failure the header describes.
  if (result.value === null && def.onFailure === 'throw') throw new Error(result.error ?? 'the harness produced no value')
  return result
})

vi.mock('@/server/db/pg', () => ({ db: async () => sql }))
// `capabilityKeysFor` is the runner's own key derivation, which `searchStage`
// now asks so it can pick between the native and the tool-driven search
// transport. Stubbed to "no keys", which lands the reach check on "nothing has
// measured this" and therefore on the native path — the behaviour every case in
// this file was written against.
vi.mock('@/server/harness/run', () => ({ runHarness, capabilityKeysFor: async () => [] }))
vi.mock('@/server/gateway', () => ({ describeAgent: (id: string) => ({ label: id }) }))
vi.mock('@/server/scheduler', () => ({ registerJob: () => {} }))
vi.mock('@/server/llm-gateway', () => ({
  gatewayModels: async () => [{ id: 'sonar' }],
  recordGatewayUsage: async () => {},
  completeViaGateway: async () => ({ text: '' }),
  buildUpstream: () => ({}),
  fetchUpstream: async () =>
    new Response(
      JSON.stringify({
        choices: [{ message: { content: 'the vendor published a SOC 2 Type II [1]' } }],
        search_results: [{ url: 'https://example.com/a', title: 'A', snippet: 's' }],
      }),
      { headers: { 'Content-Type': 'application/json' } },
    ),
  resolveRoute: async () => ({ endpoint: 'e', upstreamModel: 'sonar' }),
  contractDropsOf: () => [],
}))
vi.mock('@/server/model-roles', () => ({ resolveRoleModel: async () => 'sonar' }))
// `reachFor` asks the platform what IT can supply, and the real answer probes
// SearXNG over HTTP. This file is about the synthesis stage; it must not depend
// on a container being up, and it must not pay a network round trip to find out.
vi.mock('@/server/capability-platform', () => ({ platformSupply: async () => [], forgetPlatformSupply: () => {}, PLATFORM_SERVER: 'talaria' }))
vi.mock('@/server/notifications', () => ({ addNotification: async () => {} }))
vi.mock('@/server/retrieval/sources', () => ({ indexActivity: async () => {}, indexPersonal: async () => {} }))
vi.mock('@/server/titler', () => ({ generateTitle: async () => 'A title' }))
vi.mock('@/server/kb-perms', () => ({ setEditors: async () => {} }))
vi.mock('@/server/artifacts', () => ({
  agentCategoryFolder: async () => 'folder',
  attachArtifact: async () => {},
  createArtifact: async () => ({ id: 'art-1' }),
  saveArtifact: async (id: string, patch: { body: string }) => {
    saved.push({ id, body: patch.body })
  },
}))

const { startResearch } = await import('@/server/research')

/** Wait for the detached pipeline to reach a TERMINAL write, rather than for a
 *  fixed number of ticks.
 *
 *  It used to be `for (i < 20) await Promise.resolve()`, which is a bet that the
 *  pipeline never grows an await — and it did: the search stage now asks whether
 *  this run can reach search natively or through a tool, so it can pick the
 *  matching transport. The count silently ran out, the assertions saw a
 *  half-finished run, and the failure pointed at the synthesis stage rather than
 *  at the clock. Polling for the outcome cannot go stale that way. */
const TERMINAL = /update research_runs set status = '(?:done|error)', phase = null/
const settle = async () => {
  for (let i = 0; i < 200; i++) {
    // THE PIPELINE'S OWN terminal write, `phase = null` included. Matching a
    // bare `status = 'error'` also matches the STALE SWEEP, which runs on
    // startup and fires before the pipeline has done anything at all — so the
    // wait ended immediately and every assertion read a run that had not
    // started. The test's own lookup below has always been this specific; the
    // wait has to be too.
    if (queries.some((q) => TERMINAL.test(q.text))) break
    await new Promise((r) => setTimeout(r, 0))
  }
  // One more turn so the writes that FOLLOW the terminal one (the artifact save,
  // the notification) have landed before anything is asserted.
  await new Promise((r) => setTimeout(r, 0))
}
const statusWrite = () => queries.find((q) => q.text.includes("update research_runs set status = 'done'"))

beforeEach(() => {
  queries.length = 0
  saved.length = 0
  synthesis = {}
})

describe('the synthesis stage', () => {
  it('marks the run ERRORED, with the gateway’s sentence, when the persona was never reached', async () => {
    synthesis = { value: null, answered: false, raw: null, schemaValid: false, error: 'harness "research-synthesis" could not reach "nomad": persona gateway 502' }
    await startResearch({ question: 'what changed in postgres 17', mode: 'recon', agentModel: 'nomad', ownerUserId: 'user-1', requestedBy: 'user-1' })
    await settle()

    expect(saved).toHaveLength(0)
    // The pipeline's own terminal write (the stale sweep uses a different one).
    const write = queries.find((q) => q.text.includes("update research_runs set status = 'error', phase = null"))
    expect(write).toBeDefined()
    expect(write?.values.some((v) => String(v).includes('persona gateway 502'))).toBe(true)
    // And emphatically NOT the other one: an empty report marked done is
    // unrecoverable — nothing re-runs a run whose status says it finished.
    expect(statusWrite()).toBeUndefined()
  })

  it('still saves and finishes a run whose synthesis came back', async () => {
    await startResearch({ question: 'what changed in postgres 17', mode: 'recon', agentModel: 'nomad', ownerUserId: 'user-1', requestedBy: 'user-1' })
    await settle()
    expect(saved[0]?.body).toContain('a report')
  })
})
