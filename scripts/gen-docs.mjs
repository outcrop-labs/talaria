#!/usr/bin/env node
// gen-docs — generates the references that must never drift from source:
//
//   docs/api/**            the HTTP API reference, one file per resource group
//   docs/api/README.md     index + auth legend + envelope summary
//   docs/CLI-REFERENCE.md  the CLI command reference
//
// THE RELIGION. Hand-maintained reference tables rot — the SDK doc listed
// exports that didn't exist and missed ones that did, for months, and nothing
// failed. So the reference is EXTRACTED from the source the server actually
// runs, and `--check` (wired into `bun run check`) fails CI when they diverge.
// You cannot edit docs/api/** by hand and have it stick; that is the point.
//
// WHERE THE TRUTH LIVES NOW. The api is Rust: the router table in
// api/src/routes/mod.rs names every path and method (217 registrations, each
// with a `.fallback` allow-string this script cross-checks against the parsed
// method set), and the handler modules under api/src/routes/** carry the
// guards, body members, status literals and json! shapes. The three routes
// the SPA host still serves itself — healthz, admin.update, the app-server
// dispatch — are extracted from their TS residents exactly as before
// (mcp.gw.$server.ts is NOT one of them: /api/mcp/gw/{server} serves from
// Rust; only its /api/mcp/gw/app-* branch stays TS, and that branch has
// never had a row of its own).
//
// WHAT IS EXACT vs HEURISTIC. Path literals, methods, auth guards, body
// member tables, audit/SSE markers and literal status codes are read from
// the route source (see extractors below). The Returns column is a heuristic
// — the top-level keys of the first success-shaped `json!({…})` literal,
// `…` when the shape isn't a literal — and is marked as such in every file's
// banner. Where the extractor would have to guess, it prints `…` instead.
// Never trust a doc over the code; this tool exists to make that cheap.
//
// USAGE
//   bun scripts/gen-docs.mjs          write all generated files
//   bun scripts/gen-docs.mjs --check  diff against disk; exit 1 naming drift
//
// NOTES COME FROM THE SOURCE. A Rust handler may carry `// doc:` comment
// runs — at the top of the module file for a path-level note, directly above
// the handler fn for a method note. They render verbatim. Without one, the
// path note falls back to the module's own leading comment header (capped).
// Prose about a route lives with the route or nowhere.

import { readdirSync, readFileSync, writeFileSync, existsSync } from 'node:fs'
import { join, resolve, relative } from 'node:path'

const ROOT = resolve(import.meta.dirname, '..')
const ROUTES_MOD = join(ROOT, 'api/src/routes/mod.rs')
const ROUTES_DIR = join(ROOT, 'api/src/routes')
// The SPA host's permanent TS residents (rust-proxy.ts STAY_TS), minus the
// app-MCP branch that never had its own row. Extracted with the TS extractor.
const TS_RESIDENT_FILES = [
  'ui/src/routes/api/healthz.ts',
  'ui/src/routes/api/admin.update.ts',
  'ui/src/routes/api/apps.$app.$.ts',
]
const CHECK = process.argv.includes('--check')

// ── Grouping ────────────────────────────────────────────────────────────────
// First path segment after /api/ → group file. EXHAUSTIVE BY DESIGN: an
// unknown segment throws, so a new resource area forces a reviewed entry here
// (the check-invariants census philosophy — the map going stale is a build
// failure, not a silent misc bucket).

const GROUPS = {
  account: { segs: ['auth', 'me', 'join', 'users'], blurb: 'Sign-in, session, profile, members.' },
  activity: { segs: ['activity', 'history', 'notifications', 'alerts', 'home', 'cost'], blurb: 'What happened, what it cost, what needs you.' },
  admin: { segs: ['admin'], blurb: 'Instance administration (admin session required).' },
  agents: { segs: ['agents', 'agent', 'agent-role-templates', 'gaps', 'skills', 'muse', 'vision', 'runs'], blurb: 'Agent CRUD, registration, heartbeats, skills, runs.' },
  apps: { segs: ['apps'], blurb: 'The app platform surface and the app-server gateway.' },
  boards: { segs: ['boards'], blurb: 'Kanban boards, members, statuses, labels, views.' },
  brief: { segs: ['brief'], blurb: 'The personal brief: items, replies, delegation.' },
  comms: { segs: ['channels', 'chat', 'conversations', 'dms'], blurb: 'Channels, DMs, threads, chat streaming.' },
  files: { segs: ['artifacts', 'artifact-folders', 'uploads', 'agent-media'], blurb: 'Uploads, artifacts, shares, downloads.' },
  fleet: { segs: ['fleet'], blurb: 'The agent fleet: defs, containers, crons, federation.' },
  inbox: { segs: ['inbox'], blurb: 'The focus inbox and its command surface.' },
  integrations: { segs: ['integrations'], blurb: 'Connected accounts — Google Workspace and the rest.' },
  knowledge: { segs: ['kb', 'rag', 'memory', 'search', 'templates'], blurb: 'Knowledge base, RAG collections, org templates, search.' },
  llm: { segs: ['llm'], blurb: 'The OpenAI-compatible wire (llm.v1.*).' },
  mcp: { segs: ['mcp'], blurb: 'MCP servers, governance, gateway, OAuth.' },
  models: { segs: ['models', 'keys', 'inference'], blurb: 'Model providers, gateway API keys, local backends.' },
  plans: { segs: ['plans'], blurb: 'Living plans: draft doc, members.' },
  research: { segs: ['research'], blurb: 'Cited research reports and their conversations.' },
  secrets: { segs: ['secrets'], blurb: 'The sealed-secrets vault: folders, shares, reveal, relay.' },
  system: { segs: ['healthz', 'well-known'], blurb: 'Health and instance discovery endpoints.' },
  tasks: { segs: ['tasks', 'workflows'], blurb: 'Tickets, comments, dependencies, watchers, workflows.' },
  teams: { segs: ['teams'], blurb: 'Teams and their members.' },
  workbench: { segs: ['workbench'], blurb: 'The developer workbench: repos, jobs, harnesses, flows.' },
}
const SEG_TO_GROUP = Object.fromEntries(Object.entries(GROUPS).flatMap(([g, v]) => v.segs.map((s) => [s, g])))

const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS']

// ── Small text tools ────────────────────────────────────────────────────────

/** Index of the closer matching text[openIdx]; handles (), {} and [] and
 *  skips string literals so a brace in a prompt string can't break the walk. */
function matchDelim(text, openIdx) {
  const open = text[openIdx]
  const close = open === '(' ? ')' : open === '{' ? '}' : open === '[' ? ']' : null
  if (!close) return -1
  let depth = 0
  let i = openIdx
  while (i < text.length) {
    const c = text[i]
    if (c === "'" || c === '`') {
      const q = c
      i++
      while (i < text.length && text[i] !== q) i += text[i] === '\\' ? 2 : 1
    } else if (c === '"' && text[i - 1] !== "'") {
      i++
      while (i < text.length && text[i] !== '"') i += text[i] === '\\' ? 2 : 1
    } else if (c === open) depth++
    else if (c === close) {
      depth--
      if (depth === 0) return i
    }
    i++
  }
  return -1
}

/** The Rust walk: like matchDelim, but Rust's `'` is a lifetime or a char
 *  literal, never a string opener — only the two-char forms `'\…'` close. */
