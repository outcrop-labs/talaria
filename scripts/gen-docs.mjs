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
// WHAT IS EXACT vs HEURISTIC. Path literals, methods, auth guards, parseBody
// field tables, audit/SSE markers and literal status codes are read from the
// route source (see extractors below). The Returns column is a heuristic — the
// top-level keys of the first success-shaped `json({...})` literal, `…` when
// the shape isn't a literal — and is marked as such in every file's banner.
// Where the extractor would have to guess, it prints `…` instead. Never trust
// a doc over the code; this tool exists to make that cheap.
//
// USAGE
//   bun scripts/gen-docs.mjs          write all generated files
//   bun scripts/gen-docs.mjs --check  diff against disk; exit 1 naming drift
//
// NOTES COME FROM THE SOURCE. A route may carry `// doc:` comment runs —
// directly above `export const Route` for a path-level note, directly above a
// method key for a method note. They render verbatim. Without one, the path
// note falls back to the route's own leading comment block (capped). Prose
// about a route lives with the route or nowhere.

import { readdirSync, readFileSync, writeFileSync, existsSync, statSync } from 'node:fs'
import { join, resolve, relative } from 'node:path'

const ROOT = resolve(import.meta.dirname, '..')
const ROUTES_DIR = join(ROOT, 'ui/src/routes/api')
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

/** Strip comments, LENGTH-PRESERVING — every comment character becomes a
 *  space (newlines kept), so offsets and line numbers into the original text
 *  stay valid. Signal detection must not be fooled by a guard named inside a
 *  comment, and the method-span scanner must not be fooled by the backticks
 *  and apostrophes comments contain: a scanner that skips strings reads
 *  `` `channels.ts`'s `` as one backticked string plus a stray `'` that
 *  opens a phantom string and swallows a handler. String literals pass
 *  through verbatim (a `//` inside one is not a comment). */
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

/** Collapse whitespace and cap length — zod expressions render verbatim but
 *  never unbounded. */
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
 *  skipped only BEFORE the run starts — a comment separated from the export
 *  by a blank line is still its comment; once the run begins it must be
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

// ── Route extraction ────────────────────────────────────────────────────────

function walkRoutes(dir, acc = []) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    if (e.isDirectory()) walkRoutes(join(dir, e.name), acc)
    else if (e.name.endsWith('.ts')) acc.push(join(dir, e.name))
  }
  return acc
}

function extractRoute(file) {
  const raw = readFileSync(file, 'utf8')
  const lines = raw.split('\n')

  const pathMatch = raw.match(/defineApi\(\s*'([^']+)'/)
  if (!pathMatch) throw new Error(`no defineApi('…') literal in ${relative(ROOT, file)}`)
  const path = pathMatch[1]

  // Handlers are scanned on the comment-stripped text (same length as `raw`,
  // so offsets carry over): comments inside a handler carry backticks and
  // apostrophes that a string-skipping scanner misreads. `lines` stays raw —
  // the `// doc:` notes ARE comments.
  const sraw = stripComments(raw)
  // The handlers object: first { after the defineApi( open paren.
  const openParen = sraw.indexOf('(', pathMatch.index)
  const objOpen = sraw.indexOf('{', openParen)
  const objClose = matchDelim(sraw, objOpen)
  const handlers = sraw.slice(objOpen + 1, objClose)

  // Top-level method keys inside the handlers object. Depth is tracked from
  // the object interior; a method key at any other depth is not a handler.
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
          // find where this method's value ends: next top-level method key or object end
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


  // Path note: `// doc:` run above the Route export, else the route's own
  // leading comment block above it, capped.
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
      ...authClass(valueClean, valueRaw),
      body: bodySchema(valueRaw, sraw),
      returns: returnsShape(valueRaw),
      statuses: statusList(valueRaw, valueClean),
      sse: valueRaw.includes('text/event-stream'),
      audit: valueClean.includes('logAudit('),
      note,
    }
  })

  return { file: relative(ROOT, file), path, methods, note: pathNote }
}

