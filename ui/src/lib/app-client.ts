// The SDK's HTTP layer: `api()` for any platform route, the verbs bound to one
// app's server routes, and the query key an app-server read lives under.
//
// Built on the host's own door (fetch-json.ts), not beside it. The SDK used to
// carry a weaker copy — no status on the error, no deadline on a read, a raw
// SyntaxError for a 2xx that wasn't JSON, nothing for streams — on the belief
// that the published SDK could not import the app's door. It can: the SDK runs
// from the host bundle. Kept apart from ui/src/sdk/index.ts so it can be unit
// tested without importing the whole UI kit (app-client.test.ts).
import { delJson, getJson, patchJson, postJson, putJson } from '@/lib/fetch-json'

/** What an app's bound verbs take after the path and body: a `RequestInit`
 *  plus a deadline. */
export interface AppApiInit extends RequestInit {
  /** Abort after this many milliseconds. Reads already stop at 30s; a write
   *  has no deadline unless you give it this or a `signal`. */
  timeout?: number
}

/** Options for `api()`: the same, plus a JSON body. */
export interface ApiInit extends AppApiInit {
  /** Sent as the JSON body, with the content-type set for you. Use `body`
   *  instead for anything that isn't JSON (FormData, a Blob, text). */
  json?: unknown
}

/** `timeout` becomes a signal, combined with the caller's own. */
function withDeadline(init: AppApiInit | undefined): RequestInit | undefined {
  if (!init) return init
  const { timeout, ...rest } = init
  if (timeout === undefined) return rest
  const deadline = AbortSignal.timeout(timeout)
  return { ...rest, signal: rest.signal ? AbortSignal.any([rest.signal, deadline]) : deadline }
}

const headerRecord = (h: HeadersInit): Record<string, string> => Object.fromEntries(new Headers(h).entries())

/** The door spreads a caller's `init` after its own content-type header, so
 *  caller headers would drop it; restate it whenever a JSON body travels with
 *  them. */
function keepJsonType(init: RequestInit | undefined, body: unknown): RequestInit | undefined {
  if (!init?.headers || body === undefined || body instanceof FormData) return init
  return { ...init, headers: { 'content-type': 'application/json', ...headerRecord(init.headers) } }
}

/** JSON fetch against any platform API (runs as the signed-in user). Rejects
 *  with an `HttpError` (`.status`, the server's sentence as `.message`) on any
 *  non-2xx, and with a sentence that says so on a 2xx that isn't JSON. */
export async function api<T = unknown>(path: string, init?: ApiInit): Promise<T> {
  const { json, ...rest } = init ?? {}
  const method = (rest.method ?? 'GET').toUpperCase()
  // A raw `body` rides in `init`, which the door spreads last, so it passes
  // through untouched; a `json` body is the door's to encode.
  const req: RequestInit = { ...keepJsonType(withDeadline(rest), json), method }
  if (json !== undefined) delete req.body
  switch (method) {
    case 'GET':
      return getJson<T>(path, req)
    case 'POST':
      return postJson<T>(path, json, req)
    case 'PUT':
      return putJson<T>(path, json, req)
    case 'PATCH':
      return patchJson<T>(path, json, req)
    case 'DELETE':
      return delJson<T>(path, json, req)
    default:
      throw new Error(`api(): unsupported method ${method}`)
  }
}

/** The verbs bound to one app's server routes (/api/apps/<slug>/…). Each
 *  takes the same `{ timeout, signal, headers, … }` last, as `api()` does. */
export interface AppApi {
  get: <T = unknown>(path: string, init?: AppApiInit) => Promise<T>
  post: <T = unknown>(path: string, body?: unknown, init?: AppApiInit) => Promise<T>
  put: <T = unknown>(path: string, body?: unknown, init?: AppApiInit) => Promise<T>
  patch: <T = unknown>(path: string, body?: unknown, init?: AppApiInit) => Promise<T>
  del: <T = unknown>(path: string, body?: unknown, init?: AppApiInit) => Promise<T>
  /** The absolute URL of a route, for `getStream`, `postStream` or `openStream`. */
  url: (path: string) => string
}

export const appRouteUrl = (slug: string, path: string): string => `/api/apps/${slug}/${path.replace(/^\/+/, '')}`

/** Fetch helpers bound to an app's server routes (/api/apps/<slug>/…), for
 *  code outside a component. Inside a surface, use `useAppApi()`. */
export function appApi(slug: string): AppApi {
  const url = (path: string) => appRouteUrl(slug, path)
  const send = (body: unknown, init: AppApiInit | undefined) => keepJsonType(withDeadline(init), body)
  return {
    get: (path, init) => getJson(url(path), withDeadline(init)),
    post: (path, body, init) => postJson(url(path), body ?? {}, send(body ?? {}, init)),
    put: (path, body, init) => putJson(url(path), body ?? {}, send(body ?? {}, init)),
    patch: (path, body, init) => patchJson(url(path), body ?? {}, send(body ?? {}, init)),
    del: (path, body, init) => delJson(url(path), body, send(body, init)),
    url,
  }
}

/** The query key an app-server path reads under: the app, the path's
 *  segments, then its search params. Built from structure, not the raw
 *  string, so a prefix of segments matches every query below it —
 *  `invalidate('things')` reaches `things?q=ada` and `things/123`. */
export function appQueryKey(slug: string, path: string): readonly unknown[] {
  const at = path.indexOf('?')
  const pathname = at < 0 ? path : path.slice(0, at)
  const search = at < 0 ? '' : path.slice(at + 1)
  const key: unknown[] = ['app', slug, ...pathname.split('/').filter(Boolean)]
  if (search) {
    const params: Record<string, string | string[]> = {}
    for (const [k, v] of new URLSearchParams(search)) {
      const had = params[k]
      params[k] = had === undefined ? v : Array.isArray(had) ? [...had, v] : [had, v]
    }
    key.push(params)
  }
  return key
}
