// Routes inside an app surface: which surface a path shows, the path below
// it, and the links and navigation an app makes within itself.
//
// An app owns every route below its surfaces. /x/<slug>/<rest> renders the
// work surface and /x/<slug>/manage/<rest> the manage surface (router.ts
// mounts each once for any <rest>); a settings tab, /settings/app:<slug>, is
// one page with nothing below it. The split is the route gate's own
// (appViewFor in app-grants.ts), so where a link lands and who may open it
// are one decision.
//
// NOTHING HERE IMPORTS THE ROUTER AT LOAD. sv-router reads `location` when its
// module loads, and the dev build probe loads an app's whole import graph —
// the SDK, so this file — where there is no `location`. AppSurface (host code)
// reads the route and hands the surface its rest and query through the app
// context; appNavigate loads the router the first time it is called. Kept
// apart from ui/src/sdk/index.ts, like app-hooks.ts, so the pure parts are
// unit tested without the UI kit (app-route.test.ts).
import { appManageView, appViewFor, appWorkView, asciiLower } from '@/lib/app-grants'
import { getAppContext, type AppSurfaceName } from '@/lib/app-context'

/** Where one surface of one app lives: `/x/<slug>`, `/x/<slug>/manage`, or
 *  the settings tab `/settings/app:<slug>`. */
export function surfaceBase(slug: string, surface: AppSurfaceName): string {
  if (surface === 'settings') return `/settings/app:${slug}`
  return surface === 'manage' ? appManageView(slug) : appWorkView(slug)
}

/** Which app surface `pathname` shows, or null off the app routes. Matches
 *  as the router does: ASCII-case-insensitively, so the slug comes back
 *  folded. */
export function surfaceAt(pathname: string): { slug: string; surface: AppSurfaceName } | null {
  const view = appViewFor(pathname)
  if (view !== null) {
    const [, , slug = '', manage] = view.split('/')
    return { slug, surface: manage ? 'manage' : 'work' }
  }
  const tab = /^\/settings\/app(?::|%3a)([^/]+)\/?$/.exec(asciiLower(pathname))
  return tab?.[1] ? { slug: tab[1], surface: 'settings' } : null
}

/** The path below one surface of one app on `pathname`: '' at the surface's
 *  own address, `items/42` on a route below it — as the URL spells it (still
 *  percent-encoded), with no leading, trailing or doubled slash. '' when the
 *  path shows a different surface or app, and always on a settings tab,
 *  which has no routes below it. */
export function surfaceRest(pathname: string, slug: string, surface: AppSurfaceName): string {
  if (surface === 'settings') return ''
  const at = surfaceAt(pathname)
  if (!at || at.slug !== asciiLower(slug) || at.surface !== surface) return ''
  return pathname.slice(surfaceBase(slug, surface).length).split('/').filter(Boolean).join('/')
}

/** Decode one path segment, keeping a malformed escape as it was spelled
 *  rather than throwing. */
function decodeSegment(s: string): string {
  try {
    return decodeURIComponent(s)
  } catch {
    return s
  }
}

/** A rest's segments, each decoded once: `items/a%2Fb` → ['items', 'a/b']. */
export const restSegments = (rest: string): string[] => rest.split('/').filter(Boolean).map(decodeSegment)

/** Match a rest against a pattern of literal segments and `:name` captures,
 *  optionally ending in `*name` for whatever is left: `items/:id` matches
 *  `items/42` as `{ id: '42' }`; `docs/*path` matches `docs/a/b` as
 *  `{ path: 'a/b' }`. Captures are decoded; literals compare exactly. The
 *  params, or null when the rest is not that route. `''` matches the
 *  surface's own address. */
export function matchRest(pattern: string, rest: string): Record<string, string> | null {
  const want = pattern.split('/').filter(Boolean)
  const have = rest.split('/').filter(Boolean)
  const params: Record<string, string> = {}
  for (let i = 0; i < want.length; i++) {
    const part = want[i]!
    if (part.startsWith('*')) {
      if (i !== want.length - 1) throw new Error(`a *capture must end the pattern: '${pattern}'`)
      const tail = have.slice(i).map(decodeSegment).join('/')
      if (part.length > 1) params[part.slice(1)] = tail
      return params
    }
    const seg = have[i]
    if (seg === undefined) return null
    if (part.startsWith(':')) params[part.slice(1)] = decodeSegment(seg)
    else if (part !== seg) return null
  }
  return have.length === want.length ? params : null
}

/** Link options: which of the app's own surfaces the link opens. */
export interface AppHrefOptions {
  /** Link into another surface of the same app — `'work'` or `'manage'`.
   *  Defaults to the surface the call is made from. A settings tab has no
   *  routes below it, so from one, a path needs a surface. A path never
   *  climbs out of its surface with `..`; this is how you leave it. */
  surface?: 'work' | 'manage'
}

/** Navigation options: the link options, plus whether to replace the
 *  current history entry rather than push one (a redirect, a filter you
 *  don't want Back to step through). */
export interface AppNavigateOptions extends AppHrefOptions {
  replace?: boolean
}

/** Any origin will do: `resolveAppHref` reads only the path the URL parser
 *  makes of a host path, and the host path always starts with '/'. */
const PARSE_BASE = 'http://app.invalid'

