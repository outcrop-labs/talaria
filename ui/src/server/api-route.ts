// API route contract. Each file in src/routes/api/** exports
//
//   export const Route = defineApi('/api/boards/$id/tasks', {
//     GET: async ({ request, params }) => json({ ... }),
//   })
//
// The path uses the same `$param` / trailing `$` (splat → params._splat)
// grammar the TanStack Start file routes used, so handler bodies port
// unchanged. src/server/app.ts collects every Route into one fetch handler.

export type ApiParams = Record<string, string> & { _splat?: string }

/** `$param` segment names in a path literal — the trailing splat `$` (its
 *  "name" is the empty string) is excluded; it surfaces as `_splat` instead. */
type ParamNames<Path extends string> = Path extends `${infer Head}/${infer Rest}`
  ? ParamNames<Head> | ParamNames<Rest>
  : Path extends `$${infer Name}`
    ? Name extends ''
      ? never
      : Name
    : never

/** Params as typed by the path: `'/api/boards/$id'` → `{ id: string }`, plus
 *  `_splat` when the path ends in `$`. Declared properties (not an index
 *  signature) so `params.id` is `string`, not `string | undefined` under
 *  noUncheckedIndexedAccess — the matcher only calls a handler once every
 *  `$param` segment matched, so the values are always present. */
export type PathParams<Path extends string> = { [K in ParamNames<Path>]: string } & { _splat?: string }

export interface ApiContext<P = ApiParams> {
  request: Request
  params: P
}

export type ApiHandler<P = ApiParams> = (ctx: ApiContext<P>) => Response | Promise<Response>

export type ApiMethod = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE' | 'HEAD' | 'OPTIONS'

export interface ApiRoute {
  path: string
  handlers: Partial<Record<ApiMethod, ApiHandler>>
}

export function defineApi<Path extends string>(
  path: Path,
  handlers: Partial<Record<ApiMethod, ApiHandler<PathParams<Path>>>>,
): ApiRoute {
  // The cast only widens the compile-time view back to the runtime shape
  // (Record<string, string>); matchRoute always supplies every `$param`.
  return { path, handlers: handlers as unknown as ApiRoute['handlers'] }
}

interface CompiledRoute extends ApiRoute {
  segments: string[]
  /** Trailing `$`: match any deeper path, exposing the rest as params._splat. */
  splat: boolean
  /** Static segment count — more specific routes win over param routes. */
  staticCount: number
}

export function compileRoute(route: ApiRoute): CompiledRoute {
  let segments = route.path.split('/').filter(Boolean)
  let splat = false
  if (segments.at(-1) === '$') {
    splat = true
    segments = segments.slice(0, -1)
  }
  const staticCount = segments.filter((s) => !s.startsWith('$')).length
  return { ...route, segments, splat, staticCount }
}

/** Match a pathname against a compiled route; null when it doesn't apply. */
export function matchRoute(route: CompiledRoute, pathname: string): ApiParams | null {
  const parts = pathname.split('/').filter(Boolean)
  if (route.splat ? parts.length < route.segments.length : parts.length !== route.segments.length) {
    return null
  }
  const params: ApiParams = {}
  for (let i = 0; i < route.segments.length; i++) {
    const seg = route.segments[i]!
    const part = parts[i]!
    if (seg.startsWith('$')) params[seg.slice(1)] = decodeURIComponent(part)
    else if (seg !== part) return null
  }
  if (route.splat) {
    params._splat = parts.slice(route.segments.length).map(decodeURIComponent).join('/')
  }
  return params
}
