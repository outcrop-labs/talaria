// The fallback is the whole point of this helper: six views share it, and six
// hand-rolled versions would give six answers to "what does /models/nonsense
// do". These pin the one answer — the home tab, always something real.
import { describe, expect, it } from 'vitest'
import { activeAmong, pathId, resolveHref, tabFromPath, isUnder } from './route-tabs'

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

describe('resolveHref', () => {
  const here = 'https://talaria.test/x/crm/items/42?tab=notes'
  const parts = (href: string) => resolveHref(href, here)

  it('hands the router a path, with the query and hash beside it', () => {
    // navigate('/artifacts?a=1') used to match `artifacts?a=1` as the last
    // segment: Not Found, and `%3F` in the address.
    expect(parts('/artifacts?a=1')).toEqual({ pathname: '/artifacts', search: '?a=1', hash: '' })
    expect(parts('/x/crm/items/42?tab=notes#n3')).toEqual({ pathname: '/x/crm/items/42', search: '?tab=notes', hash: '#n3' })
    expect(parts('/knowledge#top')).toEqual({ pathname: '/knowledge', search: '', hash: '#top' })
    expect(parts('/inbox')).toEqual({ pathname: '/inbox', search: '', hash: '' })
    // A `?` after the hash belongs to the hash.
    expect(parts('/a#b?c')).toEqual({ pathname: '/a', search: '', hash: '#b?c' })
  })

  it('resolves the path as the browser will write it to the address', () => {
    // The router matched these raw and the address showed them resolved, so
    // the manage surface mounted at an address the gate calls the work view.
    const path = (href: string) => parts(href).pathname
    expect(path('/x/crm/manage/../items')).toBe('/x/crm/items')
    expect(path('/x/crm/manage/%2e%2e/items')).toBe('/x/crm/items')
    expect(path('/x/crm/manage/%2E./items')).toBe('/x/crm/items')
    expect(path('/x/crm/items/../manage/fields')).toBe('/x/crm/manage/fields')
    expect(path('/x/crm/./manage/./a')).toBe('/x/crm/manage/a')
    expect(path('/x/crm/manage/%2e')).toBe('/x/crm/manage/')
    expect(path('/x/../../boards')).toBe('/boards')
    // A backslash is a slash in an http(s) URL.
    expect(path('/x/crm\\manage\\a')).toBe('/x/crm/manage/a')
    expect(path('\\x\\crm')).toBe('/x/crm')
    // Tabs and newlines vanish; other controls and spaces are encoded.
    expect(path('/x/crm/man\tage/a')).toBe('/x/crm/manage/a')
    expect(path('/x/crm/man\nage')).toBe('/x/crm/manage')
    expect(path('/x/crm/a\u0000b c')).toBe('/x/crm/a%00b%20c')
    expect(path('  /x/crm/manage\u0001')).toBe('/x/crm/manage')
    // An escape that is not a dot stays as spelled; the router decodes it.
    expect(path('/x/crm/m%61nage/%2Fa')).toBe('/x/crm/m%61nage/%2Fa')
  })

  it('resolves a relative href against the current address, as a link would', () => {
    expect(parts('7')).toEqual({ pathname: '/x/crm/items/7', search: '', hash: '' })
    expect(parts('../manage')).toEqual({ pathname: '/x/crm/manage', search: '', hash: '' })
    expect(parts('?tab=log')).toEqual({ pathname: '/x/crm/items/42', search: '?tab=log', hash: '' })
  })

  it('takes a same-origin URL by its path, and refuses another origin', () => {
    expect(parts('https://talaria.test/boards/b1?t=2')).toEqual({ pathname: '/boards/b1', search: '?t=2', hash: '' })
    expect(() => parts('https://evil.test/x/crm')).toThrow(/not a route of this app/)
    expect(() => parts('//evil.test/x/crm')).toThrow(/not a route of this app/)
    expect(() => parts('/\\evil.test/x')).toThrow(/not a route of this app/)
    expect(() => parts('javascript:alert(1)')).toThrow(/not a route of this app/)
  })
})
