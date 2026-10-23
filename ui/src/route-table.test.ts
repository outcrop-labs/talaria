import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { beforeAll, describe, expect, it } from 'vitest'
import type { Routes } from 'sv-router'
import { appManageSurface, appWorkSurface, NotFound, routes, shell } from './route-table'
import { appViewFor } from './lib/app-grants'
import { resolveHref } from './lib/route-tabs'

// THE ROUTER AND THE ROUTE GATE MAKE ONE DECISION. The router picks the
// surface a path mounts; the gate (AppLayout → isViewDenied → appViewFor)
// picks the grant that path needs. If they read a path apart, a member
// holding one surface's grant sees another surface. This suite runs
// sv-router's own matcher — the function every navigation calls — over the
// real route table, and holds it to the gate's answer for the shared
// fixture's paths (api/tests/it/fixtures/app-grants.json, which Rust's
// app_view_for reads too) and for the spellings that once split them.
//
// sv-router's package exports only its entry, which builds a browser router
// on load, so the matcher is reached by file. It is pure.

const here = dirname(fileURLToPath(import.meta.url))
const MATCH_ROUTE = join(here, '..', 'node_modules', 'sv-router', 'src', 'helpers', 'match-route.js')
const FIXTURE = join(here, '..', '..', 'api', 'tests', 'it', 'fixtures', 'app-grants.json')

interface Matched {
  match: unknown
  layouts: unknown[]
  params: Record<string, string>
}
let matchRoute: (pathname: string, table: Routes) => Matched

beforeAll(async () => {
  ;({ matchRoute } = (await import(/* @vite-ignore */ MATCH_ROUTE)) as { matchRoute: typeof matchRoute })
})

const fixture = JSON.parse(readFileSync(FIXTURE, 'utf8')) as {
  routes: { reservedSlugs: string[]; paths: Array<{ path: string; view: string | null }> }
}

/** The app view of the surface the router mounts on `path`, or null when it
 *  mounts no app — a core view, Not Found, or an app surface handed a slug no
 *  app can have (bare /x, or an escaped slash), which AppSurface renders as
 *  "App not available". The slug is folded: the router hands the surface the
 *  slug as the path spells it (decoded), and folding only widens what this
 *  counts as mounted. */
function mountedView(path: string): string | null {
  const m = matchRoute(path, routes)
  if (m.match !== appWorkSurface && m.match !== appManageSurface) return null
  const slug = (m.params.app ?? '').toLowerCase()
  if (!/^[a-z0-9][a-z0-9-]*$/.test(slug)) return null
  // Every app surface wears the shell, whose effect is the gate.
  expect(m.layouts, `${path} mounts inside the shell`).toEqual([shell])
  return m.match === appManageSurface ? `/x/${slug}/manage` : `/x/${slug}`
}

/** That the gate holds `path` to the grant of the app surface it mounts.
 *  Where no app mounts, nothing is shown to gate. The one spelling where they
 *  differ fails closed: the router decodes an escaped slug (/x/cr%6D mounts
 *  crm) and the gate does not, so the gate asks for a view with a `%` in its
 *  slug — one no grant can name — and refuses every member. */
function gateMatchesRouter(path: string): void {
  const mounted = mountedView(path)
  const gate = appViewFor(path)
  if (mounted === null || gate === mounted) return
  expect(gate?.split('/')[2] ?? '', `${path}: the router mounts ${mounted}, the gate checks ${gate}`).toMatch(/%/)
}

