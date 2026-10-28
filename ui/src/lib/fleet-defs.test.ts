import { describe, expect, it } from 'vitest'
import { containersOf, type AgentContainers, type DepartmentShare } from './fleet-defs'

// Containers are found by department, and a department is one container. Where
// two agents hold one (a pair from before a department became one agent's), the
// container is the runner's alone: the other agent must not read its state.
const running: AgentContainers = {
  department: 'research',
  managed: { name: 'talaria-fleet-agent-research-1', state: 'running', status: 'Up 2 minutes (healthy)', health: 'healthy' },
}
const byDept = new Map([['research', running]])
const ava = { id: 'a', slug: 'ava', displayName: 'Ava' }
const zed = { id: 'z', slug: 'zed', displayName: 'Zed' }

describe('containersOf', () => {
  it('is the department container for an agent that holds its department alone', () => {
    expect(containersOf({ department: 'research', departmentShare: null }, byDept)).toBe(running)
    // An older api sends no departmentShare at all.
    expect(containersOf({ department: 'research' }, byDept)).toBe(running)
  })

  it('is the department container for the agent that runs in it', () => {
    const share: DepartmentShare = { runs: true, runner: ava, others: [zed] }
    expect(containersOf({ department: 'research', departmentShare: share }, byDept)).toBe(running)
  })

  it('is nothing for an agent left out of a shared department', () => {
    const share: DepartmentShare = { runs: false, runner: ava, others: [ava] }
    expect(containersOf({ department: 'research', departmentShare: share }, byDept)).toBeNull()
  })

  it('is nothing for any of them when none runs', () => {
    const share: DepartmentShare = { runs: false, runner: null, others: [zed] }
    expect(containersOf({ department: 'research', departmentShare: share }, byDept)).toBeNull()
  })

  it('is nothing for a department with no container', () => {
    expect(containersOf({ department: 'ops', departmentShare: null }, byDept)).toBeNull()
  })
})
