import { describe, expect, it } from 'vitest'
import { containersOf, editNotApplied, editStillApplying, ROLL_STILL_STARTING, rollInProgress, rollRowsBySlug, type AgentContainers, type DepartmentShare, type FleetRollRow } from './fleet-defs'

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

  // A second Save & apply of the same draft, answered with no `inEffect`
  // either way (an api from before the route said): read alone, that closed
  // the form as if the earlier version were running.
  const held = { version: 3, why: 'department "research" is already mid-roll — wait for it to finish' }

  it('keeps saying a version the form saw not apply when the retry created nothing and said nothing', () => {
    expect(editNotApplied({ version: 3, created: false, applied: false }, held)).toBe(
      'Version 3 is still not applied, as far as this form can tell: the answer did not say whether the agent runs it now. ' +
        'Roll the agent to put it on once this clears: department "research" is already mid-roll — wait for it to finish',
    )
  })

  it('says nothing for a no-op apply when the form saw no version fail', () => {
    // An answer that carries no `inEffect` either way (an api from before
    // the route could tell) closes a fresh form, as it always did.
    expect(editNotApplied({ version: 3, created: false, applied: false })).toBeNull()
  })

  // A no-op Save & apply on a version the agent does not run applies it,
  // and the route says whether the agent runs it now — so a fresh form no
  // longer closes on one that never applied, and an old memory no longer
  // outranks the route.
  it('says why a no-op apply did not put the current version on', () => {
    expect(
      editNotApplied({
        version: 6,
        created: false,
        applied: false,
        inEffect: false,
        warning: 'the roll failed — docker compose failed: Error response from daemon: no space left on device — the agent kept running on its previous container',
      }),
    ).toBe(
      'Version 6 is not applied: the roll failed — docker compose failed: Error response from daemon: no space left on device — the agent kept running on its previous container',
    )
    expect(
      editNotApplied({ version: 6, created: false, applied: false, inEffect: false, warning: 'Ada is retired, so nothing runs it — re-hire it to put it on' }),
    ).toBe('Version 6 is not applied: Ada is retired, so nothing runs it — re-hire it to put it on')
  })

  it('closes on a no-op apply that put the current version on', () => {
    expect(editNotApplied({ version: 6, created: false, applied: true, inEffect: true }, { version: 6, why: 'held' })).toBeNull()
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

// A save whose apply outlived the route's answer (a 202, `pending`): saved,
// and the apply goes on. Never "failed", never "not applied".
describe('editStillApplying', () => {
  it('says the version is saved and the apply goes on', () => {
    const said = editStillApplying({ version: 7, created: true, pending: true })
    expect(said).toMatch(/^Saved as version 7\. Applying it is taking longer than this form waits/)
    expect(said).toContain('it goes on')
    expect(said).not.toMatch(/fail|not applied/i)
    expect(editStillApplying({ pending: true })).toMatch(/^Saved\. /)
  })

  // A no-op Save & apply saved nothing new: it is applying a version saved
  // earlier, and must not claim this request saved it.
  it('says a no-op apply is applying the earlier version, not that it saved one', () => {
    const said = editStillApplying({ version: 6, created: false, pending: true })
    expect(said).toMatch(/^Applying version 6 is taking longer than this form waits/)
    expect(said).not.toMatch(/saved/i)
    expect(said).not.toMatch(/fail|not applied/i)
    expect(editStillApplying({ version: 7, created: true, pending: true })).toMatch(/^Saved as version 7\. Applying it /)
  })

  it('is silent for every answer that is not a 202', () => {
    expect(editStillApplying({ version: 7 })).toBeNull()
    expect(editStillApplying({ version: 7, pending: false })).toBeNull()
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
