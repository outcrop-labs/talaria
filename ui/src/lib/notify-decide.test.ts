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

// Banners off: not granted (the soft ask's whole audience), a plain-http
// origin, or switched off in this browser. The leader has nothing to post,
// so a visible tab must show it — the review's live case was a hidden
// leader and a visible-but-unfocused second window, and nothing showed.
describe('decide — banners off in this browser', () => {
  const off = { ...base, bannersOn: false }

  it('the hidden leader and a visible unfocused tab: the visible tab toasts', () => {
    const visibleTab = { visible: true, focused: false, tabId: 'b', lastInputAt: 0 }
    const hiddenLeader = tab({ tabId: 'a', visible: false, focused: false })
    expect(decide({ ...off, self: visibleTab, others: [hiddenLeader], isLeader: false })).toBe('toast')
    // …and the leader, hidden, shows nothing (no banner to post).
    expect(decide({ ...off, self: { visible: false, focused: false, tabId: 'a' }, others: [tab({ tabId: 'b', visible: true })], isLeader: true })).toBe('none')
  })

  it('the looked-at tab still comes first', () => {
    const looked = tab({ tabId: 'a', visible: true, focused: true, lastInputAt: 1 })
    expect(decide({ ...off, self: { visible: true, focused: false, tabId: 'b', lastInputAt: 9 }, others: [looked] })).toBe('none')
    expect(decide({ ...off, self: { visible: true, focused: true, tabId: 'a' }, others: [] })).toBe('toast')
  })

  it('two visible unfocused tabs: exactly one toasts — the one touched last', () => {
    const a = { tabId: 'a', lastInputAt: 100 }
    const b = { tabId: 'b', lastInputAt: 200 }
    const asOther = (t: { tabId: string; lastInputAt: number }) => tab({ ...t, visible: true, focused: false })
    const atA = decide({ ...off, self: { visible: true, focused: false, ...a }, others: [asOther(b)] })
    const atB = decide({ ...off, self: { visible: true, focused: false, ...b }, others: [asOther(a)] })
    expect([atA, atB]).toEqual(['none', 'toast'])
  })

  it('never touched: the lowest tab id breaks the tie, so one still toasts', () => {
    const atA = decide({ ...off, self: { visible: true, focused: false, tabId: 'a' }, others: [tab({ tabId: 'b', visible: true })] })
    const atB = decide({ ...off, self: { visible: true, focused: false, tabId: 'b' }, others: [tab({ tabId: 'a', visible: true })] })
    expect([atA, atB]).toEqual(['toast', 'none'])
  })

  it('a stale announcement never outranks a live tab (the arrival is never lost to it)', () => {
    const stale = tab({ tabId: 'a', visible: true, lastInputAt: NOW, at: NOW - STALE_MS - 1 })
    expect(decide({ ...off, self: { visible: true, focused: false, tabId: 'b', lastInputAt: 1 }, others: [stale] })).toBe('toast')
  })

  it('an announcement older than the truth only ever duplicates, never drops', () => {
    // b's true last input is 300; a last heard 50 from it. a (100) thinks
    // it won, b knows it won: two toasts, not zero.
    const atA = decide({ ...off, self: { visible: true, focused: false, tabId: 'a', lastInputAt: 100 }, others: [tab({ tabId: 'b', visible: true, lastInputAt: 50 })] })
    const atB = decide({ ...off, self: { visible: true, focused: false, tabId: 'b', lastInputAt: 300 }, others: [tab({ tabId: 'a', visible: true, lastInputAt: 100 })] })
    expect([atA, atB]).toEqual(['toast', 'toast'])
  })

  it('no visible tab: nothing (the bell has it)', () => {
    expect(decide({ ...off, isLeader: true })).toBe('none')
    expect(decide({ ...off, others: [tab({ tabId: 'x' })] })).toBe('none')
  })

  it('a read row is still nothing', () => {
    expect(decide({ ...off, read: true, self: { visible: true, focused: false, tabId: 'a' } })).toBe('none')
  })

  it('banners on: unchanged — the leader posts, a visible unfocused non-leader stays quiet', () => {
    const self = { visible: true, focused: false, tabId: 'b' }
    expect(decide({ ...base, bannersOn: true, self, others: [tab({ tabId: 'a' })], isLeader: false })).toBe('none')
    expect(decide({ ...base, bannersOn: true, self, isLeader: true })).toBe('banner')
  })
})
