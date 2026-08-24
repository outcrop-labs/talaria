import { describe, expect, it } from 'vitest'
import { makePlanDraftRun, type PlanDraftDeps, type PlanDraftInput } from './plan-draft'
import type { RunRow, RunStepContext } from '../define'

// The definition's contract, driven with fake deps the way run.test.ts drives
// the runner — no database, no model, no clock. What is under test is the
// part with intent: the empty-batch notes, the normalization the row depends
// on, and the fact that a planner failure is allowed to THROW (the driver
// files it as the run's error; swallowing it here would hide it forever).
const ctx = (input: PlanDraftInput): RunStepContext<PlanDraftInput, null> =>
  ({
    run: { id: 'draft-1', kind: 'plan-draft', ownerUserId: null, state: 'running', phase: '' } as RunRow,
    input,
    checkpoint: null,
    decision: null,
    signal: new AbortController().signal,
    log: () => {},
    attempt: 0,
  }) as unknown as RunStepContext<PlanDraftInput, null>

const INPUT: PlanDraftInput = {
  conversationId: 'c1',
  source: 'plan',
  agentModel: 'atlas',
  routedModel: 'atlas',
  boardId: null,
  templateId: null,
}

describe('plan-draft run', () => {
  it('normalizes proposals into the reviewed shape and files done', async () => {
    const saved: Array<{ proposals: unknown; note: string | null }> = []
    const deps: PlanDraftDeps = {
      // Model output as JSON: arrays missing despite the type's promise.
      draftTickets: async () => ({
        proposals: [{ title: 'A', description: 'do a', priority: 'high', effort: null }] as never,
        raw: 'text',
      }),
      saveResult: async (_id, out) => {
        saved.push({ proposals: out.proposals, note: out.note })
      },
    }
    const res = await makePlanDraftRun(deps).step(ctx(INPUT))
    expect(res).toEqual({ kind: 'done', result: { count: 1 } })
    expect(saved).toEqual([
      {
        proposals: [{ title: 'A', description: 'do a', priority: 'high', effort: null, dependsOn: [], tags: [], include: true }],
        note: null,
      },
    ])
  })

  it("keeps the synchronous route's distinction: answered-but-unparseable vs nothing to plan", async () => {
    const notes: Array<string | null> = []
    const deps = (raw: string): PlanDraftDeps => ({
      draftTickets: async () => ({ proposals: [], raw }),
      saveResult: async (_id, out) => {
        notes.push(out.note)
      },
    })
    await makePlanDraftRun(deps('the agent replied, just not in tickets')).step(ctx(INPUT))
    await makePlanDraftRun(deps('')).step(ctx(INPUT))
    expect(notes).toEqual(['the agent did not return parseable tickets', 'nothing to plan yet'])
  })

  it('lets a planner failure throw — the driver files the error row', async () => {
    const deps: PlanDraftDeps = {
      draftTickets: async () => {
        throw new Error('gateway unreachable')
      },
      saveResult: async () => {
        throw new Error('saveResult must not run after a planner failure')
      },
    }
    await expect(makePlanDraftRun(deps).step(ctx(INPUT))).rejects.toThrow('gateway unreachable')
  })

  it('scopes the audience to the drafting user', () => {
    const run = makePlanDraftRun({ draftTickets: async () => ({ proposals: [], raw: '' }), saveResult: async () => {} })
    const owned = { ownerUserId: 'u1' } as RunRow
    expect(run.audience(owned)).toEqual({ by: 'user', userIds: ['u1'] })
    expect(run.audience({ ownerUserId: null } as RunRow)).toEqual({ by: 'admin' })
    expect(run.kind).toBe('plan-draft')
  })
})