/** The host path for `path` below a surface: `items/42?tab=notes` on the
 *  work surface of `crm` is `/x/crm/items/42?tab=notes`. `path` is relative
 *  to the surface (a leading slash is ignored) and may carry a query and a
 *  hash; its segments go into the URL as given, so encode an id that may
 *  hold a slash (`encodeURIComponent(id)`). Pure: `from` is the surface the
 *  link is made on.
 *
 *  RESOLVED AS A LINK IS. `..` and `.` (spelled `%2e` too), a backslash, a
 *  tab or newline, and a control character come out as the browser's URL
 *  parser makes them — the same parser that writes the address the route gate
 *  checks — and empty segments collapse. A path that resolves outside its
 *  surface throws rather than land somewhere the caller did not name:
 *  `'../items'` from the manage surface, or `'manage/fields'` from the work
 *  surface, which is the manage surface's address. Name the other surface
 *  with `{ surface }` instead. */
export function resolveAppHref(from: { slug: string; surface: AppSurfaceName }, path = '', opts: AppHrefOptions = {}): string {
  const surface = opts.surface ?? from.surface
  const base = surfaceBase(from.slug, surface)
  const url = new URL(`${base}/${path.replace(/^[/\\]+/, '')}`, PARSE_BASE)
  const pathname = `/${url.pathname.split('/').filter(Boolean).join('/')}`
  const tail = `${url.search}${url.hash}`
  if (surface === 'settings') {
    if (pathname === base) return `${pathname}${tail}`
    if (pathname.startsWith(`${base}/`)) {
      const sub = pathname.slice(base.length + 1)
      throw new Error(
        `a settings tab has no routes below it — link to '${sub}' on another surface: appHref('${sub}', { surface: 'manage' })`,
      )
    }
  } else {
    const lands = surfaceAt(pathname)
    if (lands && lands.slug === asciiLower(from.slug) && lands.surface === surface) return `${pathname}${tail}`
  }
  throw new Error(
    `'${path}' resolves to ${pathname}, outside the ${surface} surface (${base}) — an app path stays below its surface; name another surface of the app with { surface }`,
  )
}

/** The surface the page shows now, for the link helpers. They run from
 *  templates and handlers, where Svelte context cannot be read, so they ask
 *  the address — which names the surface exactly as the router mounted it. */
function currentSurface(fn: string): { slug: string; surface: AppSurfaceName } {
  const at = typeof location === 'undefined' ? null : surfaceAt(location.pathname)
  if (!at) throw new Error(`${fn}() works on an app surface — /x/<slug>, /x/<slug>/manage or its settings tab`)
  return at
}

/** The href for a route of this app, relative to the surface the page
 *  shows: `appHref('items/42')` on `/x/crm/…` is `/x/crm/items/42`, and on
 *  `/x/crm/manage/…` it is `/x/crm/manage/items/42`. `appHref()` is the
 *  surface's own address. `..` resolves as in a link and may not leave the
 *  surface (resolveAppHref). An `<a href>` built from it navigates in place —
 *  the host router takes the click — and still opens in a new tab. */
export function appHref(path = '', opts?: AppHrefOptions): string {
  return resolveAppHref(currentSurface('appHref'), path, opts)
}

/** Navigate to a route of this app, relative to the surface the page shows
 *  — `appNavigate('items/42')`, `appNavigate('', { replace: true })`. Back
 *  and Forward step through these like any other address. Resolves once the
 *  new route has rendered. */
export async function appNavigate(path = '', opts: AppNavigateOptions = {}): Promise<void> {
  const href = appHref(path, opts)
  const { navigateHref } = await import('@/router')
  await navigateHref(href, { replace: opts.replace })
}

/** The capture names in a route pattern: `items/:id/*path` → 'id' | 'path'. */
type Captures<P extends string> = P extends `${infer Head}/${infer Tail}`
  ? Captures<Head> | Captures<Tail>
  : P extends `:${infer Name}`
    ? Name
    : P extends `*${infer Name}`
      ? Name extends ''
        ? never
        : Name
      : never

/** The params a pattern captures, by name — `RouteParams<'items/:id'>` is
 *  `{ id: string }`. A pattern that is not a literal gets a string record. */
export type RouteParams<P extends string> = string extends P ? Record<string, string> : { [K in Captures<P>]: string }

/** Where a surface is: the route below it and the query, both live. */
export interface AppRoute {
  /** The path below the surface: '' at its own address, `items/42` on a
   *  route below it — as the URL spells it, with no leading slash. Always ''
   *  on a settings tab. */
  readonly rest: string
  /** `rest` split into segments, each decoded once. */
  readonly segments: string[]
  /** The address's query, parsed. A fresh copy on each read: change the URL
   *  with `appNavigate`, not by editing this. */
  readonly query: URLSearchParams
  /** The params of `rest` against a pattern — `match('items/:id')` is
   *  `{ id: '42' }` on `items/42`, and null on any other route. Literal
   *  segments compare exactly, `:name` captures one segment, and a final
   *  `*name` captures the rest; captures are decoded. `match('')` is the
   *  surface's own address. */
  match<P extends string>(pattern: P): RouteParams<P> | null
}

/** The route of the surface this component renders in, live: read
 *  `route.rest`, `route.match(…)` and `route.query` in the template or
 *  inside `$derived`, and they follow every navigation, Back and Forward
 *  included. Call during component init, inside an app surface. */
export function useAppRoute(): AppRoute {
  const app = getAppContext()
  if (!app) throw new Error('useAppRoute() must run during the init of a component inside an app surface')
  return {
    get rest() {
      return app.rest
    },
    get segments() {
      return restSegments(app.rest)
    },
    get query() {
      return app.query
    },
    match: <P extends string>(pattern: P) => matchRest(pattern, app.rest) as RouteParams<P> | null,
  }
}