/** The guard vocabulary, in precedence order. `dual` = a session path and an
 *  agent-key path both reach the handler (agentCaller then requireUser, or a
 *  wrapper like actingUser/taskActor that resolves either). The vocabulary is
 *  CLOSED: a method that makes an `await X(request)` call this list doesn't
 *  know renders `unknown(X)` — never a false `public` — and the run logs it,
 *  so a new guard spelling fails loudly until it's added here. */
const KNOWN_REQUEST_TAKERS = new Set([
  'requireUser', 'requireAdmin', 'requirePerm', 'requireView', 'requireAgent',
  'agentCaller', 'fleetCaller', 'checkFleetKey', 'authenticateKey',
  'actingUser', 'getSessionUser', 'updateSessionUser', 'taskActor',
  'parseBody', 'destroySession',
])

function authClass(clean, methodRaw) {
  const has = (s) => clean.includes(s)
  const perms = [...clean.matchAll(/(?:requirePerm\(request,\s*|hasPerm\(\s*[\w.]+,\s*)'([\w.-]+)'/g)].map((m) => m[1])
  const view = clean.match(/requireView\(\s*request,\s*'([^']+)'/)
  if (has('fleetCaller(') || has('checkFleetKey(')) return { auth: 'fleet' }
  if (has('authenticateKey(')) return { auth: 'bearer-key' }
  if (has('requireAgent(')) return { auth: 'agent' }
  const session = has('requireUser(') || has('requireAdmin(') || has('requirePerm(') || has('requireView(')
  // These resolve EITHER a session user or an agent caller → dual.
  if (has('actingUser(') || has('taskActor(') || has('commentReader(') || has('commentAuthor(')) return { auth: 'dual' }
  if (has('agentCaller(')) return { auth: session ? 'dual' : 'agent' }
  if (perms.length) return { auth: 'session', perms: [...new Set(perms)] }
  if (has('requireAdmin(')) return { auth: 'admin' }
  if (view) return { auth: 'session', view: view[1] }
  if (has('requireUser(') || has('requirePerm(') || has('getSessionUser(') || has('updateSessionUser(') || has('destroySession(')) return { auth: 'session' }
  // No known guard. If the method still hands `request` to something we don't
  // recognize, say so instead of claiming public.
  const caller = /await\s+([A-Za-z_$][\w$]*)\(\s*request/.exec(methodRaw)
  if (caller && !KNOWN_REQUEST_TAKERS.has(caller[1])) return { auth: `unknown(${caller[1]})` }
  return { auth: 'public' }
}

/** parseBody schema → { kind: 'fields'|'union'|'opaque', … } or null. */
function bodySchema(methodRaw, fileRaw) {
  const idx = methodRaw.indexOf('parseBody(')
  if (idx === -1) return null
  const openParen = idx + 'parseBody('.length - 1
  const closeParen = matchDelim(methodRaw, openParen)
  const args = methodRaw.slice(openParen + 1, closeParen)
  // drop the `request,` first argument
  const rest = args.replace(/^\s*request\s*,\s*/, '')
  const schemaText = rest.trim()
  if (!schemaText) return null
  if (/^[A-Za-z_$][\w$]*$/.test(schemaText)) {
    // named schema — resolve `const Name = …` in this file
    const def = new RegExp(`const ${schemaText}\\s*=\\s*`).exec(fileRaw)
    if (!def) return { kind: 'opaque', name: schemaText }
    const start = def.index + def[0].length
    const body = fileRaw.slice(start, start + spanEnd(fileRaw, start))
    return schemaFromText(body, fileRaw)
  }
  return schemaFromText(schemaText, fileRaw)
}

/** How far the expression starting at `start` extends: to the first top-level
 *  `,` or `;` or newline-after-close at depth 0. */
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

/** z.object / z.union / anything else → a renderable shape. */
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

/** Object-literal body → field rows: name, zod expression, preceding // note. */
function parseFields(body) {
  const bodyLines = body.split('\n')
  return splitTop(body)
    .map((part) => {
      const m = /^\s*(?:'([^']+)'|"([^"]+)"|([A-Za-z_$][\w$]*))\s*:\s*([\s\S]+)$/.exec(part)
      if (!m) return null
      const name = m[1] ?? m[2] ?? m[3]
      const expr = oneline(m[4])
      // a // note directly above this part's first line
      let line = 0
      for (let i = 0; i < part.split('\n').length - 1; i++) line++ // offset of part start
      const before = body.slice(0, body.indexOf(part))
      const beforeLines = before.split('\n').length - 1
      const run = commentRunAbove(bodyLines, beforeLines, 2)
      const note = run.text.length && !run.trimmed ? run.text.join(' ') : ''
      return { name, expr, note }
    })
    .filter(Boolean)
}

/** First success-shaped `json({…})` literal's top-level keys; `…` otherwise. */
function returnsShape(methodRaw) {
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
        .map((p) => /^\s*(?:'([^']+)'|"([^"]+)"|([A-Za-z_$][\w$]*))\s*:/.exec(p))
        .filter(Boolean)
        .map((m) => m[1] ?? m[2] ?? m[3])
      out.push(keys)
    }
    i = idx + 5
  }
  const success = out.find((k) => k.length && !(k.length === 1 && k[0] === 'error'))
  if (success) return success.length ? '{' + success.join(', ') + '}' : '{}'
  return '…' // no success-shaped literal: computed, or only error literals
}

