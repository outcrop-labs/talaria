import { describe, expect, it } from 'vitest'
import type { Conversation } from '@/lib/conversations.svelte'
import {
  layoutMenuItems,
  POSITION_STEP,
  railGroups,
  railSections,
  reorderPosition,
} from './conversation-sidebar'

// Rows carry only what the rail reads. `at` is minutes of staleness, so a
// bigger number is an older row and the expected order reads left to right.
const conv = (
  id: string,
  extra: { at?: number; pinned?: boolean; group?: string | null; position?: number | null } = {},
): Conversation => ({
  id,
  agentModel: 'agent-1',
  title: id.toUpperCase(),
  updatedAt: new Date(Date.UTC(2026, 9, 16, 12, 0, 0) - (extra.at ?? 0) * 60_000).toISOString(),
  pinned: extra.pinned,
  group: extra.group ?? null,
  position: extra.position ?? null,
})

const ids = (items: Conversation[]) => items.map((c) => c.id)

describe('railSections', () => {
  it('runs pinned, then each group alphabetically, then the rest', () => {
    const sections = railSections([
      conv('loose', { at: 1 }),
      conv('zeta', { group: 'Zeta' }),
      conv('billing', { group: 'Billing' }),
      conv('top', { pinned: true }),
      conv('other', { at: 2 }),
    ])
    expect(sections.map((s) => s.key)).toEqual([
      'pinned',
      'group:Billing',
      'group:Zeta',
      'ungrouped',
    ])
    expect(sections.map((s) => s.label)).toEqual(['Pinned', 'Billing', 'Zeta', null])
    expect(sections.map((s) => s.group)).toEqual([null, 'Billing', 'Zeta', null])
    expect(ids(sections[3]!.items)).toEqual(['loose', 'other'])
  })

  it('sorts group names case-insensitively and merges names that only differ by padding', () => {
    const sections = railSections([
      conv('a', { group: 'ops' }),
      conv('b', { group: 'Billing' }),
      conv('c', { group: ' Billing ' }),
    ])
    expect(sections.map((s) => s.label)).toEqual(['Billing', 'ops'])
    expect(ids(sections[0]!.items)).toEqual(['b', 'c'])
  })

  it('lists a pinned row ONCE — under Pinned, never also under its group', () => {
    const sections = railSections([
      conv('pinned-in-group', { pinned: true, group: 'Billing' }),
      conv('plain', { group: 'Billing' }),
    ])
    expect(ids(sections[0]!.items)).toEqual(['pinned-in-group'])
    expect(sections[1]!.key).toBe('group:Billing')
    expect(ids(sections[1]!.items)).toEqual(['plain'])
  })

  it('orders every section by position ascending, nulls last, then newest first', () => {
    const [section] = railSections([
      conv('unplaced-old', { at: 50 }),
      conv('third', { position: 30 }),
      conv('unplaced-new', { at: 1 }),
      conv('first', { position: 10 }),
    ])
    expect(ids(section!.items)).toEqual(['first', 'third', 'unplaced-new', 'unplaced-old'])
  })

  it('breaks a tie between two rows on the same slot with the newer one', () => {
    const [section] = railSections([
      conv('older', { position: 10, at: 30 }),
      conv('newer', { position: 10, at: 1 }),
    ])
    expect(ids(section!.items)).toEqual(['newer', 'older'])
  })

  it('omits empty sections — a header over nothing claims a feature nobody used', () => {
    expect(railSections([])).toEqual([])
    expect(railSections([conv('a')]).map((s) => s.key)).toEqual(['ungrouped'])
    expect(railSections([conv('a', { pinned: true })]).map((s) => s.key)).toEqual(['pinned'])
  })

  it('leaves the caller’s array alone', () => {
    const list = [conv('b', { position: 20 }), conv('a', { position: 10 })]
    railSections(list)
    expect(ids(list)).toEqual(['b', 'a'])
  })
})

describe('railGroups', () => {
  it('names every group, deduped, trimmed and sorted', () => {
    expect(
      railGroups([
        conv('a', { group: 'ops' }),
        conv('b', { group: 'Billing' }),
        conv('c', { group: ' Billing ' }),
        conv('d'),
      ]),
    ).toEqual(['Billing', 'ops'])
  })

  it('keeps a group alive whose only row is pinned — the shelf still exists', () => {
    expect(railGroups([conv('a', { pinned: true, group: 'Billing' })])).toEqual(['Billing'])
  })
})

