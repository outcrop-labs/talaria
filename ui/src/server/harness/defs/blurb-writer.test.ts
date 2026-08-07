// The catalog writer's one unenforceable contract: the KEYS.
//
// `SCHEMA` is a module constant and the batch's ids are runtime input, so
// `z.record(z.string(), z.string())` cannot say "these three ids, spelled
// exactly as written" — the prompt says it in words and, until `output.verify`,
// nothing checked it. A model that tidies `qwen3-14b` into `Qwen3 14B` therefore
// produced a reply that PASSED the schema, wrote zero rows, came back around
// every ten minutes on the identical batch forever, and was recorded as a
// perfect contract. That is audit 1.1's exact symptom with green telemetry over
// it.
//
// The caller used to paper over it with a normalizing re-key pass
// (`blurbsByCandidateId`), which rescued the batch and hid the fact that the
// model had not answered the question — no repair, no record, no fitness signal.
// That pass is gone; these cases are what replaced it.
import { describe, expect, it } from 'vitest'
import { NO_TOOLS } from '@/server/harness/define'
import { blurbWriterHarness, type BlurbBatch, type BlurbMap } from '@/server/harness/defs/blurb-writer'
import { runHarness, type HarnessDeps, type HarnessRunRow, type TransportRequest } from '@/server/harness/run'
import type { GuardConfig } from '@/server/guardrails'

const GUARD: GuardConfig = { mode: 'observe', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }

function world(replies: string[]) {
  const requests: TransportRequest[] = []
  const runs: HarnessRunRow[] = []
  const deps: Partial<HarnessDeps> = {
    resolveModel: async () => ({ model: 'pl-main', step: 'pin' }),
    routing: async (model) => ({ endpoints: ['spark'], upstreamModel: model }),
    missingCapabilities: async () => [],
    capabilities: async () => ({}),
    transport: async (req) => {
      requests.push(req)
      return { kind: 'gateway', text: replies[Math.min(requests.length - 1, replies.length - 1)] ?? '', toolNames: [], usage: null, contractDropped: false }
    },
    guardConfig: async () => GUARD,
    guardText: async () => [],
    recordFindings: async () => {},
    recordRun: async (row) => {
      runs.push(row)
    },
    now: () => 0,
  }
  return { requests, runs, deps }
}

const BATCH: BlurbBatch = {
  orgName: 'Outcrop Labs',
  models: [
    { id: 'qwen3-14b', name: 'Qwen: Qwen3 14B', description: 'A general-purpose model with strong reasoning for its size.' },
    { id: 'llama-3.1-8b', name: 'Meta: Llama 3.1 8B Instruct', description: 'A small, fast instruction-tuned model.' },
  ],
}

/** What the prompt asks for. */
const CORRECT = JSON.stringify({ 'qwen3-14b': 'A capable all-rounder for everyday work.', 'llama-3.1-8b': 'Fast and cheap for short tasks.' })

/** THE REAL-WORLD WRONG SHAPE. The prompt hands the model the display names
 *  right next to the ids, and a helpful model keys by the pretty one. */
const TIDIED = JSON.stringify({ 'Qwen3 14B': 'A capable all-rounder for everyday work.', 'Llama 3.1 8B Instruct': 'Fast and cheap for short tasks.' })

const run = (batch: BlurbBatch, w: ReturnType<typeof world>) => runHarness(blurbWriterHarness, batch, { caller: 'platform:blurb-writer', deps: w.deps })

