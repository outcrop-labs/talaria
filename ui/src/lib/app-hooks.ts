// The SDK's hooks for the app a surface belongs to: which app it is, its
// bound verbs, its reads, and their invalidation.
//
// Each hook reads the surface's slug ONCE, at init, and keeps it. A mounted
// surface never changes app — AppSurface remounts its child under a
// `${slug}:${surface}` key — so a live read would gain nothing, and it would
// let a handler still running in a torn-down surface (`await api.post(…);
// await api.post(…)` across a /x/a → /x/b navigation) send its next call to
// the app that replaced it. Kept apart from ui/src/sdk/index.ts, like
// app-client.ts, so it can be unit tested without importing the whole UI kit
// (app-hooks.test.ts).
import { createQuery, useQueryClient, type CreateQueryOptions, type CreateQueryResult } from '@tanstack/svelte-query'
import { getJson } from '@/lib/fetch-json'
import { getAppContext, type AppContext } from '@/lib/app-context'
import { appApi, appQueryKey, appRouteUrl, type AppApi } from '@/lib/app-client'

/** The app a surface belongs to: `{ slug, surface, manifest }`, as THIS
 *  instance installed it. Call during component init, inside a surface.
 *  The slug is the installed one, which need not be the name the app was
 *  scaffolded with — never hard-code it. */
export function useApp(): AppContext {
  const app = getAppContext()
  if (!app) throw new Error('useApp() must run during the init of a component inside an app surface')
  return app
}

/** The slug a hook binds to: the explicit one, or the surrounding surface's,
 *  read now — at init, where context is readable — and kept. */
function hookSlug(slug: string | undefined, hook: string): string {
  if (slug) return slug
  const app = getAppContext()
  if (!app) throw new Error(`${hook}() outside an app surface needs the app's slug as its first argument`)
  return app.slug
}

/** `appApi` for the surface this component is in — no slug to spell. Call
 *  during component init; the verbs it returns are safe anywhere, and stay
 *  bound to this surface's app. */
export function useAppApi(slug?: string): AppApi {
  return appApi(hookSlug(slug, 'useAppApi'))
}

/** What `useAppQuery` passes through to svelte-query: `enabled` (false holds
 *  the read), `select` (derive what the component reads), `staleTime`,
 *  `gcTime`, `refetchInterval`, `refetchOnWindowFocus`, `placeholderData`
 *  (`keepPreviousData` for a page flip) and `retry`. */
export type AppQueryOptions<T, TData = T> = Pick<
  CreateQueryOptions<T, Error, TData>,
  'enabled' | 'select' | 'staleTime' | 'gcTime' | 'refetchInterval' | 'refetchOnWindowFocus' | 'placeholderData' | 'retry'
>

type PathArg = string | (() => string)
type OptionsArg<T, TData> = AppQueryOptions<T, TData> | (() => AppQueryOptions<T, TData>)

/** Svelte-query read of an app-server GET — the common read path.
 *
 *  `useAppQuery(path, options?)` inside a surface; `useAppQuery(slug, path,
 *  options?)` to read another app's routes, or from a component outside any
 *  surface. `path` may be a getter (`() => string`) so a path built from
 *  reactive state re-keys the query as that state changes; `options` may be
 *  a getter too, for a reactive `enabled`. A getter that returns a string is
 *  a path. Call during component init. */
export function useAppQuery<T = unknown, TData = T>(
  ...args:
    | [path: PathArg, options?: OptionsArg<T, TData>]
    | [slug: string, path: PathArg, options?: OptionsArg<T, TData>]
): CreateQueryResult<TData, Error> {
  const [a, b, c] = args as [PathArg, (PathArg | OptionsArg<T, TData>)?, OptionsArg<T, TData>?]
  // Context is readable only now, at init: the surface's slug is read once.
  const surfaceSlug = getAppContext()?.slug
  // (slug, path[, options]) when the second argument is a path — a string,
  // or a getter returning one. Otherwise (path[, options]), in a surface.
  const resolve = () => {
    const second = typeof b === 'function' ? (b as () => unknown)() : b
    if (c !== undefined || typeof second === 'string') return { slug: a as string, path: second as string, options: c }
    const path = typeof a === 'function' ? a() : a
    return { slug: surfaceSlug, path, options: second as OptionsArg<T, TData> | undefined }
  }
  const noSlug = 'useAppQuery(path) outside an app surface needs the slug first: useAppQuery(slug, path)'
  if (surfaceSlug === undefined && resolve().slug === undefined) throw new Error(noSlug)
  return createQuery<T, Error, TData>(() => {
    const { slug, path, options: given } = resolve()
    if (slug === undefined) throw new Error(noSlug)
    const options = typeof given === 'function' ? given() : given
    const url = appRouteUrl(slug, path)
    return {
      ...options,
      queryKey: appQueryKey(slug, path),
      queryFn: ({ signal }) => getJson<T>(url, { signal }),
    }
  })
}

/** Invalidate an app's queries after a write: `invalidate()` for all of
 *  them, `invalidate('things')` for every read under `things` — its search
 *  variants and the records below it included. Call during component init
 *  (it grabs the query client from context); the returned function is safe
 *  anywhere. The slug is optional inside a surface. */
export function useAppInvalidate(slug?: string): (path?: string) => Promise<void> {
  const qc = useQueryClient()
  const app = hookSlug(slug, 'useAppInvalidate')
  return (path?: string) => qc.invalidateQueries({ queryKey: path ? appQueryKey(app, path) : ['app', app] })
}
