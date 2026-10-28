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

  it('says a roll step that threw the same way as a soft failure', () => {
    // The route answers a thrown step (render, pull, recreate) as applied:
    // false with the reason, since the version row is already written.
    expect(
      editNotApplied({ version: 5, created: true, applied: false, warning: 'the roll failed — compose up exited 1' }),
    ).toBe('Saved as version 5, not applied: the roll failed — compose up exited 1')
  })

  // A second Save & apply of the same draft: nothing new, so the route has
  // nothing to apply and answers with no warning. Read alone, that closed the
  // form as if the earlier version were running.
  const held = { version: 3, why: 'department "research" is already mid-roll — wait for it to finish' }

  it('keeps saying a version the form saw not apply when the retry created nothing', () => {
    expect(editNotApplied({ version: 3, created: false, applied: false }, held)).toBe(
      'Version 3 is still not applied, and nothing changed since it was saved, so Save & apply had nothing new to apply. ' +
        'Roll the agent to put it on once this clears: department "research" is already mid-roll — wait for it to finish',
    )
  })

  it('says nothing for a no-op apply when the form saw no version fail', () => {
    // A fresh form cannot tell an unchanged running config from one that
    // never applied, so it closes as it always did.
    expect(editNotApplied({ version: 3, created: false, applied: false })).toBeNull()
  })

  it('says nothing once a new version applied, or for a different version', () => {
    expect(editNotApplied({ version: 4, created: true, applied: true }, held)).toBeNull()
    expect(editNotApplied({ version: 4, created: false, applied: false }, held)).toBeNull()
  })

  it('prefers a fresh warning over the remembered one', () => {
    expect(editNotApplied({ version: 4, created: true, applied: false, warning: 'no managed agent in department "desk"' }, held)).toBe(
      'Saved as version 4, not applied: no managed agent in department "desk"',
    )
  })
})
