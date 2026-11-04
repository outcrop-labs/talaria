import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { setAppContext, type AppContext } from './app-context'
import {
  appHref,
  appNavigate,
  matchRest,
  resolveAppHref,
  restSegments,
  surfaceAt,
  surfaceBase,
  surfaceRest,
  useAppRoute,
} from './app-route'

// Routes inside an app surface. What they pin: which surface a path shows
// (the route gate's own split, so a link and its grant agree), the rest a
// surface is handed, how a pattern matches it, and where a link or a
// navigation lands — relative to the surface it is made on.

const h = vi.hoisted(() => ({
  contexts: new Map<unknown, unknown>(),
  navigated: [] as Array<{ href: string; options?: { replace?: boolean } }>,
}))

vi.mock('svelte', async (importOriginal) => ({
  ...(await importOriginal<typeof import('svelte')>()),
  getContext: (key: unknown) => h.contexts.get(key),
  setContext: (key: unknown, value: unknown) => void h.contexts.set(key, value),
}))

// The real router reads `location` at load; appNavigate reaches it lazily.
vi.mock('@/router', () => ({
  navigateHref: async (href: string, options?: { replace?: boolean }) => void h.navigated.push({ href, options }),
}))

beforeEach(() => {
  h.contexts.clear()
  h.navigated.length = 0
})

afterEach(() => {
  vi.unstubAllGlobals()
})

const at = (pathname: string) => vi.stubGlobal('location', { pathname })

describe('surfaceBase', () => {
  it('names each surface\'s own address', () => {
    expect(surfaceBase('crm', 'work')).toBe('/x/crm')
    expect(surfaceBase('crm', 'manage')).toBe('/x/crm/manage')
    expect(surfaceBase('crm', 'settings')).toBe('/settings/app:crm')
  })
})

describe('surfaceAt', () => {
  it('gives every route below /x/<slug> to the work surface, and below /x/<slug>/manage to the manage surface', () => {
    expect(surfaceAt('/x/crm')).toEqual({ slug: 'crm', surface: 'work' })
    expect(surfaceAt('/x/crm/items/42')).toEqual({ slug: 'crm', surface: 'work' })
    expect(surfaceAt('/x/crm/managers')).toEqual({ slug: 'crm', surface: 'work' })
    expect(surfaceAt('/x/crm/manage')).toEqual({ slug: 'crm', surface: 'manage' })
    expect(surfaceAt('/x/crm/manage/fields/7')).toEqual({ slug: 'crm', surface: 'manage' })
  })

  it('matches as the router does — case-insensitively', () => {
    expect(surfaceAt('/x/crm/MANAGE/fields')).toEqual({ slug: 'crm', surface: 'manage' })
    expect(surfaceAt('/X/CRM/items')).toEqual({ slug: 'crm', surface: 'work' })
  })

  it('knows a settings tab, spelled either way', () => {
    expect(surfaceAt('/settings/app:crm')).toEqual({ slug: 'crm', surface: 'settings' })
    expect(surfaceAt('/settings/app%3Acrm')).toEqual({ slug: 'crm', surface: 'settings' })
    expect(surfaceAt('/settings/app:crm/deeper')).toBeNull()
  })

  it('is null off the app routes', () => {
    for (const p of ['/', '/boards', '/x', '/x/', '/settings', '/settings/profile']) expect(surfaceAt(p), p).toBeNull()
  })
})

describe('surfaceRest', () => {
  it('is the path below the surface, as the URL spells it', () => {
    expect(surfaceRest('/x/crm', 'crm', 'work')).toBe('')
    expect(surfaceRest('/x/crm/', 'crm', 'work')).toBe('')
    expect(surfaceRest('/x/crm/items/42', 'crm', 'work')).toBe('items/42')
    expect(surfaceRest('/x/crm/items/a%2Fb/', 'crm', 'work')).toBe('items/a%2Fb')
    expect(surfaceRest('/x/crm//items///42', 'crm', 'work')).toBe('items/42')
    expect(surfaceRest('/x/crm/manage', 'crm', 'manage')).toBe('')
    expect(surfaceRest('/x/crm/manage/fields/7', 'crm', 'manage')).toBe('fields/7')
    expect(surfaceRest('/x/crm/MANAGE/fields', 'crm', 'manage')).toBe('fields')
  })

  it('is empty for a path that shows another surface or another app', () => {
    // The beat between a navigation and the old surface's unmount.
    expect(surfaceRest('/x/crm/manage/fields', 'crm', 'work')).toBe('')
    expect(surfaceRest('/x/crm/items', 'crm', 'manage')).toBe('')
    expect(surfaceRest('/x/wiki/items', 'crm', 'work')).toBe('')
    expect(surfaceRest('/boards', 'crm', 'work')).toBe('')
  })

  it('is always empty on a settings tab, which has no routes below it', () => {
    expect(surfaceRest('/settings/app:crm', 'crm', 'settings')).toBe('')
  })
})

