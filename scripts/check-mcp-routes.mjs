#!/usr/bin/env node
// check-mcp-routes — every route the MCP server calls must exist in the Rust
// router, with the method it uses.
//
//   bun scripts/check-mcp-routes.mjs     (wired into `bun run check`)
//
// The MCP server binds the api by hardcoded path strings —
// `api('GET', '/api/boards')` at mcp/src/index.ts — so an api route rename or
// removal shows up there as a runtime 404 inside a live agent session, the
// exact failure mode whose loud cousin (the VERIFY_PATH probe) mcp already
// reports at boot. This check makes the whole surface static: it reads the
// router table through route-table.mjs (the same parse gen-docs generates
// docs/api from, so the two can never disagree) and fails naming the call
// site for anything that does not resolve.
//
// Scans ALL of mcp/src/**, so a future split of index.ts changes nothing.
// Template literals (`/api/boards/${encodeURIComponent(id)}/tasks`) match
// `{param}` segments; a trailing hole after a '?' is a query string. The
// matcher is deliberately conservative: when it cannot decide, it fails —
// argue the exception in the PR that adds it to the ALLOWLIST, don't widen
// the pattern.
//
// Stdlib-only, like everything under scripts/.

import { readFileSync, readdirSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'
import { readRoutes } from './route-table.mjs'

const ROOT = resolve(import.meta.dirname, '..')

// Genuinely-unmatchable call sites live here with the reason. EMPTY TODAY —
// none of mcp's calls hit the SPA host's TS residents (they are all
// Rust-served paths).
const ALLOWLIST = []

const { routes, warnings } = readRoutes(ROOT)
for (const w of warnings) console.error(`warn: ${w}`)

// ── route table → matchers ───────────────────────────────────────────────────
const esc = (s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

const matchers = routes.map((r) => ({
  path: r.path,
  methods: new Set(r.entries.map((e) => e.method)),
  // `/api/boards/{id}/tasks` → `^/api/boards/[^/]+/tasks(\?.*)?$`
  regex: new RegExp(
    '^' + r.path.split('/').map((seg) => (seg.startsWith('{') ? '[^/]+' : esc(seg))).join('/') + '(\\?.*)?$',
  ),
}))

/** Split a template-literal body into static chunks, brace-aware: a hole's
 *  expression may itself contain a nested template (`${q ? `?q=${…}` : ''}`)
 *  or braces — a flat `${[^}]*}` split breaks on those. */
function templateParts(body) {
  const parts = []
  let static_ = ''
  let i = 0
  while (i < body.length) {
    if (body[i] === '$' && body[i + 1] === '{') {
      parts.push(static_)
      static_ = ''
      let depth = 1
      i += 2
      while (i < body.length && depth > 0) {
        const c = body[i]
        if (c === '{') depth++
        else if (c === '}') depth--
        else if (c === '`') {
          i++
          while (i < body.length && body[i] !== '`') i += body[i] === '\\' ? 2 : 1
        } else if (c === "'") {
          i++
          while (i < body.length && body[i] !== "'") i += body[i] === '\\' ? 2 : 1
        }
        i++
      }
      continue
    }
    if (body[i] === '\\') { static_ += body[i] + (body[i + 1] ?? ''); i += 2; continue }
    static_ += body[i]
    i++
  }
  parts.push(static_)
  return parts
}

/** A template-literal body → matcher regex source: static chunks escaped,
 *  holes widened. A MIDDLE hole is one path segment (`[^/]*`); a trailing
 *  hole is the last segment OR a query tail (`[^/?]*(\?.*)?`) — it must never
 *  reach across a '/', or `/api/tasks/${id}` would match
 *  `/api/tasks/{id}/work-session`. A hole after a '?' is query territory. */
function templateRegexSource(body) {
  const parts = templateParts(body)
  let src = ''
  parts.forEach((part, i) => {
    src += esc(part)
    if (i < parts.length - 1) {
      if (part.includes('?')) src += '.*'
      else if (i === parts.length - 2 && parts[parts.length - 1] === '') src += '[^/?]*(\\?.*)?'
      else src += '[^/]*'
    }
  })
  return src
}

const failures = []
let callSites = 0
let fetchSites = 0

function check(method, pathSource, where) {
  const abs = new RegExp('^' + pathSource + '(\\?.*)?$')
  // ALL structural matches, not the first: a template like
  // `/api/artifacts/${id}` matches both the literal `/api/artifacts/for` and
  // the dynamic `/api/artifacts/{id}` — and the router serves the call if ANY
  // structural match carries the method (axum would prefer the static, but
  // for a template the segment is dynamic by construction).
  const hits = matchers.filter((m) => abs.test(m.path.replace(/\?.*$/, '')))
  if (!hits.length) {
    failures.push(`${where} — ${method} ${pathSource.replace(/\\([/.])/g, '$1')} matches no route in the Rust router table`)
    return
  }
  if (!hits.some((m) => m.methods.has(method))) {
    const served = [...new Set(hits.flatMap((m) => [...m.methods]))].sort()
    failures.push(
      `${where} — ${hits.map((h) => h.path).join(' | ')} ${hits.length > 1 ? 'are' : 'is'} served, but not as ${method} (${served.join(', ')})`,
    )
  }
}

// ── scan mcp/src/** ──────────────────────────────────────────────────────────
function walk(dir, out = []) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (e.isDirectory()) walk(join(dir, e.name), out)
    else if (e.name.endsWith('.ts')) out.push(join(dir, e.name))
  }
  return out
}