function matchDelimRust(text, openIdx) {
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

/** Strip line and block comments from TS text, LENGTH-PRESERVING — every
 *  comment character becomes a space (newlines kept), so offsets and line
 *  numbers into the original text stay valid. String literals pass through
 *  verbatim (a `//` inside one is not a comment). */
function stripComments(text) {
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
    } else if (c === "'" || c === '`' || c === '"') {
      const q = c
      out += c
      i++
      while (i < text.length) {
        if (text[i] === '\\' && i + 1 < text.length) { out += text[i] + text[i + 1]; i += 2; continue }
        out += text[i]
        if (text[i] === q) { i++; break }
        i++
      }
    } else {
      out += c
      i++
    }
  }
  return out
}

/** Strip comments from Rust text, LENGTH-PRESERVING for the same reason.
 *  Strings (incl. raw strings) pass through verbatim; `'x'` char literals
 *  pass; `'a` lifetimes are code and stay. */
function stripRustComments(text) {
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

/** Collapse whitespace and cap length — schema expressions render verbatim
 *  but never unbounded. */
function oneline(text, cap = 140) {
  const s = text.replace(/\s+/g, ' ').trim().replace(/,$/, '').replace(/ \./g, '.').replace(/\( /g, '(').replace(/ \)/g, ')')
  return s.length > cap ? s.slice(0, cap - 1) + '…' : s
}

/** Top-level split of an object-literal body by commas. */
function splitTop(body) {
  const parts = []
  let depth = 0
  let start = 0
  let i = 0
  while (i < body.length) {
    const c = body[i]
    if (c === "'" || c === '`') {
      const q = c
      i++
      while (i < body.length && body[i] !== q) i += body[i] === '\\' ? 2 : 1
    } else if (c === '"' && body[i - 1] !== "'") {
      i++
      while (i < body.length && body[i] !== '"') i += body[i] === '\\' ? 2 : 1
    } else if ('({['.includes(c)) depth++
    else if (')}]'.includes(c)) depth--
    else if (c === ',' && depth === 0) {
      parts.push(body.slice(start, i))
      start = i + 1
    }
    i++
  }
  const tail = body.slice(start)
  if (tail.trim()) parts.push(tail)
  return parts
}

/** The // run above line `lineIdx` (exclusive), capped. Blank lines are
 *  skipped only BEFORE the run starts — a comment separated from the code by
 *  a blank line is still its comment; once the run begins it must be
 *  contiguous. */
function commentRunAbove(lines, lineIdx, cap = Infinity) {
  const run = []
  for (let i = lineIdx - 1; i >= 0; i--) {
    const t = lines[i].trim()
    if (t.startsWith('//')) run.unshift(t.replace(/^\/\/ ?/, ''))
    else if (!t && run.length === 0) continue
    else break
  }
  if (run.length > cap) return { text: run.slice(0, cap), trimmed: true }
  return { text: run, trimmed: false }
}

/** GitHub heading anchor. */
const anchor = (h) => '#' + h.toLowerCase().replace(/[`'"*_./{}$]/g, '').replace(/\s+/g, '-')

const lineOf = (text, offset) => text.slice(0, offset).split('\n').length - 1

// ── Rust route extraction ───────────────────────────────────────────────────
//
// The router table in mod.rs is the path/method truth; the handler modules
// are everything else. Both are read as TEXT (no compilation): the table is
// regular enough that a delimiter-aware scan is exact, and the handler scans
// below are heuristics over a small, closed vocabulary of house idioms
// (crate::body members, guard calls, json! literals).

const warnings = []

/** Parse `.route("/path", get(mod::fn).post(mod::fn)…)` registrations. */
function parseRouterTable(modText) {
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

/** module path (`boards::boards_id_statuses`) → repo-relative handler file. */
function moduleFile(fnPath) {
  const segs = fnPath.split('::')
  segs.pop() // the fn itself
  return `api/src/routes/${segs.join('/')}.rs`
}

/** Find `fn name(…) {body}` in comment-stripped Rust text. Returns the body's
 *  interior span, or null. Offsets are into the stripped text, which is
 *  length-preserving — they address the raw file too. */
function findFnSpan(stripped, fnName) {
  const re = new RegExp(`\\b(?:pub\\s+)?(?:async\\s+)?fn\\s+${fnName}\\s*(?:<[^>]*>)?\\s*\\(`)
  const m = re.exec(stripped)
  if (!m) return null
  const openParen = m.index + m[0].length - 1
  const closeParen = matchDelimRust(stripped, openParen)
  if (closeParen === -1) return null
  const bodyOpen = stripped.indexOf('{', closeParen)
  if (bodyOpen === -1) return null
  const bodyClose = matchDelimRust(stripped, bodyOpen)
  if (bodyClose === -1) return null
  return { start: bodyOpen + 1, end: bodyClose }
}

/** Every fn defined in this file (helpers get inlined into handler scans). */
function localFns(stripped) {
  const fns = new Map()
  const re = /\b(?:pub\s+)?(?:async\s+)?fn\s+([a-z_][\w]*)\s*(?:<[^>]*>)?\s*\(/g
  let m
  while ((m = re.exec(stripped))) {
    if (!fns.has(m[1])) {
      const span = findFnSpan(stripped, m[1])
      if (span) fns.set(m[1], span)
    }
    re.lastIndex = m.index + m[0].length - 1
  }
  return fns
}

/** A handler's own body plus the bodies of same-file helpers it calls (one
 *  level of recursion) — helpers carry guards, body members and audit writes
 *  of their own. Sibling ROUTE HANDLERS are never helpers (a `.get(` map
 *  method call must not inline the file's `get` handler and its guards), and
 *  neither is a method call `x.name(`. Returns the concatenated text AND the
 *  offset map needed to translate an offset in it back to a line in the raw
 *  file (for notes). */
function handlerWithHelpers(stripped, span, locals, depth = 2, seen = new Set(), exclude = new Set()) {
  const parts = [{ text: stripped.slice(span.start, span.end), fileOffset: span.start }]
  const called = (name) => new RegExp(`(?<!\\.)\\b${name}\\s*\\(`).test(parts.map((p) => p.text).join('\n'))
  if (depth > 0) {
    for (const [name, helperSpan] of locals) {
      if (seen.has(name) || exclude.has(name)) continue
      if (called(name)) {
        seen.add(name)
        parts.push(...handlerWithHelpers(stripped, helperSpan, locals, depth - 1, seen, exclude).parts)
      }
    }
  }
  return {
    text: parts.map((p) => p.text).join('\n'),
    parts,
    /** file line number of an offset into `text` */
    lineOf: (offset) => {
      let acc = 0
      for (const p of parts) {
        if (offset <= acc + p.text.length) return lineOf(stripped, p.fileOffset + (offset - acc))
        acc += p.text.length + 1 // the joining '\n'
      }
      return lineOf(stripped, parts[0].fileOffset)
    },
  }
}

/** The guard vocabulary, in precedence order. `dual` = a session path and an
 *  agent-key path both reach the handler (agent_caller's Ok(None) falls
 *  through to a require_user — directly or via a same-file helper). The
 *  vocabulary is CLOSED: a call that hands `headers` to something this list
 *  doesn't know renders `unknown(fn)` — never a false `public` — and the run
 *  logs it, so a new guard spelling fails loudly until it's added here. */
const KNOWN_HEADER_TAKERS = new Set([
  'require_user', 'require_admin', 'require_perm', 'require_view',
  'agent_caller', 'require_agent', 'fleet_caller', 'check_fleet_key',
  'authenticate_key', 'presented', 'subject_model', 'epoch_ms_to_iso',
  'get_session_user', 'update_sessions_for_user', 'destroy_session',
  'resolve_origin', 'json_with_cookies', 'acting_user',
  // header readers that are NOT guards: rate-limit keying, OAuth origin
  // pinning, the shared Google connect-callback body, cookie building/parsing
  'client_ip', 'oauth_relocation', 'handle_connect_callback',
  'session_cookie_for', 'parse_cookies', 'google_auth_url', 'google_redirect_uri',
  'state_cookie_for', 'clear_state_cookie_for',
])

function authClassRust(combined, locals, fileRaw) {
  const has = (s) => combined.includes(s)
  const perms = [...combined.matchAll(/(?:require_perm|has_perm)\([^)]*?"([\w.:-]+)"/g)].map((m) => m[1])
  const view = /require_view\([^)]*?"([^"]+)"/.exec(combined)
  if (has('check_fleet_key(') || has('fleet_caller(')) return { auth: 'fleet' }
  if (has('authenticate_key(')) return { auth: 'bearer-key' }
  if (has('require_agent(')) return { auth: 'agent' }
  // acting_user is session.rs's assistant-proxying-a-human resolver — the
  // Rust spelling of the TS actingUser(). Its Ok(None) fall-through resolves
  // the SESSION inside session.rs, so any handler that leads with it is dual
  // by construction, whatever guard spelling follows.
  if (has('acting_user(')) return { auth: 'dual' }
  const session = has('require_user(') || has('require_admin(') || has('require_perm(') || has('require_view(') || has('has_perm(')
  if (has('agent_caller(')) return { auth: session ? 'dual' : 'agent' }
  // perm → admin → view, the same precedence the TS extractor used, so a
  // before/after diff of the auth column means the ROUTE changed, not the
  // classifier.
  if (perms.length) return { auth: 'session', perms: [...new Set(perms)] }
  if (has('require_admin(')) return { auth: 'admin' }
  if (view) return { auth: 'session', view: view[1] }
  if (has('require_user(') || has('get_session_user(') || has('update_sessions_for_user(') || has('destroy_session(')) return { auth: 'session' }
  // No known guard. If something in the handler still takes `headers` and we
  // don't recognize it, say so instead of claiming public.
  const callRe = /([A-Za-z_][\w]*(?:::[\w:]+)?)\s*\(/g
  let m
  while ((m = callRe.exec(combined))) {
    const open = m.index + m[0].length - 1
    const close = matchDelimRust(combined, open)
    if (close === -1) continue
    const args = combined.slice(open + 1, close)
    const callee = m[1].split('::').pop()
    if (/\bheaders\b/.test(args) && !KNOWN_HEADER_TAKERS.has(callee) && !locals.has(callee)) {
      return { auth: `unknown(${callee})` }
    }
    callRe.lastIndex = open
  }
  return { auth: 'public' }
}

// ── Rust body extraction ────────────────────────────────────────────────────
//
// Bodies are read as crate::body member calls on the object `as_object`
// returned — the port's spelling of the TS zod schema. Each member call names
// its key and bounds; the renderer turns the vocabulary into a compact schema
// expression (`string(1, 40)`, `enum(open|active|review|done)?`, …).

const MEMBER_SCHEMA = {
  string_member: (n) => `string(${n})`,
  optional_string_member: (n) => `string?(${n})`,
  optional_max_string_member: (n) => `string?(${n})`,
  nullish_max_string_member: (n) => `string? nullish(${n})`,
  nullable_optional_string_member: () => `string? nullish`,
  nullable_string_member: (n) => `string? nullable(${n})`,
  present_nullable_string_member: () => `string? nullable`,
  present_nullable_max_string_member: (n) => `string? nullable(${n})`,
  trimmed_string_member: (n) => `string trimmed(${n})`,
  boolean_member: () => `bool`,
  optional_boolean_member: () => `bool?`,
  nullable_boolean_member: () => `bool? nullable`,
  number_member: (n) => `number(${n})`,
  optional_number_member: (n) => `number?(${n})`,
  nullable_number_member: (n) => `number? nullable(${n})`,
  nullish_positive_int_member: () => `int? nullish (>0)`,
  uuid_member: () => `uuid`,
  optional_uuid_member: () => `uuid?`,
  nullable_uuid_member: () => `uuid? nullable`,
  present_nullable_uuid_member: () => `uuid? nullable`,
  uuid_array_member: (n) => `uuid[](${n})`,
  optional_uuid_array_member: (n) => `uuid[]?(${n})`,
  email_member: () => `email`,
  preprocessed_email_member: () => `email`,
  optional_email_array_member: () => `email[]?`,
  url_member: () => `url`,
  optional_url_member: () => `url?`,
  nullish_datetime_member: () => `datetime? nullish`,
  present_nullable_datetime_member: () => `datetime? nullable`,
  kebab_member: (n) => `kebab(${n})`,
  slug_member: (n) => `slug(${n})`,
  string_array_member: (n) => `string[](${n})`,
  optional_string_array_member: (n) => `string[]?(${n})`,
  literal_true_member: () => `literal(true)`,
  string_value_member: () => `value`,
  nullish_member: () => `nullish`,
}

/** Enum options arrive as an inline `&["a","b"]` or a const name — resolved
 *  from the module first, then from an index of every `const NAME: &[&str]`
 *  under api/src (shared vocab consts live outside the route module). */
let _constIndex = null
function constIndex() {
  if (_constIndex) return _constIndex
  _constIndex = new Map()
  const walk = (dir) => {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      if (e.isDirectory()) walk(join(dir, e.name))
      else if (e.name.endsWith('.rs')) {
        const text = readFileSync(join(dir, e.name), 'utf8')
        for (const m of text.matchAll(/const\s+([A-Z_][A-Z0-9_]*)\b[^=]*=\s*&?\s*\[([^\]]*)\]/g)) {
          const opts = [...m[2].matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((x) => x[1])
          if (opts.length && !_constIndex.has(m[1])) _constIndex.set(m[1], opts)
        }
      }
    }
  }
  walk(join(ROOT, 'api/src'))
  return _constIndex
}

function enumOptions(argsText, fileRaw) {
  const inline = /&\s*\[([^\]]*)\]/.exec(argsText)
  if (inline) {
    const opts = [...inline[1].matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => m[1])
    if (opts.length) return opts
  }
  const konst = /,\s*&?\s*([A-Z_][A-Z0-9_]*)\s*$/.exec(argsText)
  if (konst) {
    const def = new RegExp(`const\\s+${konst[1]}\\b[^=]*=\\s*&?\\s*\\[([^\\]]*)\\]`, 's').exec(fileRaw)
    if (def) {
      const opts = [...def[1].matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => m[1])
      if (opts.length) return opts
    }
    return constIndex().get(konst[1]) ?? null
  }
  return null
}

function bodySchemaRust(combined, sigText, fileRaw) {
  const text = combined.text
  // The object every member call reads from — `as_object(&parsed)` (the fn)
  // or `parsed.as_object()` (the method).
  const objVarMatch =
    /([a-z_][\w]*)\s*=\s*(?:match\s+)?(?:crate::body::)?as_object\(/.exec(text) ||
    /([a-z_][\w]*)\s*=\s*(?:match\s+)?[a-z_][\w]*\.as_object\(/.exec(text)
  const objVar = objVarMatch ? objVarMatch[1] : null
  if (!objVar) {
    // Typed extractors / serde structs: name them, don't fake them.
    const typed = /Json\s*<\s*([A-Za-z_][\w]*)/.exec(sigText) || /from_slice(?:::)?<([^>]+)>/.exec(sigText)
    if (typed) return { kind: 'opaque', name: typed[1].trim() }
    return null
  }
  const fields = []
  const callRe = new RegExp(`\\b([a-z_][\\w]*)\\(\\s*${objVar}\\s*,\\s*"((?:[^"\\\\]|\\\\.)*)"`, 'g')
  let m
  while ((m = callRe.exec(text))) {
    const fn = m[1]
    const key = m[2]
    const parenIdx = text.indexOf('(', m.index + fn.length)
    const close = matchDelimRust(text, parenIdx)
    const argsText = close === -1 ? '' : text.slice(parenIdx + 1, close)
    const nums = [...argsText.matchAll(/(?<![\w.])(\d+(?:_\d+)?)/g)].map((x) => x[1].replace(/_/g, ''))
    const opts = /enum/.test(fn) ? enumOptions(argsText, fileRaw) : null
    let expr
    if (fn in MEMBER_SCHEMA) expr = MEMBER_SCHEMA[fn](nums.join(', '))
    else if (opts) expr = `enum(${opts.join('|')})${fn.startsWith('optional_') ? '?' : fn.startsWith('nullish_') ? '? nullish' : fn.startsWith('present_nullable_') ? '? nullable' : ''}`
    else expr = fn.includes('enum') ? 'enum(…)' : fn.replace(/_member$/, '')
    // a // note directly above this call (capped at 2 contiguous lines)
    const callLine = combined.lineOf(m.index)
    const run = commentRunAbove(fileRaw.split('\n'), callLine, 2)
    const note = run.text.length && !run.trimmed ? run.text.join(' ') : ''
    fields.push({ name: key, expr, note, offset: m.index })
    callRe.lastIndex = parenIdx
  }
  if (!fields.length) {
    // The handler READS a body but validates it imperatively (`obj.get("k")`
    // dispatch, element-wise array walks) instead of through the member
    // vocabulary. That's a real body — say so rather than rendering "no body".
    const readsBody =
      /body\s*:\s*axum::body::Bytes/.test(sigText) ||
      /(?:crate::body::)?parse\(\s*&\s*body\s*\)/.test(text) ||
      /from_slice/.test(text)
    return readsBody ? { kind: 'imperative' } : null
  }
  // A shape-dispatched union (`if obj.contains_key("statusKey") { … } else { … }`)
  // renders as variants, exactly like the TS z.union it replaced.
  const armIf = new RegExp(`if\\s+${objVar}\\.contains_key\\(`).exec(text)
  if (armIf) {
    const parenIdx = text.indexOf('(', armIf.index)
    const parenClose = matchDelimRust(text, parenIdx)
    const ifOpen = text.indexOf('{', parenClose)
    const ifClose = matchDelimRust(text, ifOpen)
    const elseMatch = /\belse\s*\{/.exec(text.slice(ifClose))
    if (ifClose !== -1 && elseMatch) {
      const elseOpen = ifClose + elseMatch.index + elseMatch[0].length - 1
      const elseClose = matchDelimRust(text, elseOpen)
      const inIf = fields.filter((f) => f.offset > ifOpen && f.offset < ifClose)
      const inElse = fields.filter((f) => f.offset > elseOpen && f.offset < elseClose)
      const outside = fields.filter((f) => !inIf.includes(f) && !inElse.includes(f))
      if ((inIf.length || inElse.length) && !outside.length) {
        const strip = ({ name, expr, note }) => ({ name, expr, note })
        return { kind: 'union', branches: [inIf.map(strip), inElse.map(strip)].filter((b) => b.length).map((b) => ({ kind: 'fields', fields: b })) }
      }
    }
  }
  return { kind: 'fields', fields: fields.map(({ name, expr, note }) => ({ name, expr, note })) }
}

/** The handler's success `json!({…})` literal, top-level keys; `…` otherwise.
 *  Rust handlers return errors EARLY and success at the TAIL, and error
 *  payloads carry domain keys (`{ jobId }` on a miss), so "first non-error
 *  literal" misfires — the last literal with no `error` key is the
 *  convention-correct success shape. */
function returnsShapeRust(handlerStripped) {
  const out = []
  let i = 0
  while (true) {
    const idx = handlerStripped.indexOf('json!(', i)
    if (idx === -1) break
    let j = idx + 6
    while (j < handlerStripped.length && /\s/.test(handlerStripped[j])) j++
    // `after: Some(json!({…}))` / `before:` — an audit trail's recorded diff,
    // not a response body.
    const look = handlerStripped.slice(Math.max(0, idx - 40), idx)
    const isAudit = /(?:after|before)\s*:\s*Some\s*\(\s*$/.test(look)
    if (handlerStripped[j] === '{' && !isAudit) {
      const close = matchDelimRust(handlerStripped, j)
      const keys = splitTop(handlerStripped.slice(j + 1, close))
        .map((p) => /^\s*(?:"((?:[^"\\]|\\.)*)"|([A-Za-z_$][\w$]*))\s*:/.exec(p))
        .filter(Boolean)
        .map((m) => m[1] ?? m[2])
      out.push(keys)
    }
    i = idx + 6
  }
  const success = [...out].reverse().find((k) => k.length && !k.includes('error'))
  if (success) return success.length ? '{' + success.join(', ') + '}' : '{}'
  return '…'
}

const STATUS_NAMES = {
  OK: 200, CREATED: 201, ACCEPTED: 202, NO_CONTENT: 204,
  MOVED_PERMANENTLY: 301, FOUND: 302, SEE_OTHER: 303, TEMPORARY_REDIRECT: 307, PERMANENT_REDIRECT: 308,
  BAD_REQUEST: 400, UNAUTHORIZED: 401, FORBIDDEN: 403, NOT_FOUND: 404, METHOD_NOT_ALLOWED: 405,
  NOT_ACCEPTABLE: 406, CONFLICT: 409, GONE: 410, PRECONDITION_FAILED: 412, PAYLOAD_TOO_LARGE: 413,
  UNSUPPORTED_MEDIA_TYPE: 415, RANGE_NOT_SATISFIABLE: 416, UNPROCESSABLE_ENTITY: 422, TOO_MANY_REQUESTS: 429,
  INTERNAL_SERVER_ERROR: 500, NOT_IMPLEMENTED: 501, BAD_GATEWAY: 502, SERVICE_UNAVAILABLE: 503, GATEWAY_TIMEOUT: 504,
}

/** SSE streams are built by `crate::realtime`'s `*_event_stream` builders
 *  (the `text/event-stream` string lives there, not in the route module). */
const isSse = (text) => /\b\w*event_stream\s*\(|text\/event-stream|\bSse<|Body::from_stream\(/.test(text)

function statusListRust(handlerStripped, sse) {
  const codes = []
  let varies = false
  for (const m of handlerStripped.matchAll(/StatusCode::([A-Z_]+)/g)) {
    if (m[1] in STATUS_NAMES) codes.push(String(STATUS_NAMES[m[1]]))
    else varies = true
  }
  if (/json!\(|Json\(/.test(handlerStripped) && !codes.includes('200')) codes.push('200')
  // An event-stream response is a 200 — the stream itself carries the rest.
  if (sse && !codes.includes('200')) codes.push('200')
  // `unauthorized()` is session.rs's 401 helper — the one status that moved
  // out of the handler body in the port; read it like the literal it wraps.
  // Other guard refusals (`Err(gate) => return gate`) are the envelope's
  // floor and are not repeated on every row, same convention as the TS docs.
  if (/\bunauthorized\(\)/.test(handlerStripped)) codes.push('401')
  const set = [...new Set(codes)].sort()
  let s = set.slice(0, 6).join(', ')
  if (set.length > 6) s += ', …'
  if (varies) s += (s ? ' + ' : '') + 'varies'
  return s || null
}

/** Extract one handler module's route rows. */
function extractRustRoute(file, entries) {
  const raw = readFileSync(file, 'utf8')
  const rawLines = raw.split('\n')
  const stripped = stripRustComments(raw)
  const locals = localFns(stripped)
  const headerRun = (() => {
    const run = []
    for (const l of rawLines) {
      const t = l.trim()
      if (t.startsWith('//')) run.push(t.replace(/^\/\/ ?/, ''))
      else break
    }
    return run
  })()
  let note = { text: [], trimmed: false, source: 'none' }
  if (headerRun.length && /^\s*doc:/.test(headerRun[0])) {
    note = { text: headerRun.map((l) => l.replace(/^\s*doc:\s?/, '')), trimmed: false, source: 'doc' }
  } else if (headerRun.length) {
    note = { text: headerRun.slice(0, 4), trimmed: headerRun.length > 4, source: 'source' }
  }

  const methods = []
  const handlerNames = new Set(entries.map((e) => e.fnPath.split('::').pop()))
  for (const e of entries.sort((a, b) => METHODS.indexOf(a.method) - METHODS.indexOf(b.method))) {
    const fnName = e.fnPath.split('::').pop()
    const span = findFnSpan(stripped, fnName)
    if (!span) throw new Error(`no fn ${e.fnPath} in ${relative(ROOT, file)}`)
    const combined = handlerWithHelpers(stripped, span, locals, 2, new Set(), handlerNames)
    const sigEnd = stripped.indexOf('{', span.start - 1)
    const sigText = stripped.slice(Math.max(0, span.start - 600), sigEnd === -1 ? span.start : sigEnd)
    const fnLine = lineOf(stripped, stripped.search(new RegExp(`\\bfn\\s+${fnName}\\b`)))
    const run = commentRunAbove(rawLines, fnLine)
    let methodNote = []
    if (run.text.length && /^\s*doc:/.test(run.text[0])) methodNote = run.text.map((l) => l.replace(/^\s*doc:\s?/, ''))
    const auth = authClassRust(combined.text, locals, raw)
    const sse = isSse(combined.text)
    methods.push({
      method: e.method,
      ...auth,
      body: bodySchemaRust(combined, sigText, raw),
      returns: returnsShapeRust(stripped.slice(span.start, span.end)),
      statuses: statusListRust(combined.text, sse),
      sse,
      audit: combined.text.includes('log_audit('),
      note: methodNote,
    })
  }
  return { file: relative(ROOT, file), path: null, methods, note }
}

// ── TS resident extraction ──────────────────────────────────────────────────
//
// healthz, admin.update and the app dispatch still serve from TS; their rows
// come from their route files exactly as the pre-cutover extractor read them.

function extractRoute(file) {
  const raw = readFileSync(file, 'utf8')
  const lines = raw.split('\n')

  const pathMatch = /defineApi\(\s*'([^']+)'/.exec(raw)
  if (!pathMatch) throw new Error(`no defineApi('…') literal in ${relative(ROOT, file)}`)
  const path = pathMatch[1]

  const sraw = stripComments(raw)
  const openParen = sraw.indexOf('(', pathMatch.index)
  const objOpen = sraw.indexOf('{', openParen)
  const objClose = matchDelim(sraw, objOpen)
  const handlers = sraw.slice(objOpen + 1, objClose)

  const methodSpans = []
  {
    let depth = 0
    let i = 0
    while (i < handlers.length) {
      const c = handlers[i]
      if (c === "'" || c === '`') {
        const q = c
        i++
        while (i < handlers.length && handlers[i] !== q) i += handlers[i] === '\\' ? 2 : 1
      } else if ('({['.includes(c)) depth++
      else if (')}]'.includes(c)) depth--
      else if (depth === 0) {
        const m = /^(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s*:/y.exec(handlers.slice(i))
        if (m) {
          const valueStart = i + m[0].length
          let j = valueStart
          let d2 = 0
          let end = handlers.length
          while (j < handlers.length) {
            const c2 = handlers[j]
            if (c2 === "'" || c2 === '`') {
              const q2 = c2
              j++
              while (j < handlers.length && handlers[j] !== q2) j += handlers[j] === '\\' ? 2 : 1
            } else if ('({['.includes(c2)) d2++
            else if (')}]'.includes(c2)) d2--
            else if (d2 === 0) {
              const m2 = /^(GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\s*:/y.exec(handlers.slice(j))
              if (m2) {
                end = j
                break
              }
            }
            j++
          }
          methodSpans.push({ method: m[1], value: handlers.slice(valueStart, end), keyOffset: i })
          i += m[0].length
          continue
        }
      }
      i++
    }
  }

  const exportLine = lines.findIndex((l) => /export const Route/.test(l))
  let pathNote = { text: [], trimmed: false, source: 'none' }
  if (exportLine > 0) {
    const run = commentRunAbove(lines, exportLine)
    const docRun = run.text
    if (docRun.length && /^\s*doc:/.test(docRun[0])) {
      pathNote = { text: docRun.map((l) => l.replace(/^\s*doc:\s?/, '')), trimmed: run.trimmed, source: 'doc' }
    } else if (docRun.length) {
      pathNote = { text: docRun.slice(0, 4), trimmed: run.text.length > 4, source: 'source' }
    }
  }

  const methods = methodSpans.map((span) => {
    const valueRaw = span.value
    const valueClean = stripComments(valueRaw)
    const keyLine =
      raw.slice(0, objOpen + 1).split('\n').length - 1 + handlers.slice(0, span.keyOffset).split('\n').length - 1
    const allLines = raw.split('\n')
    let note = []
    {
      const run = commentRunAbove(allLines, keyLine)
      if (run.text.length && /^\s*doc:/.test(run.text[0])) note = run.text.map((l) => l.replace(/^\s*doc:\s?/, ''))
    }
    return {
      method: span.method,
      ...authClassTS(valueClean, valueRaw),
      body: bodySchemaTS(valueRaw, sraw),
      returns: returnsShapeTS(valueRaw),
      statuses: statusListTS(valueRaw, valueClean),
      sse: valueRaw.includes('text/event-stream'),
      audit: valueClean.includes('logAudit('),
      note,
    }
  })

  return { file: relative(ROOT, file), path, methods, note: pathNote }
}

const KNOWN_REQUEST_TAKERS = new Set([
  'requireUser', 'requireAdmin', 'requirePerm', 'requireView', 'requireAgent',
  'agentCaller', 'fleetCaller', 'checkFleetKey', 'authenticateKey',
  'actingUser', 'getSessionUser', 'updateSessionUser', 'taskActor',
  'parseBody', 'destroySession',
])

function authClassTS(clean, methodRaw) {
  const has = (s) => clean.includes(s)
  const perms = [...clean.matchAll(/(?:requirePerm\(request,\s*|hasPerm\(\s*[\w.]+,\s*)'([\w.-]+)'/g)].map((m) => m[1])
  const view = clean.match(/requireView\(\s*request,\s*'([^']+)'/)
  if (has('fleetCaller(') || has('checkFleetKey(')) return { auth: 'fleet' }
  if (has('authenticateKey(')) return { auth: 'bearer-key' }
  if (has('requireAgent(')) return { auth: 'agent' }
  const session = has('requireUser(') || has('requireAdmin(') || has('requirePerm(') || has('requireView(')
  if (has('actingUser(') || has('taskActor(') || has('commentReader(') || has('commentAuthor(')) return { auth: 'dual' }
  if (has('agentCaller(')) return { auth: session ? 'dual' : 'agent' }
  if (perms.length) return { auth: 'session', perms: [...new Set(perms)] }
  if (has('requireAdmin(')) return { auth: 'admin' }
  if (view) return { auth: 'session', view: view[1] }
  if (has('requireUser(') || has('requirePerm(') || has('getSessionUser(') || has('updateSessionUser(') || has('destroySession(')) return { auth: 'session' }
  const caller = /await\s+([A-Za-z_$][\w$]*)\(\s*request/.exec(methodRaw)
  if (caller && !KNOWN_REQUEST_TAKERS.has(caller[1])) return { auth: `unknown(${caller[1]})` }
  return { auth: 'public' }
}

function spanEnd(text, start) {
  let depth = 0
  let i = start
  while (i < text.length) {
    const c = text[i]
    if (c === "'" || c === '`') {
      const q = c
      i++
      while (i < text.length && text[i] !== q) i += text[i] === '\\' ? 2 : 1
    } else if ('({['.includes(c)) depth++
    else if (')}]'.includes(c)) {
      depth--
      if (depth === 0) return i + 1 - start
    } else if ((c === ',' || c === ';') && depth === 0) return i - start
    i++
  }
  return text.length - start
}

function bodySchemaTS(methodRaw, fileRaw) {
  const idx = methodRaw.indexOf('parseBody(')
  if (idx === -1) return null
  const openParen = idx + 'parseBody('.length - 1
  const closeParen = matchDelim(methodRaw, openParen)
  const args = methodRaw.slice(openParen + 1, closeParen)
  const rest = args.replace(/^\s*request\s*,\s*/, '')
  const schemaText = rest.trim()
  if (!schemaText) return null
  if (/^[A-Za-z_$][\w$]*$/.test(schemaText)) {
    const def = new RegExp(`const ${schemaText}\\s*=\\s*`).exec(fileRaw)
    if (!def) return { kind: 'opaque', name: schemaText }
    const start = def.index + def[0].length
    const body = fileRaw.slice(start, start + spanEnd(fileRaw, start))
    return schemaFromText(body, fileRaw)
  }
  return schemaFromText(schemaText, fileRaw)
}

function schemaFromText(text, fileRaw) {
  const t = text.trim()
  const obj = /z\.(?:looseObject|strictObject|object)\(\s*\{/.exec(t)
  if (obj && obj.index === 0) {
    const open = t.indexOf('{', obj.index + obj[0].length - 1)
    return { kind: 'fields', fields: parseFields(t.slice(open + 1, matchDelim(t, open))) }
  }
  const un = /^z\.union\(\s*\[/.exec(t)
  if (un) {
    const open = t.indexOf('[', un.index)
    const inner = t.slice(open + 1, matchDelim(t, open))
    const branches = splitTop(inner).map((b) => {
      const bt = b.trim()
      if (/^[A-Za-z_$][\w$]*$/.test(bt)) {
        const def = new RegExp(`const ${bt}\\s*=\\s*`).exec(fileRaw)
        if (def) {
          const s = def.index + def[0].length
          return { name: bt, ...schemaFromText(fileRaw.slice(s, s + spanEnd(fileRaw, s)), fileRaw) }
        }
        return { name: bt, kind: 'opaque' }
      }
      return schemaFromText(bt, fileRaw)
    })
    return { kind: 'union', branches }
  }
  return { kind: 'opaque', expr: oneline(t) }
}

function parseFields(body) {
  const bodyLines = body.split('\n')
  return splitTop(body)
    .map((part) => {
      const m = /^\s*(?:'([^']+)'|"([^"]+)"|([A-Za-z_$][\w$]*))\s*:\s*([\s\S]+)$/.exec(part)
      if (!m) return null
      const name = m[1] ?? m[2] ?? m[3]
      const expr = oneline(m[4])
      let line = 0
      for (let i = 0; i < part.split('\n').length - 1; i++) line++
      const before = body.slice(0, body.indexOf(part))
      const beforeLines = before.split('\n').length - 1
      const run = commentRunAbove(bodyLines, beforeLines, 2)
      const note = run.text.length && !run.trimmed ? run.text.join(' ') : ''
      return { name, expr, note }
    })
    .filter(Boolean)
}

function returnsShapeTS(methodRaw) {
  const out = []
  let i = 0
  while (true) {
    const idx = methodRaw.indexOf('json(', i)
    if (idx === -1) break
    let j = idx + 5
    while (j < methodRaw.length && /\s/.test(methodRaw[j])) j++
    if (methodRaw[j] === '{') {
      const close = matchDelim(methodRaw, j)
      const keys = splitTop(methodRaw.slice(j + 1, close))
        .map((p) => {
          const kv = /^\s*(?:'([^']+)'|"([^"]+)"|([A-Za-z_$][\w$]*))\s*:/.exec(p)
          if (kv) return kv[1] ?? kv[2] ?? kv[3]
          // shorthand property (`checks,`) — the key is the value's name
          const sh = /^\s*([A-Za-z_$][\w$]*)\s*,?$/.exec(p)
          return sh ? sh[1] : null
        })
        .filter(Boolean)
      out.push(keys)
    }
    i = idx + 5
  }
  const success = out.find((k) => k.length && !(k.length === 1 && k[0] === 'error'))
  if (success) return success.length ? '{' + success.join(', ') + '}' : '{}'
  return '…'
}

function statusListTS(raw, clean) {
  const codes = [...raw.matchAll(/status:\s*(\d{3})/g)].map((m) => m[1])
  const varies = /status:\s*[^0-9\s]/.test(clean)
  if (/json\(|new Response\(/.test(raw) && !codes.includes('200')) codes.push('200')
  const set = [...new Set(codes)].sort()
  let s = set.slice(0, 6).join(', ')
  if (set.length > 6) s += ', …'
  if (varies) s += (s ? ' + ' : '') + 'varies'
  return s || null
}

// ── Rendering ───────────────────────────────────────────────────────────────

const AUTH_LEGEND = `**Auth vocabulary** (the route's guard class — resource-level ACLs like board
membership or ownership apply on top; see [API-CONVENTIONS.md](../API-CONVENTIONS.md)):

| class | meaning |
| :--- | :--- |
| \`public\` | no authentication |
| \`session\` | any signed-in member (\`require_user\`) |
| \`session\` + \`perm:x\` | signed-in member holding permission \`x\` (\`require_perm\`) |
| \`session\` + \`view:p\` | signed-in member granted view \`p\` (\`require_view\`) |
| \`admin\` | an admin session (\`require_admin\`) |
| \`agent\` | an agent credential (\`tak_\` key, \`require_agent\`/\`agent_caller\`) |
| \`dual\` | session path and agent path both reach the handler |
| \`fleet\` | internal fleet key (\`fleet_caller\`/\`check_fleet_key\`) |
| \`bearer-key\` | a personal LLM-gateway API key (\`authenticate_key\`) |`

const GENERATED_BANNER = (extra = '') =>
  `> **Generated** by \`bun run docs:api\` from the Rust router table (\`api/src/routes/mod.rs\`)\n` +
  `> and the handler modules under \`api/src/routes/**\` (the TS residents still serving\n` +
  `> \`healthz\`, \`admin/update\` and the app dispatch excepted) — do not edit by hand.\n` +
  `> Change the route (or its \`// doc:\` note) and regenerate; \`bun run check\` fails on drift.\n` +
  `> The **Returns** column is the first success-shaped \`json!({…})\` literal and is heuristic —\n` +
  `> \`…\` means the shape is not a literal in source.` +
  (extra ? '\n' + extra : '')

const prettyPath = (p) =>
  p.replace(/\$(\w+)/g, '{$1}').replace(/\/\$$/, '/*').replace('$', '')

function authCell(m) {
  let a = '`' + m.auth + '`'
  if (m.auth === 'session' && m.perms) a += ' + ' + m.perms.map((p) => `\`perm:${p}\``).join(' ')
  if (m.auth === 'session' && m.view) a += ` + \`view:${m.view}\``
  return a
}

function renderSchemaTable(schema, heading) {
  if (!schema) return []
  const out = []
  if (schema.kind === 'fields') {
    out.push(`### ${heading}`, '', '| field | schema | notes |', '| :--- | :--- | :--- |')
    for (const f of schema.fields) out.push(`| \`${f.name}\` | \`${f.expr}\` | ${f.note} |`)
    out.push('')
  } else if (schema.kind === 'union') {
    schema.branches.forEach((b, i) => {
      const label = b.name ? `${heading} — variant ${i + 1} (\`${b.name}\`)` : `${heading} — variant ${i + 1}`
      if (b.kind === 'fields') out.push(...renderSchemaTable(b, label))
      else out.push(`### ${label}`, '', `Schema not an object literal in the route file (${b.kind === 'opaque' ? b.name || b.expr || 'external schema' : 'unrecognised form'}) — see the route source.`, '')
    })
  } else if (schema.kind === 'opaque') {
    out.push(`### ${heading}`, '', `Body schema \`${schema.name || schema.expr}\` is not an object literal in the route file — see the route source.`, '')
  } else if (schema.kind === 'imperative') {
    out.push(`### ${heading}`, '', 'Body is validated imperatively (`obj.get` dispatch / element-wise walks), not\nthrough the `crate::body` member vocabulary — the field set lives in the route\nsource.', '')
  }
  return out
}

function renderGroup(name, routes) {
  const isLlm = name === 'llm'
  const lines = []
  lines.push(`# API reference — ${name}`, '')
  lines.push(GENERATED_BANNER(isLlm ? '> \n> **This group is the OpenAI-compatible wire** (`llm.v1.*`): external\n> clients speak OpenAI shapes here, NOT house conventions — see the source\n> before building against it.' : ''))
  lines.push('')
  lines.push(routes.length + (routes.length === 1 ? ' route.' : ' routes.'), '')

  lines.push('| Route | Method | Auth |', '| :--- | :--- | :--- |')
  for (const r of routes) {
    for (const m of r.methods) {
      lines.push(`| [\`${prettyPath(r.path)}\`](${anchor(prettyPath(r.path))}) | ${m.method} | ${authCell(m)} |`)
    }
  }
  lines.push('')

  for (const r of routes) {
    const pp = prettyPath(r.path)
    lines.push(`## \`${pp}\``, '')
    lines.push(`Source: [\`${r.file}\`](../../${r.file})`, '')
    if (r.note.text.length) {
      lines.push(...r.note.text.map((l) => (l ? '> ' + l : '>')))
      if (r.note.trimmed) lines.push('> …')
      lines.push('')
    }
    lines.push('| Method | Auth | Body | Returns | Status | Flags |', '| :--- | :--- | :--- | :--- | :--- | :--- |')
    for (const m of r.methods) {
      const flags = [m.audit ? 'audit' : '', m.sse ? 'SSE' : ''].filter(Boolean).join(' ') || '—'
      const body = m.body ? '[body](#' + anchor(`${m.method} ${pp} body`).replace(/^#/, '') + ')' : '—'
      lines.push(`| ${m.method} | ${authCell(m)} | ${body} | \`${m.returns}\` | ${m.statuses ?? '—'} | ${flags} |`)
    }
    lines.push('')
    if (!isLlm) {
      for (const m of r.methods) {
        if (m.note.length) {
          lines.push(`**${m.method}** — ${m.note.join(' ')}`, '')
        }
        if (m.body) lines.push(...renderSchemaTable(m.body, `${m.method} \`${pp}\` body`))
      }
    }
  }
  return lines.join('\n').replace(/\n{3,}/g, '\n\n') + '\n'
}

function renderApiIndex(groupRoutes) {
  const lines = []
  lines.push('# API reference', '')
  lines.push('> **Generated** by `bun run docs:api` — do not edit by hand. One file per resource')
  lines.push('> group, one row per (path, method), extracted from the Rust router')
  lines.push('> (`api/src/routes/mod.rs`) and handler modules (`api/src/routes/**`).')
  lines.push('> Requests/responses follow the house envelope and conventions:')
  lines.push('> [API-CONVENTIONS.md](../API-CONVENTIONS.md).')
  lines.push('')
  const total = Object.values(groupRoutes).reduce((n, rs) => n + rs.length, 0)
  lines.push(`${total} routes across ${Object.keys(groupRoutes).length} groups.`, '')
  lines.push('| Group | Covers | Routes |', '| :--- | :--- | :--- |')
  for (const g of Object.keys(groupRoutes).sort()) {
    lines.push(`| [\`${g}\`](./${g}.md) | ${GROUPS[g].blurb} | ${groupRoutes[g].length} |`)
  }
  lines.push('')
  lines.push(AUTH_LEGEND, '')
  lines.push('---', '')
  lines.push('The **Returns** column everywhere in this reference is a heuristic: the top-level')
  lines.push('keys of the first success-shaped `json!({…})` literal in the handler. `…` means the')
  lines.push('shape is computed, not literal. Where a row matters to you, the source link at the')
  lines.push('top of its section is the truth.')
  lines.push('')
  return lines.join('\n')
}

// ── CLI pass ────────────────────────────────────────────────────────────────
// Not parsed — IMPORTED. Bun runs TypeScript; the tree the CLI renders its own
// help from is the tree we render the reference from. Drift is impossible by
// construction; --check only guards the file on disk.

async function renderCliReference() {
  const { tree } = await import('../cli/src/cmd/index.ts')
  const lines = []
  lines.push('# CLI reference — `talaria`', '')
  lines.push('> **Generated** by `bun run docs:api` from `cli/src/cmd/**` — the same declarations')
  lines.push('> `--help` renders. Do not edit by hand; \`bun run check\` fails on drift.')
  lines.push('> The guide (when to use what): [CLI.md](./CLI.md).')
  lines.push('')

  const flat = []
  const walk = (node, prefix) => {
    const path = [...prefix, node.name]
    if (node.kind === 'leaf') flat.push({ path, leaf: node })
    else for (const c of node.children) walk(c, path)
  }
  for (const c of tree.children) walk(c, [tree.name])

  lines.push('| Command | Summary |', '| :--- | :--- |')
  for (const { path, leaf } of flat) {
    lines.push(`| [\`${path.join(' ')}\`](${anchor(path.join(' '))}) | ${leaf.summary} |`)
  }
  lines.push('')

  const groups = tree.children.filter((c) => c.kind === 'group')
  for (const g of groups) {
    lines.push(`## \`${[tree.name, g.name].join(' ')}\``, '', `${g.summary}`, '')
    for (const c of g.children) {
      if (c.kind === 'leaf') lines.push(...leafSection([tree.name, g.name], c))
      else for (const c2 of c.children) if (c2.kind === 'leaf') lines.push(...leafSection([tree.name, g.name, c2.name], c2))
    }
  }
  for (const c of tree.children) {
    if (c.kind === 'leaf') lines.push(...leafSection([tree.name], c))
  }
  return lines.join('\n').replace(/\n{3,}/g, '\n\n') + '\n'
}

function leafSection(prefix, leaf) {
  const out = []
  const path = [...prefix, leaf.name].join(' ')
  out.push(`### \`${path}\``, '')
  if (leaf.aliases?.length) out.push(`Aliases: ${leaf.aliases.map((a) => `\`${a}\``).join(', ')}`, '')
  out.push(leaf.summary, '')
  if (leaf.usage) out.push('```', leaf.usage, '```', '')
  if (leaf.positionals) {
    out.push(`Positional \`<${leaf.positionals.name}>\`${leaf.positionals.required ? ' (required)' : ''}${leaf.positionals.multiple ? ' (takes the rest of the arguments)' : ''}${leaf.positionals.desc ? ` — ${leaf.positionals.desc}` : ''}`, '')
  }
  if (leaf.flags?.length) {
    out.push('| Flag | Kind | Default | Description |', '| :--- | :--- | :--- | :--- |')
    for (const f of leaf.flags) {
      out.push(`| \`--${f.name}\`${f.short ? ` \`-${f.short}\`` : ''} | ${f.kind} | ${f.default ?? '—'} | ${f.desc} |`)
    }
    out.push('')
  }
  return out
}

// ── Main ────────────────────────────────────────────────────────────────────

const modText = readFileSync(ROUTES_MOD, 'utf8')
const table = parseRouterTable(modText)

// Group the table's entries per path (a path's methods may live in several
// modules in principle; in practice one — but the grouping makes that
// explicit rather than assumed).
const byModule = new Map()
for (const t of table) {
  const key = t.entries.map((e) => moduleFile(e.fnPath)).join('|')
  if (!byModule.has(key)) byModule.set(key, { file: key.split('|')[0], entries: [], path: t.path })
  byModule.get(key).entries.push(...t.entries)
}

const routes = []
for (const { file, entries, path } of byModule.values()) {
  const abs = join(ROOT, file)
  if (!existsSync(abs)) throw new Error(`router names ${file} but no such module file exists`)
  const r = extractRustRoute(abs, entries)
  r.path = path
  routes.push(r)
}

// Completeness: every handler-looking module under api/src/routes/** that the
// router never references gets named, so a forgotten .route() is visible.
{
  const routed = new Set(routes.map((r) => r.file))
  const walk = (dir) => {
    for (const e of readdirSync(dir, { withFileTypes: true })) {
      if (e.isDirectory()) walk(join(dir, e.name))
      else if (e.name.endsWith('.rs') && e.name !== 'mod.rs') {
        const rel = relative(ROOT, join(dir, e.name))
        if (!routed.has(rel)) warnings.push(`unrouted module: ${rel} defines handlers but no .route( names it (helper module, or a missing registration?)`)
      }
    }
  }
  walk(ROUTES_DIR)
}

// The TS residents, extracted exactly as before.
const residentRoutes = TS_RESIDENT_FILES.map((f) => extractRoute(join(ROOT, f)))

// A resident CLAIMS its path: the SPA host's proxy serves it, so the public
// contract is the TS row even where the Rust router registers a twin (the
// Rust /api/healthz answers direct-binary checks on :5274 — an internal
// detail, not the documented surface). The app-MCP branch claims its prefix
// the same way, though it has never had a row of its own.
const residentClaims = (p) =>
  residentRoutes.some((r) => r.path === p) || p.startsWith('/api/mcp/gw/app-')
for (let i = routes.length - 1; i >= 0; i--) {
  if (residentClaims(routes[i].path)) {
    warnings.push(`resident wins over router twin: ${routes[i].path} is served by the SPA host's TS resident; the Rust twin (${routes[i].file}) is dropped from the reference`)
    routes.splice(i, 1)
  }
}
routes.push(...residentRoutes)

// apps.$app.$ — one dispatch row, not five method rows: everything about the
// app-server gateway contract lives in the SDK doc.
const APPS_DISPATCH = '/api/apps/$app/$'

const groupRoutes = {}
for (const r of routes) {
  const seg = r.path.replace(/^\/api\//, '').split('/')[0]
  const g = SEG_TO_GROUP[seg]
  if (!g) throw new Error(`no GROUPS entry for first segment "${seg}" (path ${r.path}, file ${r.file}) — add one to scripts/gen-docs.mjs`)
  ;(groupRoutes[g] ??= []).push(r)
}
if (routes.some((r) => r.path === APPS_DISPATCH)) {
  const r = routes.find((x) => x.path === APPS_DISPATCH)
  r.methods = [{ method: 'ANY', auth: 'session', body: null, returns: 'app-defined', statuses: null, sse: false, audit: false, note: [] }]
  r.note = { text: ['The app-server gateway: `/api/apps/<slug>/*` dispatches into the app\'s own `server.ts`.', 'The host authenticates, checks the app is enabled and the user may reach it, then hands over a context (user, sub-path, namespaced store). The contract: the SDK doc.'], trimmed: false, source: 'doc' }
}

// `$param` renders as `{param}`; trailing `$` as `*` — but sort by raw path.
for (const g of Object.keys(groupRoutes)) {
  groupRoutes[g].sort((a, b) => a.path.localeCompare(b.path) || a.file.localeCompare(b.file))
}

const outputs = new Map()
for (const [g, rs] of Object.entries(groupRoutes)) outputs.set(join(ROOT, `docs/api/${g}.md`), renderGroup(g, rs))
outputs.set(join(ROOT, 'docs/api/README.md'), renderApiIndex(groupRoutes))
outputs.set(join(ROOT, 'docs/CLI-REFERENCE.md'), await renderCliReference())

// ── Write or check ──────────────────────────────────────────────────────────

const problems = []
const onDisk = new Set()
for (const e of readdirSync(join(ROOT, 'docs/api'), { withFileTypes: true })) {
  if (e.isFile() && e.name.endsWith('.md')) onDisk.add(join(ROOT, 'docs/api', e.name))
}

for (const [file, content] of outputs) {
  const rel = relative(ROOT, file)
  if (CHECK) {
    if (!existsSync(file)) problems.push(`${rel}: missing (run \`bun run docs:api\`)`)
    else if (readFileSync(file, 'utf8') !== content) problems.push(`${rel}: out of date (run \`bun run docs:api\`)`)
    onDisk.delete(file)
  } else {
    writeFileSync(file, content)
    onDisk.delete(file)
  }
}
for (const stale of onDisk) problems.push(`${relative(ROOT, stale)}: stale generated file (no group produces it — delete it)`)

if (problems.length) {
  const BAR = '─'.repeat(78)
  console.error(`\n${BAR}\nFAIL  generated-docs-drift\n${BAR}`)
  console.error('  generated reference files that do not match what the source says:\n')
  for (const p of problems) console.error(`    ${p}`)
  console.error('')
  console.error('  WHAT TO DO INSTEAD:')
  console.error('    Never edit docs/api/** or docs/CLI-REFERENCE.md by hand — they are generated.')
  console.error('    Change the source (route, // doc: note, or CLI declaration), then run')
  console.error('    `bun run docs:api` and commit the result with your change.')
  console.error(`\n${BAR}\n`)
  process.exit(1)
}

const methodRows = routes.reduce((n, r) => n + r.methods.length, 0)
for (const r of routes) {
  for (const m of r.methods) {
    if (m.auth.startsWith('unknown(')) console.warn(`gen-docs: ${r.file} ${m.method} uses an unrecognized guard → ${m.auth} (add it to KNOWN_HEADER_TAKERS/authClassRust in scripts/gen-docs.mjs)`)
  }
}
for (const w of warnings) console.warn(`gen-docs: ${w}`)
console.log(`gen-docs: ${routes.length} routes (${methodRows} method rows) → ${outputs.size} generated files${CHECK ? ', all current' : ''}`)