describe('restSegments', () => {
  it('decodes each segment once and keeps a malformed escape as spelled', () => {
    expect(restSegments('')).toEqual([])
    expect(restSegments('items/a%2Fb/%E2%9C%93')).toEqual(['items', 'a/b', '✓'])
    expect(restSegments('items/%zz')).toEqual(['items', '%zz'])
  })
})

describe('matchRest', () => {
  it('captures :params and compares literals exactly', () => {
    expect(matchRest('items/:id', 'items/42')).toEqual({ id: '42' })
    expect(matchRest('items/:id/notes/:note', 'items/42/notes/7')).toEqual({ id: '42', note: '7' })
    expect(matchRest('items/:id', 'items/a%2Fb')).toEqual({ id: 'a/b' })
    expect(matchRest('items/:id', 'Items/42')).toBeNull()
  })

  it('needs every segment, and no more', () => {
    expect(matchRest('items/:id', 'items')).toBeNull()
    expect(matchRest('items/:id', 'items/42/notes')).toBeNull()
    expect(matchRest('items', 'items/42')).toBeNull()
  })

  it('matches the surface\'s own address with an empty pattern', () => {
    expect(matchRest('', '')).toEqual({})
    expect(matchRest('', 'items')).toBeNull()
    expect(matchRest('/items/', 'items')).toEqual({})
  })

  it('takes whatever is left with a final *capture', () => {
    expect(matchRest('docs/*path', 'docs/a/b%20c')).toEqual({ path: 'a/b c' })
    expect(matchRest('docs/*path', 'docs')).toEqual({ path: '' })
    expect(matchRest('docs/*', 'docs/a/b')).toEqual({})
    expect(() => matchRest('*path/x', 'a/x')).toThrow(/must end the pattern/)
  })
})

describe('resolveAppHref', () => {
  const work = { slug: 'crm', surface: 'work' } as const
  const manage = { slug: 'crm', surface: 'manage' } as const
  const settings = { slug: 'crm', surface: 'settings' } as const

  it('resolves a path relative to the surface it is made on', () => {
    expect(resolveAppHref(work)).toBe('/x/crm')
    expect(resolveAppHref(work, 'items/42')).toBe('/x/crm/items/42')
    expect(resolveAppHref(work, '/items/42/')).toBe('/x/crm/items/42')
    expect(resolveAppHref(manage, 'fields')).toBe('/x/crm/manage/fields')
    expect(resolveAppHref(manage, '')).toBe('/x/crm/manage')
  })

  it('carries a query and a hash', () => {
    expect(resolveAppHref(work, 'items?sort=title')).toBe('/x/crm/items?sort=title')
    expect(resolveAppHref(work, '?sort=title')).toBe('/x/crm?sort=title')
    expect(resolveAppHref(work, 'items/42#notes')).toBe('/x/crm/items/42#notes')
  })

  it('links into the app\'s other surface on request', () => {
    expect(resolveAppHref(work, 'fields', { surface: 'manage' })).toBe('/x/crm/manage/fields')
    expect(resolveAppHref(manage, 'items/42', { surface: 'work' })).toBe('/x/crm/items/42')
    expect(resolveAppHref(settings, '', { surface: 'work' })).toBe('/x/crm')
  })

  it('keeps a settings tab to its own address', () => {
    expect(resolveAppHref(settings)).toBe('/settings/app:crm')
    expect(resolveAppHref(settings, '?section=keys')).toBe('/settings/app:crm?section=keys')
    expect(() => resolveAppHref(settings, 'fields')).toThrow(/no routes below it/)
    expect(() => resolveAppHref(settings, 'a/../fields')).toThrow(/no routes below it/)
    expect(resolveAppHref(settings, 'a/..')).toBe('/settings/app:crm')
    expect(() => resolveAppHref(settings, '..')).toThrow(/outside the settings surface/)
  })

  it('resolves dots, backslashes and control characters as a link does', () => {
    // What the browser writes to the address is what the gate checks, so the
    // href is that address — never a spelling the router would read apart.
    expect(resolveAppHref(work, 'items/42/../7')).toBe('/x/crm/items/7')
    expect(resolveAppHref(work, 'items/%2e%2e/7')).toBe('/x/crm/7')
    expect(resolveAppHref(work, './items/./42')).toBe('/x/crm/items/42')
    expect(resolveAppHref(work, 'items\\42')).toBe('/x/crm/items/42')
    expect(resolveAppHref(work, '\\items')).toBe('/x/crm/items')
    expect(resolveAppHref(work, 'ite\tms/4\n2')).toBe('/x/crm/items/42')
    expect(resolveAppHref(work, 'a\u0000b c')).toBe('/x/crm/a%00b%20c')
    expect(resolveAppHref(manage, 'fields/..')).toBe('/x/crm/manage')
    expect(resolveAppHref(work, 'items/42/..?tab=1#n')).toBe('/x/crm/items?tab=1#n')
  })

  it('refuses a path that resolves outside its surface', () => {
    // Each of these used to go into the href verbatim: the router mounted one
    // surface and the address — the gate's input — named another.
    for (const [from, path] of [
      [manage, '../items'],
      [manage, '%2e%2e/items'],
      [manage, '.%2E/items'],
      [manage, '..\\items'],
      [manage, 'fields/../../items'],
      [work, '..'],
      [work, '../other/items'],
      [work, '../../boards'],
      [work, 'manage'],
      [work, 'MANAGE/fields'],
      [work, 'items/../manage/fields'],
    ] as const) {
      expect(() => resolveAppHref(from, path), `${from.surface} ${path}`).toThrow(/outside the (work|manage) surface/)
    }
    // An escape that is not a dot is a name: it stays below the surface.
    expect(resolveAppHref(work, 'm%61nage')).toBe('/x/crm/m%61nage')
    expect(resolveAppHref(work, 'items/..%2Fmanage')).toBe('/x/crm/items/..%2Fmanage')
  })
})

