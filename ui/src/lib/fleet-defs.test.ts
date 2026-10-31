import { describe, expect, it } from 'vitest'
import { containersOf, editNotApplied, ROLL_STILL_STARTING, rollInProgress, rollRowsBySlug, type AgentContainers, type DepartmentShare, type FleetRollRow } from './fleet-defs'

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
    // An answer that carries no `inEffect` either way (an api from before
    // the route could tell) closes a fresh form, as it always did.
    expect(editNotApplied({ version: 3, created: false, applied: false })).toBeNull()
  })

  // The route now tells a no-op Save & apply whether the agent runs the
  // current version — so a fresh form no longer closes on one that never
  // applied, and an old memory no longer outranks the route.
  it('says why a no-op apply did not put the current version on', () => {
    expect(
      editNotApplied({
        version: 6,
        created: false,
        applied: false,
        inEffect: false,
        warning: 'it was saved and never applied — Bob still runs version 5; nothing changed since, so Save & apply had nothing new to apply. Roll the agent to put it on',
      }),
    ).toBe(
      'Version 6 is not applied: it was saved and never applied — Bob still runs version 5; nothing changed since, so Save & apply had nothing new to apply. Roll the agent to put it on',
    )
  })

  it('closes quietly on a no-op apply the route says is in effect, whatever the form remembers', () => {
    expect(editNotApplied({ version: 3, created: false, applied: false, inEffect: true })).toBeNull()
    // Rolled from the roster since the form saw version 3 fail.
    expect(editNotApplied({ version: 3, created: false, applied: false, inEffect: true }, held)).toBeNull()
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

// A roll's row holds the Roll controls only while the roll is in its steps. A
// row noted `still starting` has let go of its locks (the route takes a new
// roll), and its late wait can run twelve minutes: it must not hold them.
describe('rollInProgress', () => {
  const row = (state: FleetRollRow['state'], detail: string | null = null): FleetRollRow => ({
    slug: 'ava',
    department: 'research',
    displayName: 'Ava',
    state,
    since: 1,
    detail,
    batch: null,
  })

  it('is a roll in its steps, queued ones included', () => {
    expect(rollInProgress(row('rolling'))).toBe(true)
    expect(rollInProgress(row('rolling', 'queued'))).toBe(true)
  })

  it('is not a roll whose container is still starting', () => {
    expect(ROLL_STILL_STARTING).toBe('still starting')
    expect(rollInProgress(row('rolling', ROLL_STILL_STARTING))).toBe(false)
  })

  it('is not a finished roll, or no row at all', () => {
    expect(rollInProgress(row('done'))).toBe(false)
    expect(rollInProgress(row('failed', 'the recreated container stopped running before it reported healthy'))).toBe(false)
    expect(rollInProgress(row('idle'))).toBe(false)
    expect(rollInProgress(undefined)).toBe(false)
    expect(rollInProgress(null)).toBe(false)
  })
})

// The roster reads each agent's own row. A pair that shares a department has
// two rows with one department: the runner's roll, and the left-out agent's
// idle row. Keyed by department, the last one won and both tiles read it.
describe('rollRowsBySlug', () => {
  const runner: FleetRollRow = { slug: 'pippa', department: 'personal-pip', displayName: 'Pippa', state: 'rolling', since: 5, detail: null, batch: 'b1' }
  const leftOut: FleetRollRow = { slug: 'pipdup', department: 'personal-pip', displayName: 'Pip Dup', state: 'idle', since: null, detail: null, batch: null }

  it('gives each agent of a shared department its own row, in either order', () => {
    for (const rows of [[runner, leftOut], [leftOut, runner]]) {
      const bySlug = rollRowsBySlug(rows)
      expect(bySlug.get('pippa')).toBe(runner)
      expect(bySlug.get('pipdup')).toBe(leftOut)
      expect(rollInProgress(bySlug.get('pippa'))).toBe(true)
      expect(rollInProgress(bySlug.get('pipdup'))).toBe(false)
    }
  })

  it('is empty with no picture yet', () => {
    expect(rollRowsBySlug(undefined).size).toBe(0)
    expect(rollRowsBySlug(null).size).toBe(0)
  })
})
