import { describe, expect, it } from 'vitest'
import { STALE_MS, atPath, decide, dndHolds, hrefPath, selfLooking, visitReadIds, winnerIsHere, type DecideInput, type TabState } from './notify-decide'

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

// ── Wave 2: the full matrix, in rule order ───────────────────────────────────

describe('decide — wave 2 rules, in order', () => {
  const looking = { visible: true, focused: true, tabId: 'me', lastInputAt: NOW - 5_000, clientId: 'c-me' }
  const hiddenSelf = { visible: false, focused: false, tabId: 'me', lastInputAt: NOW - 5_000, clientId: 'c-me' }

  it('1. read beats everything', () => {
    expect(decide({ ...base, read: true, self: looking, atRowPath: true, isLeader: true })).toBe('none')
  })

  it('2. a feed row is never toasted or bannered (I-15) — looking, leader, nobody else', () => {
    expect(decide({ ...base, alert: 'feed', self: looking, isLeader: true })).toBe('none')
    expect(decide({ ...base, alert: 'feed', self: hiddenSelf, isLeader: true })).toBe('none')
    expect(decide({ ...base, alert: 'feed', self: hiddenSelf, isLeader: true, bannersOn: false })).toBe('none')
    // null / absent alert is a banner row (an api before the contract).
    expect(decide({ ...base, alert: null, self: hiddenSelf, isLeader: true })).toBe('banner')
    expect(decide({ ...base, self: hiddenSelf, isLeader: true })).toBe('banner')
  })

  it('3. Do Not Disturb holds everything but a breaks-through class (I-14)', () => {
    const dnd = { active: true }
    expect(decide({ ...base, dnd, self: hiddenSelf, isLeader: true })).toBe('none')
    expect(decide({ ...base, dnd, self: looking })).toBe('none')
    expect(decide({ ...base, dnd, breaksThrough: true, self: hiddenSelf, isLeader: true })).toBe('banner')
    expect(decide({ ...base, dnd, breaksThrough: true, self: looking })).toBe('toast')
    expect(decide({ ...base, dnd: { active: false }, self: hiddenSelf, isLeader: true })).toBe('banner')
  })

  it('4. looking at the row\'s own page: read on sight, no toast (EB-13)', () => {
    expect(decide({ ...base, atRowPath: true, self: looking, isLeader: true })).toBe('seen')
  })

  it('4. …but a hidden tab on that page never reads it: it banners (C8)', () => {
    expect(decide({ ...base, atRowPath: true, self: hiddenSelf, isLeader: true })).toBe('banner')
  })

  it('4. …nor a focused tab nobody touched for a minute (it toasts, as wave 1 did)', () => {
    const still = { ...looking, lastInputAt: NOW - 61_000 }
    expect(decide({ ...base, atRowPath: true, self: still })).toBe('toast')
    expect(decide({ ...base, atRowPath: true, self: { ...looking, lastInputAt: 0 } })).toBe('toast')
  })

  it('4. …nor a visible, unfocused tab', () => {
    expect(decide({ ...base, atRowPath: true, self: { ...looking, focused: false }, isLeader: true })).toBe('banner')
  })

  it('5. this tab focused and visible toasts — even when the winner is elsewhere', () => {
    expect(decide({ ...base, self: looking, winner: 'c-phone' })).toBe('toast')
  })

  it('6. another tab here looking: nothing, before any presence rule', () => {
    const other = tab({ tabId: 'x', visible: true, focused: true, clientId: 'c-x' })
    expect(decide({ ...base, self: hiddenSelf, others: [other], isLeader: true, winner: 'c-x' })).toBe('none')
  })

  it('7. the winner is another device: this browser stays quiet — leader or not, banners on or off', () => {
    expect(decide({ ...base, self: hiddenSelf, isLeader: true, winner: 'c-laptop-b' })).toBe('none')
    expect(decide({ ...base, self: { ...hiddenSelf, visible: true }, isLeader: true, bannersOn: false, winner: 'c-laptop-b' })).toBe('none')
  })

  it('7. the winner is this tab or another tab of this browser: wave 1\'s tail decides', () => {
    expect(decide({ ...base, self: hiddenSelf, isLeader: true, winner: 'c-me' })).toBe('banner')
    // The person is active on a second-monitor tab; the hidden leader banners.
    const side = tab({ tabId: 'side', visible: true, focused: false, clientId: 'c-side' })
    expect(decide({ ...base, self: hiddenSelf, others: [side], isLeader: true, winner: 'c-side' })).toBe('banner')
  })

  it('7. a winner that is a STALE tab of this browser counts as elsewhere', () => {
    const gone = tab({ tabId: 'gone', clientId: 'c-gone', at: NOW - STALE_MS - 1 })
    expect(decide({ ...base, self: hiddenSelf, others: [gone], isLeader: true, winner: 'c-gone' })).toBe('none')
  })

  it("7. in the desktop shell, its own watcher (desk-<local instance id>) is THIS device: the restored page alerts", () => {
    // After a restore the watcher's last `active` beat can be the winner for
    // up to 75 s while the page's own first state is `idle`.
    const desk = 'desk-7d1f0f6e-3a52-4c1e-9d55-0c3f8f2b9a11'
    expect(decide({ ...base, self: hiddenSelf, isLeader: true, winner: desk, sameDevice: [desk] })).toBe('banner')
    expect(decide({ ...base, self: { ...hiddenSelf, visible: true }, isLeader: true, bannersOn: false, winner: desk, sameDevice: [desk] })).toBe('toast')
    // Without it (the defect): quiet, "on another device".
    expect(decide({ ...base, self: hiddenSelf, isLeader: true, winner: desk })).toBe('none')
    // Another machine's watcher is another device.
    expect(decide({ ...base, self: hiddenSelf, isLeader: true, winner: 'desk-0b9e2c1d-5f4a-4e3b-8c2d-1a0f9e8d7c6b', sameDevice: [desk] })).toBe('none')
  })

  it('no presence (null / absent winner): wave 1, exactly', () => {
    expect(decide({ ...base, self: hiddenSelf, isLeader: true, winner: null })).toBe('banner')
    expect(decide({ ...base, self: hiddenSelf, isLeader: false, winner: null })).toBe('none')
  })
})