describe('appHref and appNavigate', () => {
  it('resolve against the surface the address shows', async () => {
    at('/x/crm/items/42')
    expect(appHref()).toBe('/x/crm')
    expect(appHref('items/7')).toBe('/x/crm/items/7')
    at('/x/crm/manage/fields')
    expect(appHref('fields/new')).toBe('/x/crm/manage/fields/new')
    await appNavigate('fields', { replace: true })
    await appNavigate('items/9', { surface: 'work' })
    expect(h.navigated).toEqual([
      { href: '/x/crm/manage/fields', options: { replace: true } },
      { href: '/x/crm/items/9', options: { replace: undefined } },
    ])
  })

  it('refuse a path that leaves the surface, before navigating anywhere', async () => {
    at('/x/crm/items/42')
    expect(() => appHref('manage/fields')).toThrow(/outside the work surface/)
    await expect(appNavigate('../other')).rejects.toThrow(/outside the work surface/)
    at('/x/crm/manage/fields')
    await expect(appNavigate('../items')).rejects.toThrow(/outside the manage surface/)
    // The way across is to name the surface; its path is relative to it.
    await expect(appNavigate('../items', { surface: 'work' })).rejects.toThrow(/outside the work surface/)
    await appNavigate('items', { surface: 'work' })
    expect(h.navigated).toEqual([{ href: '/x/crm/items', options: { replace: undefined } }])
  })

  it('refuse off an app surface, before navigating anywhere', async () => {
    at('/boards')
    expect(() => appHref('items')).toThrow(/works on an app surface/)
    await expect(appNavigate('items')).rejects.toThrow(/works on an app surface/)
    expect(h.navigated).toEqual([])
  })
})

describe('useAppRoute', () => {
  /** A surface the host can move: its rest and query follow `go`. */
  function mountSurface(): { go: (rest: string, query?: string) => void } {
    let rest = ''
    let query = ''
    setAppContext({
      slug: 'crm',
      surface: 'work',
      manifest: {} as AppContext['manifest'],
      get rest() {
        return rest
      },
      get query() {
        return new URLSearchParams(query)
      },
    })
    return {
      go: (r, q = '') => {
        rest = r
        query = q
      },
    }
  }

  it('reads where the surface is, live', () => {
    const host = mountSurface()
    const route = useAppRoute()
    expect(route.rest).toBe('')
    expect(route.match('')).toEqual({})
    expect(route.match('items/:id')).toBeNull()

    host.go('items/a%2Fb', 'tab=notes')
    expect(route.rest).toBe('items/a%2Fb')
    expect(route.segments).toEqual(['items', 'a/b'])
    expect(route.match('items/:id')?.id).toBe('a/b')
    expect(route.query.get('tab')).toBe('notes')
  })

  it('refuses outside an app surface', () => {
    expect(() => useAppRoute()).toThrow(/inside an app surface/)
  })
})
