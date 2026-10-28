import { describe, expect, it } from 'vitest'
import { containersOf, editNotApplied, type AgentContainers, type DepartmentShare } from './fleet-defs'

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

// A config save answers `applied: false` with a `warning` when the apply it
// asked for did not land. The editor shows the sentence and stays open; it
// used to read only `notes`, so a failed roll closed the modal in silence.
describe('editNotApplied', () => {
  it('says the version saved and why it is not running', () => {
    expect(
      editNotApplied({ version: 4, warning: 'department "research" is already mid-roll — wait for it to finish' }),
    ).toBe('Saved as version 4, not applied: department "research" is already mid-roll — wait for it to finish')
  })

  it('still says it without a version number', () => {
    expect(editNotApplied({ warning: 'no managed agent in department "ops"' })).toBe(
      'Saved, not applied: no managed agent in department "ops"',
    )
  })

  it('is nothing for an apply that landed, or a save that asked for none', () => {
    expect(editNotApplied({ version: 2 })).toBeNull()
    expect(editNotApplied({ version: 2, warning: '  ' })).toBeNull()
  })
})
