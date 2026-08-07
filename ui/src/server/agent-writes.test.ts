import { describe, expect, it, vi } from 'vitest'
import { guardAgentWrite, type AgentWriteDeps } from '@/server/agent-writes'
import { RULES, runGuardrails, type Finding, type GuardConfig, type GuardMode } from '@/server/guardrails'

// The door itself, with every edge injected — no database, no app_settings read.
// The rules underneath are the REAL ones (`guardText`'s gate-safe subset, run
// here through the same pure `runGuardrails` it uses), because a fake detector
// would prove only that this file can call a function it wrote.
//
// What each case is actually protecting is stated on the case. The one-line
// version: agent-authored MCP text is scanned before it is persisted, a person's
// typing is not, and nothing this door returns can carry a flagged snippet back
// into a model.

// The credential an agent is most certain to have in its own environment
// (agent-auth.ts mints it), which is what makes it the honest fixture here.
const CRED = `tak_${'a'.repeat(40)}`

const config = (mode: GuardMode): GuardConfig => ({ mode, checks: {}, minConfidence: 0.5, policedHosts: [], coach: false })

/** `guardText`, minus its settings read: the gate-safe rules over plain text,
 *  empty when the guard is off. Mirrors guardrails.ts's own implementation. */
const gateSafe = (mode: GuardMode) => async (text: string): Promise<Finding[]> => {
  if (mode === 'off') return []
  const checks = Object.fromEntries(RULES.map((r) => [r.id, r.gateSafe === true]))
  return runGuardrails(
    { answer: text, toolRecord: { backingTools: [], resultsText: '', anyError: false, overflowed: false }, userMessage: '', policedHosts: [] },
    config(mode),
    { results: false, errorInfo: false },
  ).filter((f) => checks[f.check])
}

interface Recorded {
  findings: Finding[]
  meta: { caller: string; model: string; endpoint: string | null; mode: GuardMode }
}

const harness = (mode: GuardMode, over: Partial<AgentWriteDeps> = {}) => {
  const recorded: Recorded[] = []
  const isAgent = vi.fn(async (name: string) => name === 'nomad')
  const deps: Partial<AgentWriteDeps> = {
    isAgent,
    guardText: gateSafe(mode),
    guardConfig: async () => config(mode),
    recordFindings: async (findings, meta) => {
      recorded.push({ findings, meta: meta as Recorded['meta'] })
    },
    ...over,
  }
  return { deps, recorded, isAgent }
}