describe('the helpers', () => {
  const O = 'http://localhost:5310'
  it('selfLooking needs input within 60 s', () => {
    expect(selfLooking({ visible: true, focused: true, lastInputAt: NOW - 60_000 }, NOW)).toBe(true)
    expect(selfLooking({ visible: true, focused: true, lastInputAt: NOW - 60_001 }, NOW)).toBe(false)
    expect(selfLooking({ visible: true, focused: true }, NOW)).toBe(false)
  })
  it('winnerIsHere', () => {
    expect(winnerIsHere({ winner: null, self: { visible: false, focused: false }, others: [], now: NOW })).toBe(true)
    expect(winnerIsHere({ winner: 'a', self: { visible: false, focused: false, clientId: 'a' }, others: [], now: NOW })).toBe(true)
    expect(winnerIsHere({ winner: 'a', self: { visible: false, focused: false, clientId: 'b' }, others: [], now: NOW })).toBe(false)
    expect(winnerIsHere({ winner: 'desk-x', self: { visible: false, focused: false, clientId: 'b' }, others: [], sameDevice: ['desk-x'], now: NOW })).toBe(true)
    expect(winnerIsHere({ winner: 'desk-y', self: { visible: false, focused: false, clientId: 'b' }, others: [], sameDevice: ['desk-x'], now: NOW })).toBe(false)
  })
  it('hrefPath and atPath compare the path only, on this origin', () => {
    expect(hrefPath('/comms/channel/x?m=1#y', O)).toBe('/comms/channel/x')
    expect(hrefPath('/comms/channel/x/', O)).toBe('/comms/channel/x')
    expect(hrefPath('//evil.example/x', O)).toBeNull()
    expect(hrefPath('https://evil.example/x', O)).toBeNull()
    expect(hrefPath('', O)).toBeNull()
    expect(atPath('/comms/channel/x?m=2', '/comms/channel/x', O)).toBe(true)
    expect(atPath('/comms/channel/x', '/comms/channel/y', O)).toBe(false)
    expect(atPath('/', '/', O)).toBe(true)
  })
  it('visitReadIds reads direct and following rows at this path, never needs-you, never an unknown category', () => {
    const rows = [
      { id: 'dm', href: '/comms/channel/x', readAt: null, category: 'direct' },
      { id: 'fol', href: '/comms/channel/x', readAt: null, category: 'following' },
      { id: 'ny', href: '/comms/channel/x', readAt: null, category: 'needs_you' },
      { id: 'old', href: '/comms/channel/x', readAt: null },
      { id: 'read', href: '/comms/channel/x', readAt: '2026-11-01T00:00:00Z', category: 'direct' },
      { id: 'else', href: '/comms/channel/y', readAt: null, category: 'direct' },
    ]
    expect(visitReadIds(rows, '/comms/channel/x', O)).toEqual(['dm', 'fol'])
  })
})

describe('dndHolds — Do Not Disturb as of now, not as of the list read', () => {
  const iso = (ms: number) => new Date(ms).toISOString()
  it('off, or absent, holds nothing', () => {
    expect(dndHolds(null, NOW)).toBe(false)
    expect(dndHolds(undefined, NOW)).toBe(false)
    expect(dndHolds({ active: false, until: iso(NOW + 60_000) }, NOW)).toBe(false)
  })
  it('active with no end, or an end still ahead (until I resume included), holds', () => {
    expect(dndHolds({ active: true }, NOW)).toBe(true)
    expect(dndHolds({ active: true, until: null }, NOW)).toBe(true)
    expect(dndHolds({ active: true, until: iso(NOW + 1) }, NOW)).toBe(true)
    expect(dndHolds({ active: true, until: '9999-12-31T23:59:59.999Z' }, NOW)).toBe(true)
  })
  it('a pause that ended since the list was read holds nothing: the next arrival alerts', () => {
    expect(dndHolds({ active: true, until: iso(NOW) }, NOW)).toBe(false)
    expect(dndHolds({ active: true, until: iso(NOW - 5_000) }, NOW)).toBe(false)
    const row = { ...base, self: { visible: false, focused: false }, isLeader: true }
    expect(decide({ ...row, dnd: { active: dndHolds({ active: true, until: iso(NOW - 5_000) }, NOW) } })).toBe('banner')
  })
  it('an unreadable end is taken as still holding', () => {
    expect(dndHolds({ active: true, until: 'soon' }, NOW)).toBe(true)
  })
})
