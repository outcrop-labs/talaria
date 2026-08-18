// The fallback is the whole point of this helper: six views share it, and six
// hand-rolled versions would give six answers to "what does /models/nonsense
// do". These pin the one answer — the home tab, always something real.
import { describe, expect, it } from 'vitest'
import { activeAmong, pathId, tabFromPath, isUnder } from './route-tabs'

const TABS = ['models', 'pricing', 'fitness'] as const

describe('tabFromPath', () => {
  it('reads the segment when it names a tab', () => {
    expect(tabFromPath('/models/pricing', '/models', TABS, 'models')).toBe('pricing')
    expect(tabFromPath('/models/fitness', '/models', TABS, 'models')).toBe('fitness')
  })

  it('falls back on the bare view path', () => {
    expect(tabFromPath('/models', '/models', TABS, 'models')).toBe('models')
  })

  it('falls back on a trailing slash', () => {
    expect(tabFromPath('/models/', '/models', TABS, 'models')).toBe('models')
  })

  it('falls back on an unknown segment — a stale or typo’d link lands somewhere real', () => {
    expect(tabFromPath('/models/nonsense', '/models', TABS, 'models')).toBe('models')
  })

  it('falls back on a deeper path rather than matching its first segment', () => {
    // `/models/pricing/extra` means something else, or nothing. Rendering the
    // pricing tab for it would be a quiet wrong answer.
    expect(tabFromPath('/models/pricing/extra', '/models', TABS, 'models')).toBe('models')
  })

  it('does not match a different view that shares a prefix', () => {
    // `/modelsomething` must not read as `/models` + a tab.
    expect(tabFromPath('/modelsomething/pricing', '/models', TABS, 'models')).toBe('models')
  })

  it('does not treat the view itself as a tab of another', () => {
    expect(tabFromPath('/admin/security', '/models', TABS, 'models')).toBe('models')
  })

  it('decodes a percent-encoded segment', () => {
    // App-provided settings tabs are keyed by slug and route through the same
    // helper, so a slug that needed encoding must still match.
    expect(tabFromPath('/settings/my%2Dapp', '/settings', ['profile', 'my-app'] as const, 'profile')).toBe('my-app')
  })

  it('is exact about case, because the tab ids are', () => {
    expect(tabFromPath('/models/Pricing', '/models', TABS, 'models')).toBe('models')
  })

  it('handles a root-level base', () => {
    expect(tabFromPath('/activity', '', ['inbox', 'activity'] as const, 'inbox')).toBe('activity')
  })
})

describe('pathId', () => {
  it('reads the id a path selects', () => {
    expect(pathId('/research/run-42', '/research')).toBe('run-42')
  })

  it('is null on the bare view — nothing selected is a real state', () => {
    expect(pathId('/research', '/research')).toBeNull()
    expect(pathId('/research/', '/research')).toBeNull()
  })

  it('does not read a deeper path as a selection', () => {
    expect(pathId('/research/run-42/extra', '/research')).toBeNull()
  })

  it('does not match a view that shares a prefix', () => {
    expect(pathId('/researchers/run-42', '/research')).toBeNull()
  })

  it('decodes an encoded id', () => {
    expect(pathId('/plan/a%2Fb', '/plan')).toBe('a/b')
  })

  it('takes ids as opaque — existence is the loading query’s question', () => {
    // Deliberately NOT validated as a uuid: an id may name a row that was
    // deleted, and the query that loads it already has a not-found story.
    expect(pathId('/plan/not-a-uuid', '/plan')).toBe('not-a-uuid')
  })
})

// ── isUnder: the guard that keeps a leaving view from navigating you back ────

describe('isUnder', () => {
  it('matches the view itself and everything beneath it', () => {
    expect(isUnder('/comms', '/comms')).toBe(true)
    expect(isUnder('/comms/channel/c-1', '/comms')).toBe(true)
    expect(isUnder('/knowledge/space-1/doc-2', '/knowledge')).toBe(true)
  })

  it('rejects other views, which is the whole point', () => {
    // The bug this guard exists for: a view that is leaving still runs its
    // effects against the NEXT view's pathname, and if those effects navigate,
    // a wrong answer here drags the user back. It reads as a broken nav rail —
    // the same item wants two or three clicks before it moves.
    expect(isUnder('/agents', '/comms')).toBe(false)
    expect(isUnder('/', '/comms')).toBe(false)
  })

  it('rejects prefix look-alikes, which a bare startsWith would accept', () => {
    expect(isUnder('/commsomething', '/comms')).toBe(false)
    expect(isUnder('/knowledgebase', '/knowledge')).toBe(false)
  })
})

// THE SIDEBAR RULE: a nav item is active when you are on it or anywhere nested
// inside it, and when two items both contain the path the MORE SPECIFIC one
// wins. Every case below was a real wrong highlight before `activeAmong`
// existed — the rail lighting two items, or none, or the wrong one.
describe('activeAmong', () => {
  const NAV = ['/', '/boards', '/comms', '/knowledge', '/x/notes', '/x/notes/manage']

  it('lights the item you are standing on', () => {
    expect(activeAmong('/comms', NAV)).toBe('/comms')
  })

  it('keeps a parent lit for anything nested inside it', () => {
    // The case the sidebar exists to get right: reading a task must not blank
    // the board it belongs to.
    expect(activeAmong('/boards/b1/t2', NAV)).toBe('/boards')
    expect(activeAmong('/knowledge/space/doc', NAV)).toBe('/knowledge')
  })

  it('gives a nested nav item priority over its own ancestor', () => {
    // Both contain the path. Lighting both said you were in two places, and
    // that is what the old `exactFor` special case was patching by hand.
    expect(activeAmong('/x/notes/manage', NAV)).toBe('/x/notes/manage')
    expect(activeAmong('/x/notes/manage/anything', NAV)).toBe('/x/notes/manage')
    expect(activeAmong('/x/notes', NAV)).toBe('/x/notes')
  })

  it('lets Home match only Home, with no exemption of its own', () => {
    // Under a bare `startsWith`, `/` is a prefix of every route and lit on
    // every view — the reason NavRail special-cased it. `isUnder` retires that
    // without anyone deciding to: it tests `base + '/'`, and for `/` that is
    // `//`, which no real path starts with. So Home matches itself and nothing
    // else, structurally rather than by exemption.
    expect(activeAmong('/', NAV)).toBe('/')
    expect(activeAmong('/boards', NAV)).toBe('/boards')
    // An undeclared route lights NOTHING, rather than falling back to Home.
    // Highlighting Home for a page that is not Home is the fuzzy behaviour this
    // whole change is removing.
    expect(activeAmong('/nothing-declared', NAV)).toBeNull()
  })

  it('rejects prefix look-alikes rather than lighting a neighbour', () => {
    expect(activeAmong('/boardsomething', ['/boards'])).toBeNull()
    expect(activeAmong('/x/notesx', ['/x/notes'])).toBeNull()
  })

  it('returns null when nothing contains the path', () => {
    expect(activeAmong('/settings', ['/boards', '/comms'])).toBeNull()
  })

  it('does not depend on the order the paths are declared in', () => {
    // The strip took the FIRST match in nav order while the rail computed its
    // own answer, so the two disagreed about where you were.
    const forwards = ['/x/notes', '/x/notes/manage']
    expect(activeAmong('/x/notes/manage', forwards)).toBe('/x/notes/manage')
    expect(activeAmong('/x/notes/manage', [...forwards].reverse())).toBe('/x/notes/manage')
  })
})