for (const file of walk(join(ROOT, 'mcp/src'))) {
  const rel = relative(ROOT, file)
  const text = readFileSync(file, 'utf8')
  const lineOf = (idx) => text.slice(0, idx).split('\n').length

  // api('METHOD', …) call sites. The helper's own definition takes variable
  // names, so the literal-method requirement skips it.
  const re = /\bapi\(\s*'(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)'\s*,\s*/g
  let m
  while ((m = re.exec(text))) {
    const i = re.lastIndex
    const q = text[i]
    if (q !== "'" && q !== '`') continue // not a literal/template path — nothing to check
    let j = i + 1
    while (j < text.length && text[j] !== q) j += text[j] === '\\' ? 2 : 1
    const body = text.slice(i + 1, j)
    callSites++
    const where = `${rel}:${lineOf(m.index)}`
    if (ALLOWLIST.includes(where)) continue
    check(m[1], q === "'" ? esc(body) : templateRegexSource(body), where)
    re.lastIndex = j + 1
  }

  // Raw fetches that name a path literally: fetch(`${BASE}/api/…`). The
  // helper's own generic fetch (`${BASE}${path}`) has no literal and skips.
  const fre = /fetch\(\s*`\$\{BASE\}(\/api\/[^`]*)`/g
  while ((m = fre.exec(text))) {
    fetchSites++
    const where = `${rel}:${lineOf(m.index)}`
    if (ALLOWLIST.includes(where)) continue
    // All current raw fetches are GETs (uploads bytes, the verify probe).
    // If one ever POSTs, read the method from the init object instead.
    check('GET', templateRegexSource(m[1]), where)
  }

  // The verify probe's path: resolve VERIFY_PATH's default literal.
  const v = /const VERIFY_PATH = [^;]*?'(\/api\/[^']*)'/.exec(text)
  if (v) {
    check('GET', esc(v[1]), `${rel}:VERIFY_PATH default`)
  }
}

// ── report ───────────────────────────────────────────────────────────────────
if (failures.length) {
  console.error('\nFAIL mcp-route-drift')
  for (const f of failures) console.error(`  ${f}`)
  console.error(
    '\nWHAT TO DO INSTEAD: the router table is the truth ' +
      '(api/crates/talaria-api-routes/src/routes/mod.rs).\n' +
      '  Fix the tool path or the route — not this checker. If the call is genuinely correct\n' +
      '  and the matcher cannot see it, add it to ALLOWLIST in scripts/check-mcp-routes.mjs\n' +
      '  with the reason, and argue it in the PR.',
  )
  process.exit(1)
}
console.log(`mcp-routes: ${callSites} api() + ${fetchSites} fetch call sites all resolve against ${matchers.length} routes`)
