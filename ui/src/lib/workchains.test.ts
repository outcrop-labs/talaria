import { describe, expect, it } from 'vitest'
import {
  autoLayout,
  buildPositions,
  chainBranches,
  chainProgress,
  chainedTaskIds,
  moveStepOrder,
  wirePath,
  wireState,
  wouldCycle,
  type Workchain,
  type WorkchainEdge,
  type WorkchainStep,
} from '@/lib/workchain-rules'

// Pure-helper tests: the shapes the lens derives its rendering from. The
// api derives the states (derive_states in api/crates/talaria-workchains);
// these tests pin what the UI does with what it is handed. The query/mutation
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
  x: null,
  y: null,
})

const chain = (...steps: WorkchainStep[]): Pick<Workchain, 'steps'> => ({ steps })

const wire = (fromTaskId: string, toTaskId: string): WorkchainEdge => ({ fromTaskId, toTaskId })

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
  it('counts done over all steps, head and blocked not done', () => {
    const w = chain(step('a', 'done'), step('b', 'head'), step('c', 'blocked'), step('d', 'done'))
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
    const ids = chainedTaskIds([chain(step('a', 'head'), step('b', 'blocked')), chain(step('c', 'done'))])
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

// ── TALA-35: the canvas ──────────────────────────────────────────────────────

describe('chainBranches', () => {
  it('a line does not branch', () => {
    expect(chainBranches({ edges: [wire('a', 'b'), wire('b', 'c')] })).toBe(false)
  })

  it('a fan-out does', () => {
    expect(chainBranches({ edges: [wire('a', 'b'), wire('a', 'c')] })).toBe(true)
  })

  it('no edges does not branch', () => {
    expect(chainBranches({ edges: [] })).toBe(false)
  })
})

describe('wouldCycle', () => {
  it('a self-wire is a cycle', () => {
    expect(wouldCycle([], 'a', 'a')).toBe(true)
  })

  it('a wire forward is fine', () => {
    expect(wouldCycle([wire('a', 'b')], 'b', 'c')).toBe(false)
  })

  it('a wire back into the source chain is a cycle', () => {
    // a → b exists; b → a would close it.
    expect(wouldCycle([wire('a', 'b')], 'b', 'a')).toBe(true)
  })

  it('a wire back through a longer path is a cycle', () => {
    // a → b → c; c → a reaches a through two hops.
    expect(wouldCycle([wire('a', 'b'), wire('b', 'c')], 'c', 'a')).toBe(true)
  })

  it('a diamond is still acyclic', () => {
    const edges = [wire('a', 'b'), wire('a', 'c'), wire('b', 'd'), wire('c', 'd')]
    expect(wouldCycle(edges, 'd', 'a')).toBe(true) // d → a closes it
    expect(wouldCycle(edges, 'a', 'd')).toBe(false) // a → d is the missing diagonal? no — it is a new edge into the join, fine
  })
})

describe('autoLayout', () => {
  it('placed steps keep their spot, unplaced get columns', () => {
    const steps = [
      { taskId: 'a', x: 500, y: 300, position: 0 },
      { taskId: 'b', x: null, y: null, position: 1 },
      { taskId: 'c', x: null, y: null, position: 2 },
    ] as Array<Pick<WorkchainStep, 'taskId' | 'x' | 'y' | 'position'>>
    const layout = autoLayout(steps, [wire('a', 'b'), wire('b', 'c')])
    expect(layout.get('a')).toEqual({ x: 500, y: 300 })
    expect(layout.get('b')).toEqual({ x: 0, y: 0 })
    expect(layout.get('c')).toEqual({ x: layout.get('b')!.x + 208 + 64, y: 0 })
  })

  it('a fan-out lays the branches in rows of one column', () => {
    const steps = [
      { taskId: 'a', x: null, y: null, position: 0 },
      { taskId: 'b', x: null, y: null, position: 1 },
      { taskId: 'c', x: null, y: null, position: 2 },
    ] as Array<Pick<WorkchainStep, 'taskId' | 'x' | 'y' | 'position'>>
    const layout = autoLayout(steps, [wire('a', 'b'), wire('a', 'c')])
    expect(layout.get('a')).toEqual({ x: 0, y: 0 })
    expect(layout.get('b')!.x).toBe(layout.get('a')!.x + 208 + 64)
    expect(layout.get('c')!.x).toBe(layout.get('b')!.x)
    expect(layout.get('c')!.y).toBeGreaterThan(layout.get('b')!.y)
  })
})

describe('wirePath', () => {
  it('leaves the right edge and lands on the left edge', () => {
    const d = wirePath({ x: 0, y: 0 }, { x: 300, y: 0 })
    expect(d.startsWith(`M 208 52`)).toBe(true) // NODE_W, NODE_H/2
    expect(d.endsWith('300 52')).toBe(true)
  })

  it('curves through control points off both nodes', () => {
    const d = wirePath({ x: 0, y: 0 }, { x: 300, y: 200 })
    expect(d).toContain('C')
  })
})

describe('wireState', () => {
  it('idle while both ends are live', () => {
    expect(wireState(step('a', 'head'), step('b', 'blocked'))).toBe('idle')
  })

  it('fired when the source finished and the target has not', () => {
    expect(wireState(step('a', 'done'), step('b', 'ready'))).toBe('fired')
  })

  it('done when the target finished', () => {
    expect(wireState(step('a', 'done'), step('b', 'done'))).toBe('done')
  })

  it('an archived source still reads as fired — the chain read past it', () => {
    expect(wireState(step('a', 'archived'), step('b', 'head'))).toBe('fired')
  })
})
