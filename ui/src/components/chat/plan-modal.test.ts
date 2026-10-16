// The batch's hierarchy rules, driven at the pure helpers they live in
// (plan-modal.ts) — and the store's normalize, which is the other half of the
// same contract: what the drafting agent's JSON is allowed to leave out.
//
// There is no component-test precedent in ui/src, and these are the two edges
// that can quietly file a ticket under the wrong parent — or under a parent
// that was never created — so they are tested directly rather than by walking
// the modal.
import { describe, expect, it } from 'vitest'
import { createOrder, isEpic, parentIndex, ticketAssignee, type Proposal } from './plan-modal'
import { normalizeProposal } from './plan-drafts.svelte'

const draft = (over: Partial<Proposal> = {}): Proposal => ({
  title: 'a ticket',
  description: '',
  priority: 'medium',
  effort: null,
  dependsOn: [],
  tags: [],
  parent: null,
  assignees: [],
  duplicateOf: null,
  include: true,
  ...over,
})

describe('createOrder', () => {
  it('keeps the drafter’s order when nothing hangs under anything', () => {
    const batch = [draft(), draft(), draft()]
    expect(createOrder(batch)).toEqual([0, 1, 2])
  })

  it('creates a parent named later in the array first', () => {
    // The drafter emitted the child first; the epic has to exist anyway.
    const batch = [draft({ parent: 1 }), draft()]
    expect(createOrder(batch)).toEqual([1, 0])
    expect(parentIndex(batch, 0)).toBe(1)
  })

  it('files an excluded parent’s children top-level', () => {
    const batch = [draft({ include: false }), draft({ parent: 0 }), draft({ parent: 0 })]
    expect(createOrder(batch)).toEqual([1, 2])
    expect(parentIndex(batch, 1)).toBeNull()
    expect(parentIndex(batch, 2)).toBeNull()
  })

  it('treats a parent that is itself a child as top-level — one level only', () => {
    const batch = [draft(), draft({ parent: 0 }), draft({ parent: 1 })]
    // #2 hangs under #1, so #1 is an epic and drops its own parent rather
    // than building a third tier.
    expect(parentIndex(batch, 1)).toBeNull()
    expect(parentIndex(batch, 2)).toBe(1)
    expect(createOrder(batch)).toEqual([0, 1, 2])
  })

  it('skips the proposals the reviewer dropped', () => {
    const batch = [draft(), draft({ include: false }), draft({ parent: 0, include: false })]
    expect(createOrder(batch)).toEqual([0])
  })

  it('survives a drafter’s slip — a parent index that points at itself or at nothing', () => {
    const batch = [draft({ parent: 0 }), draft({ parent: 9 })]
    expect(createOrder(batch)).toEqual([0, 1])
    expect(parentIndex(batch, 0)).toBeNull()
    expect(parentIndex(batch, 1)).toBeNull()
  })
})

describe('isEpic', () => {
  it('is a relationship, not a flag: named by an included proposal', () => {
    const batch = [draft(), draft({ parent: 0 })]
    expect(isEpic(batch, 0)).toBe(true)
    expect(isEpic(batch, 1)).toBe(false)
  })

  it('stops being an epic when its last child is dropped', () => {
    const batch = [draft(), draft({ parent: 0, include: false })]
    expect(isEpic(batch, 0)).toBe(false)
  })
})

describe('normalizeProposal', () => {
  it('defaults the three fields a terse draft row leaves out', () => {
    const terse = {
      title: 'a ticket',
      description: '',
      priority: 'medium',
      effort: null,
    } as unknown as Proposal
    expect(normalizeProposal(terse)).toMatchObject({
      parent: null,
      assignees: [],
      duplicateOf: null,
      dependsOn: [],
      tags: [],
      include: true,
    })
  })

  it('keeps what the drafter did send', () => {
    const row = draft({ parent: 2, assignees: ['u-1'], duplicateOf: 't-9', include: false })
    expect(normalizeProposal(row)).toMatchObject({
      parent: 2,
      assignees: ['u-1'],
      duplicateOf: 't-9',
      // A possible duplicate arrives unticked from the server; normalize must
      // not tick it back on.
      include: false,
    })
  })
})

describe('ticketAssignee', () => {
  it('spells a drafted member id the way a ticket does', () => {
    // The drafter answers with the bare id it read off the board block; the
    // ticket reads a bare string as an AGENT model, so the prefix is the whole
    // difference between assigning a person and assigning nobody.
    expect(ticketAssignee('3f1c7a2e')).toBe('user:3f1c7a2e')
    // Already prefixed — e.g. a draft edited after a round-trip — stays put
    // rather than stacking a second prefix.
    expect(ticketAssignee('user:3f1c7a2e')).toBe('user:3f1c7a2e')
  })
})
