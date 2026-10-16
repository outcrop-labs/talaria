import { describe, expect, it } from 'vitest'
import type { Conversation } from '@/lib/conversations.svelte'
import { filterSessions, nextIndex } from './session-palette'

const conv = (
  id: string,
  extra: { title?: string | null; group?: string | null; pinned?: boolean } = {},
): Conversation => ({
  id,
  agentModel: 'agent-1',
  title: extra.title === undefined ? id : extra.title,
  updatedAt: '2026-10-16T12:00:00.000Z',
  group: extra.group ?? null,
  pinned: extra.pinned,
})

const ids = (list: Conversation[]) => list.map((c) => c.id)

describe('filterSessions', () => {
  const list = [
    conv('drive-audit', { title: 'Drive billing audit' }),
    conv('q4', { title: 'Q4 plan', group: 'Billing' }),
    conv('rename', { title: 'Rename the fleet' }),
    conv('pinned', { title: 'Zebra counts', pinned: true }),
  ]

  it('lists everything on an empty query, pinned first', () => {
    expect(ids(filterSessions(list, ''))).toEqual(['pinned', 'drive-audit', 'q4', 'rename'])
    expect(ids(filterSessions(list, '   '))).toEqual(['pinned', 'drive-audit', 'q4', 'rename'])
  })

  it('matches titles without regard to case', () => {
    expect(ids(filterSessions(list, 'DRIVE'))).toEqual(['drive-audit'])
    expect(ids(filterSessions(list, 'fleet'))).toEqual(['rename'])
  })

  it('matches the group a thread is filed under', () => {
    expect(ids(filterSessions(list, 'billing'))).toEqual(['drive-audit', 'q4'])
  })

  it('needs every word, in any order', () => {
    expect(ids(filterSessions(list, 'audit drive'))).toEqual(['drive-audit'])
    expect(ids(filterSessions(list, 'drive fleet'))).toEqual([])
  })

  it('keeps pinned threads at the top of a filtered list', () => {
    const counted = [conv('a', { title: 'counts up' }), conv('z', { title: 'counts down', pinned: true })]
    expect(ids(filterSessions(counted, 'counts'))).toEqual(['z', 'a'])
  })

  it('holds the incoming order among equals', () => {
    expect(ids(filterSessions([conv('b'), conv('a')], ''))).toEqual(['b', 'a'])
  })

  it('never matches an untitled thread on a query', () => {
    const untitled = [conv('u', { title: null })]
    expect(ids(filterSessions(untitled, ''))).toEqual(['u'])
    expect(ids(filterSessions(untitled, 'untitled'))).toEqual([])
  })

  it('leaves the caller’s array alone', () => {
    const source = [conv('a'), conv('z', { pinned: true })]
    filterSessions(source, '')
    expect(ids(source)).toEqual(['a', 'z'])
  })
})

describe('nextIndex', () => {
  it('steps forward and back', () => {
    expect(nextIndex(0, 1, 3)).toBe(1)
    expect(nextIndex(2, -1, 3)).toBe(1)
  })

  it('wraps at both ends', () => {
    expect(nextIndex(2, 1, 3)).toBe(0)
    expect(nextIndex(0, -1, 3)).toBe(2)
  })

  it('answers 0 for an empty list', () => {
    expect(nextIndex(0, 1, 0)).toBe(0)
    expect(nextIndex(4, -1, 0)).toBe(0)
  })

  it('pulls a stale index back inside the list', () => {
    expect(nextIndex(9, 1, 3)).toBe(1)
  })
})