describe('reorderPosition', () => {
  const placed = [
    conv('a', { position: 10 }),
    conv('b', { position: 20 }),
    conv('c', { position: 30 }),
  ]

  it('takes the midpoint when a row is dropped between two neighbours — one write', () => {
    // a dragged DOWN onto b lands after it: between 20 and 30.
    expect(reorderPosition(placed, 'a', 'b')).toEqual([{ id: 'a', position: 25 }])
  })

  it('lands before the target when the drag goes up', () => {
    // c dragged UP onto b lands between a (10) and b (20) — there is only
    // one integer in that gap, and this is it.
    expect(reorderPosition(placed, 'c', 'b')).toEqual([{ id: 'c', position: 15 }])
  })

  it('halves the first slot at the top of the list', () => {
    expect(reorderPosition(placed, 'c', 'a')).toEqual([{ id: 'c', position: 5 }])
  })

  it('steps past the last slot at the bottom of the list', () => {
    expect(reorderPosition(placed, 'a', 'c')).toEqual([{ id: 'a', position: 40 }])
  })

  it('renumbers by tens on the first drop in a list nobody has ordered', () => {
    const fresh = [conv('a', { at: 1 }), conv('b', { at: 2 }), conv('c', { at: 3 })]
    expect(reorderPosition(fresh, 'c', 'a')).toEqual([
      { id: 'c', position: 10 },
      { id: 'a', position: 20 },
      { id: 'b', position: 30 },
    ])
  })

  it('renumbers when the neighbours have no integer between them', () => {
    const tight = [
      conv('a', { position: 10 }),
      conv('b', { position: 11 }),
      conv('c', { position: 12 }),
    ]
    expect(reorderPosition(tight, 'a', 'b')).toEqual([
      { id: 'b', position: 10 },
      { id: 'a', position: 20 },
      { id: 'c', position: 30 },
    ])
  })

  it('renumbers rather than writing past the slot the server refuses', () => {
    const far = [conv('a', { position: 99_995 }), conv('b', { position: 100_000 })]
    expect(reorderPosition(far, 'a', 'b')).toEqual([
      { id: 'b', position: 10 },
      { id: 'a', position: 20 },
    ])
  })

  it('drops a placed row ahead of the unplaced tail with one write', () => {
    const mixed = [conv('a', { position: 10 }), conv('b', { at: 1 }), conv('c', { at: 2 })]
    expect(reorderPosition(mixed, 'c', 'b')).toEqual([
      { id: 'c', position: 10 + POSITION_STEP },
    ])
  })

  it('writes nothing for a drop on itself or on a row that is not in the list', () => {
    expect(reorderPosition(placed, 'a', 'a')).toEqual([])
    expect(reorderPosition(placed, 'a', 'nope')).toEqual([])
    expect(reorderPosition(placed, 'nope', 'a')).toEqual([])
  })

  it('reads the list in rail order, not the order it was handed', () => {
    const shuffled = [placed[2]!, placed[0]!, placed[1]!]
    expect(reorderPosition(shuffled, 'a', 'b')).toEqual([{ id: 'a', position: 25 }])
  })
})

describe('layoutMenuItems', () => {
  const actions = { pin: () => {}, moveTo: () => {}, newGroup: () => {}, ungroup: () => {} }
  const labels = (items: ReturnType<typeof layoutMenuItems>) =>
    items.map((i) => (i === 'sep' ? 'sep' : i.label))

  it('offers Pin on an unpinned row and Unpin on a pinned one', () => {
    expect(labels(layoutMenuItems(conv('a'), [], actions))[0]).toBe('Pin')
    expect(labels(layoutMenuItems(conv('a', { pinned: true }), [], actions))[0]).toBe('Unpin')
  })

  it('offers only the groups the row is not already in, plus a new one', () => {
    const items = layoutMenuItems(conv('a', { group: 'Billing' }), ['Billing', 'ops'], actions)
    const move = items[1]!
    expect(move === 'sep' ? [] : labels(move.children ?? [])).toEqual(['ops', 'sep', 'New group'])
  })

  it('drops the separator when New group is the only entry', () => {
    const move = layoutMenuItems(conv('a'), [], actions)[1]!
    expect(move === 'sep' ? [] : labels(move.children ?? [])).toEqual(['New group'])
  })

  it('offers Remove from group only on a row that is in one', () => {
    expect(labels(layoutMenuItems(conv('a'), ['ops'], actions))).toEqual(['Pin', 'Move to group'])
    expect(labels(layoutMenuItems(conv('a', { group: 'ops' }), ['ops'], actions))).toEqual([
      'Pin',
      'Move to group',
      'Remove from group',
    ])
  })
})
