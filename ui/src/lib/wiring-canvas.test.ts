import { describe, expect, it } from 'vitest'
import {
  edgeCandidateSteps,
  edgeDropOutcome,
  findEdge,
  WIRE_HIT_RADIUS,
  type WiringStepLike,
} from '@/lib/wiring-canvas'
import type { WorkchainEdge } from '@/lib/workchain-rules'

// The wiring editor's graph decisions (TALA-34) — the drop-resolution state
// machine and the edge lookup, pinned as pure functions.

const step = (taskId: string, state: WiringStepLike['state']): WiringStepLike => ({
  taskId,
  state,
})

const edges = (...pairs: Array<[string, string]>): WorkchainEdge[] =>
  pairs.map(([fromTaskId, toTaskId]) => ({ fromTaskId, toTaskId }))

describe('findEdge', () => {
  it('names the exact edge', () => {
    const list = edges(['a', 'b'], ['b', 'c'])
    expect(findEdge(list, 'a', 'b')).toEqual({ fromTaskId: 'a', toTaskId: 'b' })
    expect(findEdge(list, 'b', 'a')).toBeUndefined()
    expect(findEdge([], 'a', 'b')).toBeUndefined()
  })
})

describe('edgeCandidateSteps', () => {
  it('excludes the source and anything already wired from it', () => {
    const steps = [step('a', 'head'), step('b', 'blocked'), step('c', 'blocked')]
    const list = edges(['a', 'b'])
    expect(edgeCandidateSteps(steps, list, 'a').map((s) => s.taskId)).toEqual(['c'])
  })

  it('a fan-out still leaves the unwired steps as targets', () => {
    const steps = [step('a', 'done'), step('b', 'ready'), step('c', 'ready'), step('d', 'blocked')]
    const list = edges(['a', 'b'], ['a', 'c'])
    expect(edgeCandidateSteps(steps, list, 'a').map((s) => s.taskId)).toEqual(['d'])
  })

  it('everything already wired means an empty target set', () => {
    const steps = [step('a', 'head'), step('b', 'blocked')]
    expect(edgeCandidateSteps(steps, edges(['a', 'b']), 'a')).toEqual([])
  })
})

describe('edgeDropOutcome', () => {
  const steps = [step('a', 'head'), step('b', 'blocked'), step('c', 'blocked'), step('d', 'done')]
  const list = edges(['a', 'b'])

  it('an in-port drop creates the edge', () => {
    expect(edgeDropOutcome(steps, list, 'a', { taskId: 'c', side: 'in' })).toEqual({
      kind: 'create',
      toTaskId: 'c',
    })
  })

  it('an out-port drop reads as a mis-drop', () => {
    expect(edgeDropOutcome(steps, list, 'a', { taskId: 'c', side: 'out' })).toEqual({ kind: 'cancel' })
  })

  it('a node drop on an unwired step creates the edge', () => {
    expect(edgeDropOutcome(steps, list, 'a', { taskId: 'c', side: null })).toEqual({
      kind: 'create',
      toTaskId: 'c',
    })
  })

  it('the source card itself cancels', () => {
    expect(edgeDropOutcome(steps, list, 'a', { taskId: 'a', side: 'in' })).toEqual({ kind: 'cancel' })
  })

  it('an already-wired target cancels — not a duplicate write', () => {
    expect(edgeDropOutcome(steps, list, 'a', { taskId: 'b', side: 'in' })).toEqual({ kind: 'cancel' })
  })

  it('a step that reached the source would cycle — cancel (the api also refuses)', () => {
    expect(edgeDropOutcome(steps, edges(['a', 'b']), 'b', { taskId: 'a', side: 'in' })).toEqual({
      kind: 'cycle',
    })
  })

  it('a far miss (no hit at all) cancels', () => {
    expect(edgeDropOutcome(steps, list, 'a', null)).toEqual({ kind: 'cancel' })
  })
})

describe('WIRE_HIT_RADIUS', () => {
  it('stays tighter than the port target — wires are dense, ports are not', () => {
    expect(WIRE_HIT_RADIUS).toBeLessThanOrEqual(12)
    expect(WIRE_HIT_RADIUS).toBeGreaterThan(0)
  })
})