describe('guardAgentWrite', () => {
  it('flags a credential in a DM and redacts the copy that gets persisted, in strict mode', async () => {
    // The bug in one assertion: the outreach check-in's REPORT line was guarded
    // and redacted by the runner, and the DM sent during that same turn was not.
    const { deps, recorded } = harness('strict')
    const out = await guardAgentWrite('direct-message', { agent: 'nomad' }, `here is the key you asked for: ${CRED}`, deps)

    expect(out.text).not.toContain(CRED)
    expect(out.text).toContain('[redacted Talaria agent credential]')
    expect(out.redacted).toBe(true)
    expect(out.findings.map((f) => f.check)).toEqual(['secret_leak'])
    expect(recorded).toHaveLength(1)
  })

  it('names the agent on the finding, so guard_findings.model stays a per-model rate', async () => {
    // The fitness page reads `model` as "this model's confabulation rate" and
    // `caller` as "which door". Both have to be true of a tool argument too,
    // or the rate silently measures only the harness half of an agent's output.
    const { deps, recorded } = harness('strict')
    await guardAgentWrite('direct-message', { agent: 'nomad' }, `key: ${CRED}`, deps)

    expect(recorded[0]?.meta).toEqual({ caller: 'direct-message:nomad', model: 'nomad', endpoint: 'fleet', mode: 'strict' })
  })

  it('carries the write path in the caller, per surface', async () => {
    const { deps, recorded } = harness('strict')
    await guardAgentWrite('ticket-comment', { agent: 'nomad' }, `key: ${CRED}`, deps)
    await guardAgentWrite('channel-post', { agent: 'nomad' }, `key: ${CRED}`, deps)

    expect(recorded.map((r) => r.meta.caller)).toEqual(['ticket-comment:nomad', 'channel-post:nomad'])
  })

  it('leaves a clean message completely untouched', async () => {
    // A guard that rewrites ordinary sentences is a guard an operator turns off.
    const { deps, recorded, isAgent } = harness('strict')
    const text = 'ALPH-14 is blocked on the staging credentials — can you take a look before standup?'
    const out = await guardAgentWrite('direct-message', { agent: 'nomad' }, text, deps)

    expect(out.text).toBe(text)
    expect(out.redacted).toBe(false)
    expect(out.findings).toEqual([])
    expect(recorded).toEqual([])
    expect(isAgent).not.toHaveBeenCalled() // the caller already proved the author
  })

  it('records without altering in observe mode', async () => {
    // Observe's whole promise: detect and record out-of-band, never touch what
    // the human receives. Strict is the mode that scrubs.
    const { deps, recorded } = harness('observe')
    const text = `key: ${CRED}`
    const out = await guardAgentWrite('direct-message', { agent: 'nomad' }, text, deps)

    expect(out.text).toBe(text)
    expect(out.redacted).toBe(false)
    expect(recorded[0]?.meta.mode).toBe('observe')
    expect(recorded[0]?.findings.map((f) => f.check)).toEqual(['secret_leak'])
  })

  it('annotates without altering: findings for the caller to pin, content unchanged', async () => {
    const { deps } = harness('annotate')
    const text = `key: ${CRED}`
    const out = await guardAgentWrite('direct-message', { agent: 'nomad' }, text, deps)

    expect(out.text).toBe(text)
    expect(out.mode).toBe('annotate')
    expect(out.findings).toHaveLength(1)
  })

  it('does nothing at all when the guard is off', async () => {
    const { deps, recorded } = harness('off')
    const text = `key: ${CRED}`
    const out = await guardAgentWrite('channel-post', { agent: 'nomad' }, text, deps)

    expect(out.text).toBe(text)
    expect(out.findings).toEqual([])
    expect(recorded).toEqual([])
  })

  it('flags PII as well as credentials, and redacts both', async () => {
    const { deps } = harness('strict')
    const out = await guardAgentWrite('ticket-comment', { agent: 'nomad' }, 'the applicant SSN is 123-45-6789', deps)

    expect(out.text).toContain('[redacted SSN]')
    expect(out.findings.map((f) => f.check)).toEqual(['pii_leak'])
  })

  it('does not run a rule that needs a tool record it cannot have', async () => {
    // An MCP comment saying the PR is open is a claim whose backing tool ran in
    // another process. `zero_tool_claim` is not gate-safe for exactly that
    // reason, and flagging honest work here would train operators to ignore the
    // guard.
    const { deps, recorded } = harness('strict')
    const out = await guardAgentWrite('ticket-comment', { agent: 'nomad' }, 'I created the ticket and posted the summary.', deps)

    expect(out.findings).toEqual([])
    expect(recorded).toEqual([])
  })

  // ── Who counts as an agent ────────────────────────────────────────────────

  it('leaves a human author alone — a person typing is not model output', async () => {
    const { deps, recorded } = harness('strict')
    const text = `my card is 4111 1111 1111 1111`
    const out = await guardAgentWrite('ticket-comment', 'priya@example.com', text, deps)

    expect(out.text).toBe(text)
    expect(out.agent).toBeNull()
    expect(recorded).toEqual([])
  })

  it('verifies a bare author string against the fleet, so the human door cannot launder an agent write', async () => {
    // `logTicket`'s `agentBehind` rule, and the reason `addComment` can take an
    // author of either kind and still be one door.
    const { deps, recorded } = harness('strict')
    const out = await guardAgentWrite('ticket-comment', 'nomad', `key: ${CRED}`, deps)

    expect(out.agent).toBe('nomad')
    expect(out.text).not.toContain(CRED)
    expect(recorded[0]?.meta.model).toBe('nomad')
  })

  it('skips empty text without touching the database', async () => {
    const { deps, isAgent } = harness('strict')
    const out = await guardAgentWrite('channel-post', '   ', '', deps)

    expect(out.text).toBe('')
    expect(isAgent).not.toHaveBeenCalled()
  })

  // ── Failing open ──────────────────────────────────────────────────────────

  it('keeps the observe posture when the mode read fails, rather than guessing strict', async () => {
    // A rule fired, so the guard is on — but which mode is unknown. Silently
    // rewriting a teammate's message under a policy nobody set is worse than
    // recording it and leaving it alone.
    const { deps, recorded } = harness('strict', {
      guardConfig: async () => {
        throw new Error('app_settings unreachable')
      },
    })
    const text = `key: ${CRED}`
    const out = await guardAgentWrite('direct-message', { agent: 'nomad' }, text, deps)

    expect(out.text).toBe(text)
    expect(out.mode).toBe('observe')
    expect(recorded[0]?.meta.mode).toBe('observe')
  })

  it('never throws: a guard that fails closed would take commenting down with it', async () => {
    const { deps } = harness('strict', {
      guardText: async () => {
        throw new Error('boom')
      },
      recordFindings: async () => {
        throw new Error('boom')
      },
      isAgent: async () => {
        throw new Error('boom')
      },
    })
    await expect(guardAgentWrite('ticket-comment', 'nomad', 'anything', deps)).resolves.toMatchObject({ text: 'anything' })
  })
})
