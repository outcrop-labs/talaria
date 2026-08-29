import { describe, expect, it } from 'vitest'
import { makeAgentHireRun, type AgentHireCheckpoint, type AgentHireDeps, type AgentHireInput } from './agent-hire'
import type { RunRow, RunStepContext } from '../define'

// The definition's contract, driven with fake deps the way plan-draft.test.ts
// drives its — no database, no docker, no clock. What is under test is the
// stage machine: the checkpoint is the only state, each stage is entered from
// the checkpoint the previous one wrote, and a hire that did not ask to start
// never touches the container.
const ctx = (input: AgentHireInput, checkpoint: AgentHireCheckpoint | null): RunStepContext<AgentHireInput, AgentHireCheckpoint> =>
  ({
    run: { id: 'hire-1', kind: 'agent-hire', ownerUserId: null, state: 'running', phase: '' } as RunRow,
    input,
    checkpoint,
    decision: null,
    signal: new AbortController().signal,
    log: () => {},
    attempt: 0,
  }) as unknown as RunStepContext<AgentHireInput, AgentHireCheckpoint>

const INPUT: AgentHireInput = {
  slug: 'sloane',
  department: 'research',
  displayName: 'Sloane',
  role: 'Research Analyst',
  templateId: null,
  soul: '# Sloane',
  skills: [{ name: 'weekly-sweep', content: 'the playbook' }],
  start: true,
  actor: 'jon@example.com',
}

const DEF = { id: 'def-1', slug: 'sloane', department: 'research', displayName: 'Sloane' }

/** Deps that record every call, so the stage boundaries are observable. */
const recording = () => {
  const calls: string[] = []
  const deps: AgentHireDeps = {
    create: async () => {
      calls.push('create')
      return { def: DEF as never, keyCreated: true }
    },
    writeSkills: async () => {
      calls.push('skills')
    },
    audit: () => {
      calls.push('audit')
    },
    render: async () => {
      calls.push('render')
      return { warnings: ['one warning'] }
    },
    up: async () => {
      calls.push('up')
    },
    waitHealthy: async () => {
      calls.push('healthy')
      return true
    },
  }
  return { calls, deps }
}

describe('agent-hire run', () => {
  it('walks create → render → boot, each stage entered from the last checkpoint', async () => {
    const { calls, deps } = recording()
    const run = makeAgentHireRun(deps)

    const a = await run.step(ctx(INPUT, null))
    expect(a).toEqual({ kind: 'next', checkpoint: { defId: 'def-1', stage: 'render', warnings: [] }, phase: 'rendering the fleet config' })
    expect(calls).toEqual(['create', 'skills', 'audit'])

    const b = await run.step(ctx(INPUT, (a as { checkpoint: AgentHireCheckpoint }).checkpoint))
    expect(b).toEqual({ kind: 'next', checkpoint: { defId: 'def-1', stage: 'boot', warnings: ['one warning'] }, phase: 'starting the container' })
    expect(calls).toEqual(['create', 'skills', 'audit', 'render'])

    const c = await run.step(ctx(INPUT, (b as { checkpoint: AgentHireCheckpoint }).checkpoint))
    expect(c).toEqual({ kind: 'done', result: { defId: 'def-1', healthy: true, warnings: ['one warning'] } })
    expect(calls).toEqual(['create', 'skills', 'audit', 'render', 'up', 'healthy'])
  })

  it('never touches the container when start was not asked for', async () => {
    const { calls, deps } = recording()
    const run = makeAgentHireRun(deps)

    const a = await run.step(ctx({ ...INPUT, start: false }, null))
    const b = await run.step(ctx({ ...INPUT, start: false }, (a as { checkpoint: AgentHireCheckpoint }).checkpoint))
    expect(b).toEqual({ kind: 'done', result: { defId: 'def-1', healthy: undefined, warnings: ['one warning'] } })
    expect(calls).toEqual(['create', 'skills', 'audit', 'render'])
  })

  it('files not-healthy as a WARNING in the result, not a failed run', async () => {
    const { deps } = recording()
    deps.waitHealthy = async () => false
    const run = makeAgentHireRun(deps)
    const a = await run.step(ctx(INPUT, null))
    const b = await run.step(ctx(INPUT, (a as { checkpoint: AgentHireCheckpoint }).checkpoint))
    const c = await run.step(ctx(INPUT, (b as { checkpoint: AgentHireCheckpoint }).checkpoint))
    expect((c as { kind: string }).kind).toBe('done')
    expect((c as { result: { healthy: boolean } }).result.healthy).toBe(false)
  })

  it('resumes from a mid-boot crash without re-running create', async () => {
    // A driver that died after render re-enters at boot: the def, the skills
    // and the audit are the previous driver's completed work, not a rerun.
    const { calls, deps } = recording()
    const run = makeAgentHireRun(deps)
    const c = await run.step(ctx(INPUT, { defId: 'def-1', stage: 'boot', warnings: [] }))
    expect((c as { kind: string }).kind).toBe('done')
    expect(calls).toEqual(['up', 'healthy'])
  })
})