function statusList(raw, clean) {
  const codes = [...raw.matchAll(/status:\s*(\d{3})/g)].map((m) => m[1])
  const varies = /status:\s*[^0-9\s]/.test(clean)
  // json(...) and bare new Response(...) without a status ARE 200s.
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
| \`session\` | any signed-in member (\`requireUser\`) |
| \`session\` + \`perm:x\` | signed-in member holding permission \`x\` |
| \`session\` + \`view:p\` | signed-in member granted view \`p\` |
| \`admin\` | an admin session (\`requireAdmin\`) |
| \`agent\` | an agent credential (\`tak_\` key, \`requireAgent\`/\`agentCaller\`) |
| \`dual\` | session path and agent path both reach the handler |
| \`fleet\` | internal fleet key (\`fleetCaller\`) |
| \`bearer-key\` | a personal LLM-gateway API key (\`authenticateKey\`) |`

const GENERATED_BANNER = (extra = '') =>
  `> **Generated** by \`bun run docs:api\` from \`ui/src/routes/api/**\` — do not edit by hand.\n` +
  `> Change the route (or its \`// doc:\` note) and regenerate; \`bun run check\` fails on drift.\n` +
  `> The **Returns** column is the first success-shaped \`json({…})\` literal and is heuristic —\n` +
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
      else out.push(`### ${label}`, '', `Schema not a literal object in the route file (${b.kind === 'opaque' ? b.name || b.expr || 'external schema' : 'unrecognised form'}) — see the route source.`, '')
    })
  } else if (schema.kind === 'opaque') {
    out.push(`### ${heading}`, '', `Body schema \`${schema.name || schema.expr}\` is not an object literal in the route file — see the route source.`, '')
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

  // jump table
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
  lines.push('> group, one row per (path, method). Requests/responses follow the house envelope')
  lines.push('> and conventions: [API-CONVENTIONS.md](../API-CONVENTIONS.md).')
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
  lines.push('keys of the first success-shaped `json({…})` literal in the handler. `…` means the')
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

const routes = walkRoutes(ROUTES_DIR)
  .map(extractRoute)
  .sort((a, b) => a.path.localeCompare(b.path) || a.file.localeCompare(b.file))

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
    if (m.auth.startsWith('unknown(')) console.warn(`gen-docs: ${r.file} ${m.method} uses an unrecognized guard → ${m.auth} (add it to KNOWN_REQUEST_TAKERS/authClass in scripts/gen-docs.mjs)`)
  }
}
console.log(`gen-docs: ${routes.length} routes (${methodRows} method rows) → ${outputs.size} generated files${CHECK ? ', all current' : ''}`)
