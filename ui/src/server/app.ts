// The whole server, as one fetch handler: every src/routes/api/** route,
// matched and dispatched, with an SPA-shell fallback for page loads.
//
// Consumed two ways:
//   dev   vite.config.ts middleware ssrLoadModule()s this file per request
//   prod  vite.server.config.ts bundles it to dist/server/server.js, which
//         server-entry.js wraps in a Node http server (SSE pump, logging,
//         graceful shutdown, scheduler boot — all live there, not here)
//
// Importing every route eagerly is deliberate: the route graph is what pulls
// in the job modules, and server-entry.js's scheduler handshake (the
// well-known global symbol) depends on those modules loading with the graph.
import { readFile } from 'node:fs/promises'
import { compileRoute, matchRoute, type ApiMethod, type ApiRoute } from './api-route'
import { json } from './http'
import { maybeProxy } from './rust-proxy'

const modules = import.meta.glob<{ Route?: ApiRoute }>('../routes/api/**/*.ts', { eager: true })

const routes = Object.entries(modules)
  .flatMap(([file, mod]) => {
    if (!mod.Route?.path || !mod.Route.handlers) {
      if (!file.endsWith('.test.ts')) console.error(`[api] ${file} exports no Route — unreachable`)
      return []
    }
    return [compileRoute(mod.Route)]
  })
  // Static-heavy routes first so /api/boards/mine beats /api/boards/$id.
  .sort((a, b) => b.staticCount - a.staticCount || b.segments.length - a.segments.length)

// ── SPA shell ────────────────────────────────────────────────────────────────
// Any GET that isn't an API route gets index.html; the client router takes it
// from there. Read lazily (dist/client lives next to dist/server) and cached.
let shell: string | null = null
async function loadShell(): Promise<string | null> {
  if (shell !== null) return shell
  try {
    shell = await readFile(new URL('../client/index.html', import.meta.url), 'utf8')
  } catch {
    shell = null // dev: vite serves index.html itself, this path never runs
  }
  return shell
}

async function handle(request: Request): Promise<Response> {
  const url = new URL(request.url)
  const pathname = decodeURIComponent(url.pathname)
  const method = request.method.toUpperCase() as ApiMethod

  // /api/* is the Rust api's before the route table is consulted; the table
  // below is the four residents (app dispatch, admin.update, healthz) — the
  // SPA shell, and nothing else.
  const proxied = await maybeProxy(request, pathname)
  if (proxied) return proxied

  for (const route of routes) {
    const params = matchRoute(route, pathname)
    if (!params) continue
    const handler = route.handlers[method] ?? (method === 'HEAD' ? route.handlers.GET : undefined)
    if (!handler) {
      return json(
        { error: 'method not allowed' },
        { status: 405, headers: { allow: Object.keys(route.handlers).join(', ') } },
      )
    }
    return handler({ request, params })
  }

  const isApi = pathname === '/api' || pathname.startsWith('/api/') || pathname.startsWith('/.well-known/')
  if (isApi) return json({ error: 'not found' }, { status: 404 })

  if (method === 'GET' || method === 'HEAD') {
    const html = await loadShell()
    if (html !== null) {
      return new Response(html, {
        headers: { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' },
      })
    }
  }
  return new Response('Not Found', { status: 404 })
}

// The one non-fetch export, and the reason it must live HERE: only this
// module's exports survive into dist/server/server.js, and server-entry.js
// runs the migration pass off this export before it spawns the Rust api —
// post-cutover, boot itself touches no table, so a fresh database never
// migrated until this hook existed.
export { migrate } from './db/pg'

export default { fetch: handle }
