import { describe, expect, it } from 'vitest'
import { STALE_MS, decide, type DecideInput, type TabState } from './notify-decide'

const NOW = 1_800_000_000_000
const tab = (over: Partial<TabState> = {}): TabState => ({
  tabId: 't-other',
  visible: false,
  focused: false,
  lastInputAt: 0,
  at: NOW - 1000,
  ...over,
})
const base: DecideInput = {
  read: false,
  self: { visible: false, focused: false },
  others: [],
  isLeader: false,
  now: NOW,
}

describe('decide — one banner per notification per browser', () => {
  it('does nothing for a row already read', () => {
    expect(decide({ ...base, read: true, self: { visible: true, focused: true }, isLeader: true })).toBe('none')
  })

  it('toasts in the tab the person is looking at, leader or not', () => {
    expect(decide({ ...base, self: { visible: true, focused: true } })).toBe('toast')
    expect(decide({ ...base, self: { visible: true, focused: true }, isLeader: true })).toBe('toast')
  })

  it('stays quiet when another fresh tab is looked at — even the leader', () => {
    const looked = tab({ visible: true, focused: true })
    expect(decide({ ...base, others: [looked], isLeader: true })).toBe('none')
    expect(decide({ ...base, others: [looked] })).toBe('none')
  })

  it('ignores a looked-at tab that went stale', () => {
    const stale = tab({ visible: true, focused: true, at: NOW - STALE_MS - 1 })
    expect(decide({ ...base, others: [stale], isLeader: true })).toBe('banner')
  })

  it('a visible but unfocused tab is not "looked at" (side-by-side window)', () => {
    expect(decide({ ...base, others: [tab({ visible: true, focused: false })], isLeader: true })).toBe('banner')
    expect(decide({ ...base, self: { visible: true, focused: false }, isLeader: true })).toBe('banner')
  })

  it('only the leader posts the banner when nobody is looking', () => {
    const others = [tab({ tabId: 'a' }), tab({ tabId: 'b' })]
    expect(decide({ ...base, others, isLeader: true })).toBe('banner')
    expect(decide({ ...base, others, isLeader: false })).toBe('none')
  })

  it('a lone tab with no locks decides as its own leader', () => {
    expect(decide({ ...base, isLeader: true })).toBe('banner')
  })
})
