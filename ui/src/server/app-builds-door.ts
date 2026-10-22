// /app-builds/<slug>/<key>/… — the compiled CLIENT of one app, and nothing
// else, to a session that holds a grant for that app.
//
// This door used to serve every file under the builds directory to anyone
// who asked: server.js and mcp.js (an app's compiled server and tool code)
// beside app.js, and <slug>/current.json, which names the current key. A
// private, org-specific plugin was readable by anyone who could reach the
// instance. The browser needs exactly three things — app.js, app.css and the
// chunks and files under assets/ that they pull in (lib/apps.ts,
// AppSurface.svelte) — so that is the whole list, and the same gate the app's
// API uses (app-access.ts) decides who gets them.
//
// Served from the one fetch handler (server/app.ts), so `server-entry.ts` in
// production and the dev middleware (app-runtime/vite-plugin.ts) answer
// through the same code.
import { readFile, stat } from 'node:fs/promises'
import { extname, join, resolve, sep } from 'node:path'
import { authorizeApp } from './app-access'
import { appBuildsDir, slugOk } from './app-build/paths'

export const APP_BUILDS_PREFIX = '/app-builds/'

/** One requestable build file: which app, which build, and the path inside
 *  that build's directory. */
export interface AppBuildAsset {
  slug: string
  key: string
  rel: string[]
}

// A build key is the hex digest app-build/key.ts writes.
const KEY_RE = /^[0-9a-f]{1,64}$/

/** A path segment that names a file or directory under assets/: not empty,
 *  no separator or NUL, and no leading dot — which refuses `..`, `.` and
 *  every dotfile (the builder's .cache-* directories among them). */
const plainSegment = (s: string): boolean => s !== '' && !s.startsWith('.') && !/[\\/\0]/.test(s)

/** Parse `/app-builds/<slug>/<key>/<file>` into the asset it names, or null
 *  for anything that is not app.js, app.css or a file under assets/. Takes
 *  the RAW (percent-encoded) pathname and decodes each segment once, so an
 *  encoded slash can never pass as a separator. */
export function appBuildAsset(rawPathname: string): AppBuildAsset | null {
  if (!rawPathname.startsWith(APP_BUILDS_PREFIX)) return null
  const parts: string[] = []
  for (const raw of rawPathname.slice(APP_BUILDS_PREFIX.length).split('/')) {
    try {
      parts.push(decodeURIComponent(raw))
    } catch {
      return null
    }
  }
  const [slug, key, ...rel] = parts
  if (!slug || !key || !slugOk(slug) || !KEY_RE.test(key)) return null
  const [head, ...rest] = rel
  if (rel.length === 1 && (head === 'app.js' || head === 'app.css')) return { slug, key, rel }
  if (head === 'assets' && rest.length > 0 && rest.every(plainSegment)) return { slug, key, rel }
  return null
}

const MIME: Record<string, string> = {
  '.js': 'application/javascript',
  '.mjs': 'application/javascript',
  '.css': 'text/css',
  '.json': 'application/json',
  '.map': 'application/json',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.wasm': 'application/wasm',
}

const notFound = () => new Response('not found', { status: 404, headers: { 'content-type': 'text/plain', 'cache-control': 'no-store' } })

/** Answer an /app-builds/ request: 404 for anything outside the client list
 *  (the same 404 whether or not the file exists), then the app gate (401,
 *  404 for a disabled app, 403 without a grant, 503 when the session cannot
 *  be read), then the file. */
export async function serveAppBuild(request: Request): Promise<Response> {
  if (request.method !== 'GET' && request.method !== 'HEAD') {
    return new Response('method not allowed', { status: 405, headers: { allow: 'GET, HEAD' } })
  }
  const asset = appBuildAsset(new URL(request.url).pathname)
  if (!asset) return notFound()
  const gate = await authorizeApp(request, asset.slug)
  if (!gate.ok) return gate.response
  const root = resolve(appBuildsDir())
  const file = resolve(join(root, asset.slug, asset.key, ...asset.rel))
  if (!file.startsWith(root + sep)) return notFound()
  try {
    const st = await stat(file)
    if (!st.isFile()) return notFound()
    const body = request.method === 'HEAD' ? null : new Uint8Array(await readFile(file))
    return new Response(body, {
      status: 200,
      headers: {
        'content-type': MIME[extname(file).toLowerCase()] ?? 'application/octet-stream',
        'content-length': String(st.size),
        // A build key names its bytes forever, so the browser may keep them —
        // but only this browser: the door is per-session now, and no shared
        // cache may hand one person's grant to another.
        'cache-control': 'private, max-age=31536000, immutable',
        'x-content-type-options': 'nosniff',
      },
    })
  } catch {
    return notFound()
  }
}
