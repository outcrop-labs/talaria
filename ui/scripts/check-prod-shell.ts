// Prod-bundle smoke — CI's proof that the BUILT server serves the app, not
// just that the source typechecks and its unit tests pass.
//
//   cd ui && bun run build && bun scripts/check-prod-shell.ts
//
// Why this exists (2026-09-17): a vite output change split the server bundle
// into dist/server/assets/*.js — one directory deeper than the
// dist/server/server.js the SPA-shell lookup was anchored to. The read threw,
// the handler cached shell = null, and every page on every instance answered
// 404 while /api kept working. Dev mode never runs the shell path (vite
// serves index.html itself) and no gate executed the built bundle, so CI was
// green on a build that served nothing.
//
// What it does: imports dist/server/server.js and calls its fetch handler the
// way server-entry.ts does. GET / needs no database and no rust api; if that
// ever stops being true, growing this script is the point.
process.env.DATABASE_URL ??= 'postgres://prod-shell@127.0.0.1:1/prod_shell'

import { pathToFileURL } from 'node:url'

const fail = (msg: string): never => {
  console.error(`[prod-shell] ${msg}`)
  process.exit(1)
}

// Non-literal on purpose, same rule as server-entry.ts: dist/ is build output.
let mod: { default?: { fetch: (request: Request) => Promise<Response> } }
try {
  mod = (await import(pathToFileURL('dist/server/server.js').href)) as typeof mod
} catch (e) {
  fail(`cannot import dist/server/server.js — run \`bun run build\` first (${String(e)})`)
}
const handler = mod.default?.fetch
if (typeof handler !== 'function') fail('bundle exports no default.fetch handler')

const get = (path: string): Promise<Response> =>
  handler(new Request(`http://prod-shell-check.local${path}`, { method: 'GET' }))

const page = await get('/')
if (page.status !== 200) fail(`GET / answered ${page.status} — the SPA shell is not being served (expected 200 text/html)`)
const ctype = page.headers.get('content-type') ?? ''
if (!ctype.startsWith('text/html')) fail(`GET / served content-type "${ctype}" — expected text/html`)
const html = await page.text()
if (!/<html/i.test(html)) fail(`GET / served a body that is not HTML ("${html.slice(0, 80)}")`)

// A client-side route deep in the tree gets the same shell — the fallback is
// the product, not just the bare root.
const deep = await get('/home/inbox')
if (deep.status !== 200 || !(deep.headers.get('content-type') ?? '').startsWith('text/html')) {
  fail(`GET /home/inbox answered ${deep.status} — the SPA fallback is broken for client routes`)
}

// The API door still gates: an unknown /api path belongs to the rust proxy
// (502 here — no backend next to a bare bundle), but it must NEVER get the
// SPA shell — that would mean the fallback is swallowing API paths.
const api = await get('/api/definitely-not-a-route')
if ((api.headers.get('content-type') ?? '').startsWith('text/html')) {
  fail(`GET /api/<unknown> served HTML (${api.status}) — the API door is leaking the SPA shell`)
}

console.log('[prod-shell] OK — built bundle serves the SPA shell and keeps /api off it')