describe('the key contract', () => {
  it('does not fire on a batch keyed the way it was asked for', async () => {
    const w = world([CORRECT])
    const res = await run(BATCH, w)
    expect(res.value).toEqual({ 'qwen3-14b': 'A capable all-rounder for everyday work.', 'llama-3.1-8b': 'Fast and cheap for short tasks.' })
    expect(res.schemaValid).toBe(true)
    expect(res.repairs).toBe(0)
  })

  it('accepts a PARTIAL batch, because six good lines are six good lines', async () => {
    // The models this sweep skipped keep their catalog line and come back around
    // in ten minutes. Demanding all of them would throw the rest away.
    const w = world([JSON.stringify({ 'qwen3-14b': 'A capable all-rounder.' })])
    const res = await run(BATCH, w)
    expect(res.value).toEqual({ 'qwen3-14b': 'A capable all-rounder.' })
    expect(res.schemaValid).toBe(true)
  })

  it('fails a reply the SCHEMA accepted, then repairs it on the second attempt', async () => {
    const w = world([TIDIED, CORRECT])
    const res = await run(BATCH, w)

    expect(res.repairs).toBe(1)
    expect(res.schemaValid).toBe(true)
    expect(res.value).toEqual({ 'qwen3-14b': 'A capable all-rounder for everyday work.', 'llama-3.1-8b': 'Fast and cheap for short tasks.' })
    // The sentence a 7-14B model can act on: it quotes the key it used and
    // re-states the ids. "keys must be a subset of the requested ids" would not.
    const repair = w.requests[1]?.messages.at(-1)?.content ?? ''
    expect(repair).toContain('the keys must be the model ids exactly as they were given')
    expect(repair).toContain('"Qwen3 14B"')
    expect(repair).toContain('qwen3-14b, llama-3.1-8b')
  })

  it('is an honest contract failure on the row when the repair does not take', async () => {
    // THE bug, stated as telemetry: before this, a batch that wrote nothing was
    // recorded as a 100% contract and re-burned every ten minutes forever.
    const w = world([TIDIED, TIDIED])
    const res = await run(BATCH, w)

    expect(res.value).toBeNull()
    expect(res.schemaValid).toBe(false)
    expect(res.answered).toBe(true) // it spoke, twice; that is a different fact
    expect(w.runs[0]?.schemaValid).toBe(false)
    expect(w.runs[0]?.error).toContain('the keys must be the model ids')
  })

  it('fails an empty object, which is the sweep achieving nothing', async () => {
    // Unlike the ticket drafter, there is no honest "nothing to write here":
    // every id in the batch is a registered model with a catalog line attached.
    const w = world(['{}', '{}'])
    const res = await run(BATCH, w)
    expect(res.value).toBeNull()
    expect(w.requests[1]?.messages.at(-1)?.content).toContain('you returned an empty object')
    expect(w.runs[0]?.error).toContain('qwen3-14b, llama-3.1-8b')
  })

  it('rejects an id the model merely re-punctuated or re-cased', async () => {
    // The old caller-side pass normalized these into a match, silently. They are
    // the same mistake as a display name and they get the same repair turn.
    const w = world([JSON.stringify({ 'Qwen3-14B': 'x', 'llama_3_1_8b': 'y' })])
    const res = await run(BATCH, w)
    expect(res.schemaValid).toBe(false)
    expect(res.error).toContain('"Qwen3-14B"')
  })
})

describe('the eval fixtures', () => {
  const check = (i: number, v: BlurbMap): string | null => {
    const fixture = blurbWriterHarness.evals?.[i]
    if (!fixture) throw new Error(`no eval fixture at index ${i}`)
    return fixture.check(v, NO_TOOLS)
  }

  it('assert the key rule through the same function the harness enforces', () => {
    // The two used to be separate spellings, and that is exactly how the offline
    // suite and `harness_runs.schema_valid` came to disagree about one reply.
    expect(check(0, { 'pl-main': 'a', 'pl-fast': 'b', 'pl-code': 'c' })).toBeNull()
    expect(check(0, { 'PL Main': 'a' })).toContain('the keys must be the model ids exactly as they were given')
    expect(check(2, { 'llama-3.1-8b': 'a', 'mixtral-8x7b-v0.1': 'b' })).toBeNull()
  })

  it('MEASURE what the contract deliberately tolerates', () => {
    // Length and per-line emptiness are scored, not enforced: failing a batch of
    // ten over one long sentence would cost the other nine, while scoring it
    // tells an admin something true about the model.
    expect(check(0, { 'pl-main': 'a', 'pl-fast': 'b', 'pl-code': 'x'.repeat(240) })).toContain('240 chars')
    expect(check(0, { 'pl-main': '   ' })).toContain("the description for 'pl-main' is empty")
  })
})
