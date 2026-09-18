import { describe, expect, it } from 'vitest'
import { buildPositions, chainProgress, chainedTaskIds, moveStepOrder, type Workchain, type WorkchainStep } from '@/lib/workchain-rules'

// Pure-helper tests: the shapes the lens derives its rendering from. The
// api derives the states (derive_states in api/src/workchains.rs); these
// tests pin what the UI does with what it is handed. The query/mutation
// client (workchain-client.ts) rides the svelte-query runtime and stays out
// of the node suite — the repo's convention for every client module.

const step = (taskId: string, state: WorkchainStep['state']): WorkchainStep => ({
  taskId,
  position: 0,
  state,
  ticketRef: null,
  title: '',
  assignees: [],
  effort: null,
  dueDate: null,
  status: 'inbox',
  archived: state === 'archived',
})

const chain = (...steps: WorkchainStep[]): Pick<Workchain, 'steps'> => ({ steps })

describe('buildPositions', () => {
  it('numbers the given order contiguously from 0', () => {
    expect(buildPositions(['c', 'a', 'b'])).toEqual([
      { taskId: 'c', position: 0 },
      { taskId: 'a', position: 1 },
      { taskId: 'b', position: 2 },
    ])
  })

  it('squeezes gaps out of stale positions', () => {
    // The wire order after a middle removal: 0, 2, 3 — the payload must be
    // 0, 1, 2, or the next reorder starts from the gaps it was fixing.
    expect(buildPositions(['a', 'b', 'c']).map((p) => p.position)).toEqual([0, 1, 2])
  })

  it('empty chain reorders to an empty payload', () => {
    expect(buildPositions([])).toEqual([])
  })
})

describe('chainProgress', () => {
  it('counts done over all steps, head and waiting not done', () => {
    const w = chain(step('a', 'done'), step('b', 'head'), step('c', 'waiting'), step('d', 'done'))
    expect(chainProgress(w)).toBe('2/4')
  })

  it('archived steps count in the total, never as done', () => {
    // Archived is structure the chain reads past — padding the denominator
    // without ever advancing the numerator.
    const w = chain(step('a', 'done'), step('b', 'archived'), step('c', 'head'))
    expect(chainProgress(w)).toBe('1/3')
  })

  it('a chain with no steps reads 0/0, not 0/1 or NaN', () => {
    expect(chainProgress(chain())).toBe('0/0')
  })
})

describe('moveStepOrder', () => {
  it('swaps the task with its neighbor', () => {
    expect(moveStepOrder(['a', 'b', 'c'], 'a', 1)).toEqual(['b', 'a', 'c'])
    expect(moveStepOrder(['a', 'b', 'c'], 'c', -1)).toEqual(['a', 'c', 'b'])
  })

  it('refuses a move off either end', () => {
    expect(moveStepOrder(['a', 'b'], 'a', -1)).toBeNull()
    expect(moveStepOrder(['a', 'b'], 'b', 1)).toBeNull()
  })

  it('refuses an unknown task', () => {
    expect(moveStepOrder(['a', 'b'], 'zz', 1)).toBeNull()
  })

  it('does not mutate the input order', () => {
    const order = ['a', 'b', 'c']
    moveStepOrder(order, 'a', 1)
    expect(order).toEqual(['a', 'b', 'c'])
  })
})

describe('chainedTaskIds', () => {
  it('collects across chains', () => {
    const ids = chainedTaskIds([chain(step('a', 'head'), step('b', 'waiting')), chain(step('c', 'done'))])
    expect(ids.has('a')).toBe(true)
    expect(ids.has('b')).toBe(true)
    expect(ids.has('c')).toBe(true)
    expect(ids.size).toBe(3)
  })

  it('archived steps stay chained — the unique index still holds them', () => {
    const ids = chainedTaskIds([chain(step('a', 'archived'))])
    expect(ids.has('a')).toBe(true)
  })

  it('no chains means no chained ids', () => {
    expect(chainedTaskIds([]).size).toBe(0)
  })
})
