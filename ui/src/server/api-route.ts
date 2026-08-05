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

export interface ApiContext {
  request: Request
  params: ApiParams
}

export type ApiHandler = (ctx: ApiContext) => Response | Promise<Response>

export type ApiMethod = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE' | 'HEAD' | 'OPTIONS'

export interface ApiRoute {
  path: string
  handlers: Partial<Record<ApiMethod, ApiHandler>>
}

export function defineApi(path: string, handlers: ApiRoute['handlers']): ApiRoute {
  return { path, handlers }
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
