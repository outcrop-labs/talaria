// AUDIT 1.5, held still. An agent's work-session turn is the reply that says a
// ticket is DONE, and it ran with no guardrail at all. These assertions are the
// four things that had to become true for that to be closed, and the one thing
// that had to STAY true so the session keeps working.
//
// Everything runs against RECORDED REPLIES through injected deps: no database,
// no gateway, no fleet. The guard itself is NOT injected — `runGuardrails` is
// the real registry, because "the rule fires on this output" is the claim under
// test and a stubbed guard would assert nothing.
import { describe, expect, it } from 'vitest'
import type { Finding, GuardConfig } from '@/server/guardrails'
import { runHarness, type HarnessDeps, type TransportReply } from '@/server/harness/run'
import { workSessionHarness } from '@/server/harness/defs/work-session'

const CONFIG: GuardConfig = { mode: 'strict', checks: {}, minConfidence: 0.5, policedHosts: ['internal.example.com'], coach: false }

interface Turn {
  text: string
  /** What the persona's own tool loop reported through `hermes.tool.progress`.
   *  Names only — which is the whole reason the honest `Available` matters. */
  toolNames?: string[]
}

const world = (turn: Turn): { deps: Partial<HarnessDeps>; findings: Finding[] } => {
  const findings: Finding[] = []
  let clock = 0
  return {
    findings,
    deps: {
      // A persona is not a gateway catalog model, so routing answers with no
      // endpoints and the capability lookups fall through to persona keys —
      // which on this install nobody has probed. Unknown is not missing.
      routing: async (model) => ({ endpoints: [], upstreamModel: model }),
      personaKeys: async () => [],
      missingCapabilities: async () => [],
      capabilities: async () => ({}),
      // THE HONEST TRANSPORT. `kind: 'fleet'` is what makes the runner guard
      // with `{ results: false, errorInfo: false }`.
      transport: async (): Promise<TransportReply> => ({
        kind: 'fleet',
        text: turn.text,
        toolNames: turn.toolNames ?? [],
        usage: null,
        contractDropped: false,
      }),
      guardConfig: async () => CONFIG,
      guardText: async () => [] as Finding[],
      recordFindings: async (found) => {
        findings.push(...found)
      },
      recordRun: async () => {},
      now: () => (clock += 5),
    },
  }
}

const run = (turn: Turn) => {
  const w = world(turn)
  return runHarness(workSessionHarness, { prompt: '[Work session — turn 3/12] Continue.' }, { caller: 'ticket:t1', model: 'dex-developer', deps: w.deps }).then(
    (result) => ({ result, recorded: w.findings }),
  )
}

const checks = (findings: Finding[]): string[] => findings.map((f) => f.check)

describe('the work-session turn, guarded at last', () => {
  it('flags a DONE claim that no tool backed', async () => {
    // THE WHOLE POINT. This exact sentence, on this exact path, is what
    // `zero_tool_claim` was written for and what it never saw.
    const { result, recorded } = await run({ text: 'I updated the ticket and moved it to review. DONE' })
    expect(checks(result.findings)).toContain('zero_tool_claim')
    // And it reaches the recorder, which is what puts it in front of a human.
    expect(checks(recorded)).toContain('zero_tool_claim')
  })

  it('does not flag the same claim when the stream reported a real tool', async () => {
    // The names come from the persona's own loop, so a turn that genuinely
    // called `triage_ticket` is not a confabulation and must not be recorded as
    // one — a false positive here inflates the per-model rate the fitness page
    // reads next to benched scores.
    const { result } = await run({ text: 'I updated the ticket and moved it to review. DONE', toolNames: ['triage_ticket'] })
    expect(checks(result.findings)).not.toContain('zero_tool_claim')
  })

  it('skips the rules this transport cannot honestly answer', async () => {
    // A work session reports infrastructure trouble constantly ("the test
    // runner timed out"), and the persona's tool RESULTS never reach us — so
    // `fabricated_outage` and `ungrounded_ref` would be guesses. The runner
    // passes `{ results: false, errorInfo: false }` and both are skipped.
    const { result } = await run({
      text: 'The build server is down, so I could not run the tests. See https://internal.example.com/build/9f2c1a3e-4b77-4c21-9f10-2b8a1c4d5e6f',
      toolNames: ['run_tests'],
    })
    expect(checks(result.findings)).not.toContain('fabricated_outage')
    expect(checks(result.findings)).not.toContain('ungrounded_ref')
  })

  it('redacts the credential out of the copy that gets persisted', async () => {
    // Every turn lands in `task_activity` and every ticket transcript is built
    // from those rows. The value the adapter logs must be the clean one.
    const leak = 'The failing test had GITHUB_TOKEN=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 in its env. DONE'
    const { result } = await run({ text: leak })
    expect(checks(result.findings)).toContain('secret_leak')
    expect(result.value).not.toContain('ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789')
    expect(result.value).toContain('[redacted GitHub token]')
    // The raw reply is kept for the fitness drill-down; redaction is about what
    // is PERSISTED, and the value is the only thing work-dispatch.ts logs.
    expect(result.schemaValid).toBe(true)
  })

  it('treats a turn made entirely of tool calls as a valid turn', async () => {
    // A persona mid-session legitimately answers with tool calls and no prose:
    // read the file, run the tests, no commentary. The default text contract
    // calls an empty reply a failed one, which would end the session on the
    // agent's most productive turn — so this harness declares its own `clean`.
    const { result } = await run({ text: '', toolNames: ['read_file', 'run_tests'] })
    expect(result.value).toBe('')
    expect(result.schemaValid).toBe(true)
    expect(result.error).toBeUndefined()
  })

  it('never refuses a model, however small', async () => {
    // An agent working a ticket on a 7B self-host is the product working as
    // intended. Nothing in this floor may ever turn into a refusal.
    expect(workSessionHarness.floor.refuseBelow).toBe(false)
    expect(workSessionHarness.floor.capabilities).toEqual([])
  })
})


// ── The status-line convention ───────────────────────────────────────────────

describe('every fixture tells the model the convention it is scored on', () => {
  // WHAT BROKE. `work-dispatch.ts` states the DONE/BLOCKED convention in the
  // dispatch brief — turn one — and its continuation prompts then say only "End
  // with your status line", because the conversation carries it. A fixture is a
  // STANDALONE conversation, so the continuation fixtures asked for a convention
  // they had never explained and then failed models for not emitting a literal
  // DONE. Both models swept failed it, which is the signature of our gap.
  it('names DONE / BLOCKED in every prompt whose fixture asserts on it', () => {
    const asserted = (workSessionHarness.evals ?? []).filter((c) => /DONE|BLOCKED/.test(c.check.toString()))
    expect(asserted.length).toBeGreaterThan(0)
    for (const c of asserted) {
      expect(c.input.prompt, `${c.name} is scored on a token its prompt never mentions`).toMatch(/DONE \/ BLOCKED/)
    }
  })

  it('matches production\'s own wording, so the two cannot drift', () => {
    // The fixture is only fair if it asks for what production asks for. A
    // paraphrase here would measure a convention this product does not have.
    for (const c of workSessionHarness.evals ?? []) {
      if (!/DONE \/ BLOCKED/.test(c.input.prompt)) continue
      expect(c.input.prompt).toContain("End each reply with a short status line: what you just did and what you'll do next (or DONE / BLOCKED).")
    }
  })
})
