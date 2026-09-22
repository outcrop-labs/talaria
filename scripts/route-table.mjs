// route-table — the Rust router table, parsed once, for every consumer.
//
// gen-docs.mjs (the docs/api reference) and check-mcp-routes.mjs (the mcp↔api
// cross-check) both need the same truth: the `.route("/path",
// get(mod::fn).post(mod::fn)…)` registrations in
// api/crates/talaria-api-routes/src/routes/mod.rs. The parser lives here so
// the two can never disagree about what the router serves — the same
// extraction religion gen-docs' header states: hand-maintained tables rot.
//
// The functions are verbatim moves from gen-docs.mjs (which imports them
// back); byte-identical output there is the invariant of the move.
//
// Stdlib-only, like everything under scripts/.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'

/** The Rust walk: like matchDelim, but Rust's `'` is a lifetime or a char
 *  literal, never a string opener — only the two-char forms `'\…'` close. */
export function matchDelimRust(text, openIdx) {
  const open = text[openIdx]
  const close = open === '(' ? ')' : open === '{' ? '}' : open === '[' ? ']' : null
  if (!close) return -1
  let depth = 0
  let i = openIdx
  while (i < text.length) {
    const c = text[i]
    if (c === '"' || (c === 'r' && /["#]/.test(text[i + 1] ?? ''))) {
      // string, or raw string r"…" / r#"…"#
      let j = i
      let hashes = 0
      if (c === 'r') {
        j = i + 1
        while (text[j] === '#') { hashes++; j++ }
        if (text[j] !== '"') { i++; continue }
      }
      j++
      while (j < text.length) {
        if (text[j] === '\\') { j += 2; continue }
        if (text[j] === '"') {
          let k = j + 1, h = 0
          while (h < hashes && text[k] === '#') { h++; k++ }
          if (h === hashes) { i = k - 1; break }
        }
        j++
      }
      if (j >= text.length) i = text.length
    } else if (c === "'" && text[i + 1] === '\\') {
      i += 2
      while (i < text.length && text[i] !== "'") i++
      // i sits on the closing quote; the shared i++ below steps past it
    } else if (c === "'" && text[i + 2] === "'") {
      i += 2 // 'a' — leave i on the closing quote for the shared i++
    } else if (c === open) depth++
    else if (c === close) {
      depth--
      if (depth === 0) return i
    }
    i++
  }
  return -1
}

/** Strip comments from Rust text, LENGTH-PRESERVING for the same reason.
 *  Strings (incl. raw strings) pass through verbatim; `'x'` char literals
 *  pass; `'a` lifetimes are code and stay. */
export function stripRustComments(text) {
  let out = ''
  let i = 0
  while (i < text.length) {
    const c = text[i]
    if (c === '/' && text[i + 1] === '/') {
      while (i < text.length && text[i] !== '\n') { out += ' '; i++ }
    } else if (c === '/' && text[i + 1] === '*') {
      out += '  '
      i += 2
      while (i < text.length && !(text[i] === '*' && text[i + 1] === '/')) {
        out += text[i] === '\n' ? '\n' : ' '
        i++
      }
      if (i < text.length) { out += '  '; i += 2 }
    } else if (c === '"' || (c === 'r' && /["#]/.test(text[i + 1] ?? ''))) {
      let j = i
      let hashes = 0
      const isRaw = c === 'r'
      if (isRaw) {
        j = i + 1
        while (text[j] === '#') { hashes++; j++ }
        if (text[j] !== '"') { out += c; i++; continue }
      }
      out += text.slice(i, j + 1)
      j++
      while (j < text.length) {
        if (!isRaw && text[j] === '\\') { out += text[j] + text[j + 1]; j += 2; continue }
        out += text[j]
        if (text[j] === '"') {
          let k = j + 1, h = 0
          while (h < hashes && text[k] === '#') { h++; k++ }
          if (h === hashes) { j = k; break }
        }
        j++
      }
      i = Math.min(j, text.length)
    } else if (c === "'" && text[i + 1] === '\\') {
      out += text[i] + text[i + 1]
      i += 2
      while (i < text.length && text[i] !== "'") { out += text[i]; i++ }
      if (i < text.length) { out += text[i]; i++ }
    } else if (c === "'" && text[i + 2] === "'") {
      out += text.slice(i, i + 3)
      i += 3
    } else {
      out += c
      i++
    }
  }
  return out
}

/** Parse `.route("/path", get(mod::fn).post(mod::fn)…)` registrations.
 *  `warnings` (optional array) collects the table's own parity findings —
 *  an allow-string that disagrees with the parsed method set — so callers
 *  surface them their own way. */
export function parseRouterTable(modText, warnings = []) {
  const stripped = stripRustComments(modText)
  const routes = []
  let i = 0
  while (true) {
    const idx = stripped.indexOf('.route(', i)
    if (idx === -1) break
    const openParen = idx + '.route('.length - 1
    const closeParen = matchDelimRust(stripped, openParen)
    if (closeParen === -1) break
    const args = stripped.slice(openParen + 1, closeParen)
    const pathMatch = /"([^"]+)"/.exec(args)
    if (!pathMatch) throw new Error(`.route( registration without a path literal near offset ${idx}`)
    const entries = []
    const chain = /\b(get|post|put|patch|delete|head|options)\s*\(\s*([A-Za-z_][\w]*(?:::[A-Za-z_][\w]*)*)\s*\)/g
    let m
    while ((m = chain.exec(args))) entries.push({ method: m[1].toUpperCase(), fnPath: m[2] })
    // The table's own parity check: the allow-string the fallback answers 405
    // with must name exactly the methods parsed off the chain.
    const allow = /method_not_allowed\(\s*"([^"]*)"\s*\)/.exec(args)
    if (allow) {
      const declared = new Set(allow[1].split(',').map((s) => s.trim()).filter(Boolean))
      const parsed = new Set(entries.map((e) => e.method))
      if (declared.size !== parsed.size || [...declared].some((d) => !parsed.has(d))) {
        warnings.push(
          `router: ${pathMatch[1]} allow-string "${allow[1]}" disagrees with the parsed method set ` +
            `[${[...parsed].sort().join(', ')}] — one of them is wrong in mod.rs`,
        )
      }
    }
    if (!entries.length) throw new Error(`.route( for ${pathMatch[1]} has no method chain`)
    routes.push({ path: pathMatch[1], entries })
    i = closeParen
  }
  if (!routes.length) throw new Error('no .route( registrations found in api/src/routes/mod.rs')
  return routes
}

/** The handler groups live one crate per family; the table's fn paths carry
 *  the crate prefix (`talaria_routes_boards::boards::boards_id_statuses`). */
export const ROUTE_GROUP_CRATES = {
  admin: 'talaria-routes-admin',
  fleet: 'talaria-routes-fleet',
  agents: 'talaria-routes-fleet',
  apps: 'talaria-routes-fleet',
  models: 'talaria-routes-fleet',
  llm: 'talaria-routes-fleet',
  mcp: 'talaria-routes-fleet',
  boards: 'talaria-routes-boards',
  tasks: 'talaria-routes-boards',
  workchains: 'talaria-routes-boards',
  plans: 'talaria-routes-boards',
  comms: 'talaria-routes-comms',
  inbox: 'talaria-routes-comms',
  brief: 'talaria-routes-comms',
  activity: 'talaria-routes-comms',
  knowledge: 'talaria-routes-knowledge',
  files: 'talaria-routes-knowledge',
  integrations: 'talaria-routes-integrations',
  secrets: 'talaria-routes-integrations',
  account: 'talaria-routes-integrations',
  teams: 'talaria-routes-integrations',
  workbench: 'talaria-routes-workbench',
  research: 'talaria-routes-workbench',
  system: 'talaria-routes-workbench',
}

/** module path (`talaria_routes_boards::boards::boards_id_statuses`) → repo-relative handler file. */
export function moduleFile(fnPath) {
  const segs = fnPath.split('::')
  segs.pop() // the fn itself
  const [crate, group, ...rest] = segs
  return `api/crates/${ROUTE_GROUP_CRATES[group]}/src/${group}/${rest.join('/')}.rs`
}

/** The router table read and parsed in one step: every served path with its
 *  methods, plus the parity warnings. Paths keep their `{param}` syntax. */
export function readRoutes(root) {
  const modText = readFileSync(join(root, 'api/crates/talaria-api-routes/src/routes/mod.rs'), 'utf8')
  const warnings = []
  const routes = parseRouterTable(modText, warnings)
  return { routes, warnings }
}