describe('the route table', () => {
  it('mounts the surface the gate grants, for every path in the shared fixture', () => {
    expect(fixture.routes.paths.length).toBeGreaterThan(0)
    for (const c of fixture.routes.paths) {
      expect(mountedView(c.path), c.path).toBe(c.view)
      gateMatchesRouter(c.path)
    }
  })

  it('gives every route below a surface to that surface', () => {
    expect(mountedView('/x/crm')).toBe('/x/crm')
    expect(mountedView('/x/crm/a/b')).toBe('/x/crm')
    expect(mountedView('/x/crm/a/b/c/d/e')).toBe('/x/crm')
    expect(mountedView('/x/crm/managers/1')).toBe('/x/crm')
    expect(mountedView('/x/crm/manage')).toBe('/x/crm/manage')
    expect(mountedView('/x/crm/manage/a')).toBe('/x/crm/manage')
    expect(mountedView('/x/crm/manage/a/b/c')).toBe('/x/crm/manage')
    expect(mountedView('/x/crm/Manage/a')).toBe('/x/crm/manage')
  })

  it('opens an app whose slug is a router config key, and an app route named like one', () => {
    // A `layout` or `hooks` key beside the slug segment outranked '/:app', so
    // /x/hooks matched the key, found no route under it, and the matcher
    // threw: a blank page.
    for (const slug of fixture.routes.reservedSlugs) {
      for (const [path, view] of [
        [`/x/${slug}`, `/x/${slug}`],
        [`/x/${slug}/`, `/x/${slug}`],
        [`/x/${slug}/items/42`, `/x/${slug}`],
        [`/x/${slug}/${slug}`, `/x/${slug}`],
        [`/x/${slug}/manage`, `/x/${slug}/manage`],
        [`/x/${slug}/manage/${slug}/a`, `/x/${slug}/manage`],
        [`/x/crm/${slug}`, '/x/crm'],
        [`/x/crm/manage/${slug}`, '/x/crm/manage'],
      ] as const) {
        expect(() => matchRoute(path, routes), path).not.toThrow()
        expect(mountedView(path), path).toBe(view)
        gateMatchesRouter(path)
      }
    }
  })

  it('keeps every config key out of /x, and reserves each one as a slug', () => {
    // A config key the table grows anywhere becomes a slug install refuses;
    // one inside /x would shadow a slug or an app's route again.
    const configKeys = new Set<string>()
    const walk = (table: Record<string, unknown>, underX: boolean) => {
      for (const [key, value] of Object.entries(table)) {
        const route = key.startsWith('/') || key.startsWith('*') || key.startsWith('(')
        if (!route) {
          configKeys.add(key)
          expect(underX, `config key '${key}' inside /x`).toBe(false)
        }
        if (route && value && typeof value === 'object') walk(value as Record<string, unknown>, underX || key === '/x')
      }
    }
    walk(routes, false)
    expect(configKeys.size).toBeGreaterThan(0)
    for (const key of configKeys) expect(fixture.routes.reservedSlugs, key).toContain(key)
  })

  it('reads a spelling the same way the gate does, or fails closed', () => {
    for (const path of [
      '/x/crm//manage',
      '/x/crm/manage//a',
      '/x/CRM/MANAGE/a',
      '/x/crm/m%61nage/a',
      '/x/crm/%6Danage',
      '/x/cr%6D/manage/a',
      '/x/cr%6D/items',
      '/x/crm%2Fmanage/a',
      '/x/crm/%zz/manage',
      '/x//crm/manage',
      '/x',
      '/x/',
    ]) {
      expect(() => matchRoute(path, routes), path).not.toThrow()
      gateMatchesRouter(path)
    }
  })

  it('renders Not Found bare, and keeps the public pages out of the shell', () => {
    for (const path of ['/nope', '/x-ray', '/teams/t1', '/a/s/extra', '/login/extra', '/kb/space/s/extra', '/boards/b/t/extra']) {
      const m = matchRoute(path, routes)
      expect(m.match, path).toBe(NotFound)
      expect(m.layouts, path).toEqual([])
    }
    expect(matchRoute('/a/s', routes).layouts).toEqual([])
    expect(matchRoute('/login', routes).layouts).toEqual([])
    expect(matchRoute('/boards', routes).layouts[0]).toBe(shell)
    expect(matchRoute('/', routes).layouts).toEqual([shell])
  })
})

describe('navigation', () => {
  // The router matches the path navigate is handed and writes it to the
  // address through the URL parser; the gate reads the address. Resolved
  // first (navigateHref → resolveHref), they read one path.
  const here = 'https://talaria.test/x/crm/items/42'
  const spellings = [
    '/x/crm/manage/../items',
    '/x/crm/manage/%2e%2e/items',
    '/x/crm/manage/.%2E/items',
    '/x/crm/items/../manage/fields',
    '/x/crm/items/%2e%2e/manage',
    '/x/crm/manage/./a',
    '/x/crm\\manage\\a',
    '/x/crm/man\tage/a',
    '/x/crm/man\nage',
    '/x/crm/manage/a\u0000b',
    '/x/hooks/manage/../items',
    '/x/crm/../hooks/manage',
  ]

  it('lands where the gate looks', () => {
    for (const href of spellings) {
      const { pathname } = resolveHref(href, here)
      expect(new URL(pathname, here).pathname, `${href} resolves to a path the address keeps as is`).toBe(pathname)
      gateMatchesRouter(pathname)
    }
  })

  it('would not, unresolved — which is why navigateHref resolves', () => {
    // The raw spelling mounts one surface while the address it leaves names
    // another: a work grant would have shown the manage surface.
    const raw = '/x/crm/items/../manage/fields'
    expect(mountedView(raw)).toBe('/x/crm')
    expect(appViewFor(new URL(raw, here).pathname)).toBe('/x/crm/manage')
    const back = '/x/crm/manage/../items'
    expect(mountedView(back)).toBe('/x/crm/manage')
    expect(appViewFor(new URL(back, here).pathname)).toBe('/x/crm')
  })
})
