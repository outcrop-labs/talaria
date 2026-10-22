// sdk-reference — what the two SDK entry points export, and what
// docs/sdk/reference.md claims they export, entry point by entry point.
//
// Pure functions, stdlib only (`bun run check` runs with no node_modules).
// check-docs.mjs wires them to the tree; check-docs.test.mjs pins them.
//
// WHY PER ENTRY AND PER KIND. The first version of this check merged both
// entry points' names into one set and diffed backticked identifiers, so a
// symbol moved from `@talaria/sdk` to `@talaria/sdk/server`, or a component
// re-exported as a type, passed — and an app author importing it the way the
// row says got a build error. The row's claim is three things: the name, the
// entry point it is imported from, and what it is. All three are checked.

/** The kinds a reference row may claim. `value` is not one of them: it is
 *  what the parser reports for a runtime export whose declaration it cannot
 *  see (a re-export from a package in node_modules), and it accepts any of
 *  the value kinds. */
export const ROW_KINDS = ['fn', 'const', 'class', 'comp', 'type']

const ID = '[A-Za-z_$][\\w$]*'

/** Blank out whole-line `//` comments, then block comments, keeping
 *  offsets. Line comments go first: a line comment that mentions a path like
 *  `/api/apps/<slug>/*` would otherwise open a block comment that swallows
 *  the declarations below it. */
export function stripComments(src) {
  return src
    .replace(/^[ \t]*\/\/.*$/gm, (m) => ' '.repeat(m.length))
    .replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\n]/g, ' '))
}

/** What a `const NAME (: T)? = …` initializer is: a function or a constant. */
function constKind(rhs) {
  return /^\s*(?:async\b|function\b|\(|<|[A-Za-z_$][\w$]*\s*=>)/.test(rhs) ? 'fn' : 'const'
}

/** One `{ … }` clause item: `type X`, `X as Y`, `default as Y`, `X`. */
function clauseItems(body) {
  const out = []
  for (const piece of body.split(',')) {
    let t = piece.trim()
    if (!t) continue
    let typeOnly = false
    if (/^type\s+/.test(t)) {
      typeOnly = true
      t = t.replace(/^type\s+/, '')
    }
    const as = new RegExp(`^(${ID})\\s+as\\s+(${ID})$`).exec(t)
    if (as) out.push({ local: as[1], name: as[2], typeOnly })
    else if (new RegExp(`^${ID}$`).test(t)) out.push({ local: t, name: t, typeOnly })
  }
  return out
}

/**
 * Every name a module exports, with its kind, from the declaration spellings
 * the SDK uses: `export (async) function`, `export const`, `export class`,
 * `export interface`, `export type NAME<…> =`, and `export (type) { … }`
 * clauses with or without `from` (aliases, per-item `type`, `default as X`).
 *
 * `resolve(spec)` returns a module's source for a specifier the parser should
 * follow (`@/lib/motion`), or null; a re-export it cannot follow is a `value`
 * (or a `type`, when the clause says so). A `.svelte` default is a `comp`.
 */
export function sdkExports(src, resolve = () => null, depth = 0) {
  const code = stripComments(src)
  const out = new Map()
  const set = (name, kind) => {
    if (!out.has(name)) out.set(name, kind)
  }

  for (const m of code.matchAll(new RegExp(`export\\s+(?:async\\s+)?function\\s*\\*?\\s*(${ID})`, 'g'))) set(m[1], 'fn')
  for (const m of code.matchAll(new RegExp(`export\\s+const\\s+(${ID})\\s*(?::[^=]*?)?=(?!>)(?=([^\\n]*(?:\\n[^\\n]*)?))`, 'g'))) set(m[1], constKind(m[2]))
  for (const m of code.matchAll(new RegExp(`export\\s+const\\s+(${ID})`, 'g'))) set(m[1], 'value')
  for (const m of code.matchAll(new RegExp(`export\\s+(?:abstract\\s+)?class\\s+(${ID})`, 'g'))) set(m[1], 'class')
  for (const m of code.matchAll(new RegExp(`export\\s+interface\\s+(${ID})`, 'g'))) set(m[1], 'type')
  // `export type NAME` — generics and all; `export type { … }` never matches
  // here, because `{` cannot start a name.
  for (const m of code.matchAll(new RegExp(`export\\s+type\\s+(${ID})`, 'g'))) set(m[1], 'type')

  // Local bindings an `export { … }` without `from` may name.
  const local = new Map()
  for (const m of code.matchAll(new RegExp(`import\\s+(${ID})\\s+from\\s+['"]([^'"]+)['"]`, 'g'))) {
    local.set(m[1], m[2].endsWith('.svelte') ? { kind: 'comp' } : { kind: 'value' })
  }
  for (const m of code.matchAll(/import\s+(type\s+)?\{([^}]*)\}\s+from\s+['"]([^'"]+)['"]/g)) {
    for (const item of clauseItems(m[2])) local.set(item.name, { from: m[3], orig: item.local, typeOnly: Boolean(m[1]) || item.typeOnly })
  }
  for (const m of code.matchAll(new RegExp(`(?:^|\\n)\\s*(?:async\\s+)?function\\s+(${ID})`, 'g'))) local.set(m[1], { kind: 'fn' })
  for (const m of code.matchAll(new RegExp(`(?:^|\\n)\\s*const\\s+(${ID})\\s*(?::[^=]*?)?=(?!>)([^\\n]*)`, 'g'))) {
    if (!local.has(m[1])) local.set(m[1], { kind: constKind(m[2]) })
  }

  const kindIn = (spec, orig) => {
    if (depth > 4) return 'value'
    const text = resolve(spec)
    if (text == null) return 'value'
    return sdkExports(text, resolve, depth + 1).get(orig) ?? 'value'
  }

  for (const m of code.matchAll(/export\s+(type\s+)?\{([^}]*)\}(?:\s*from\s*['"]([^'"]+)['"])?/g)) {
    const clauseType = Boolean(m[1])
    const from = m[3]
    for (const item of clauseItems(m[2])) {
      if (clauseType || item.typeOnly) {
        set(item.name, 'type')
      } else if (from) {
        if (item.local === 'default') set(item.name, from.endsWith('.svelte') ? 'comp' : kindIn(from, 'default'))
        else set(item.name, kindIn(from, item.local))
      } else {
        const b = local.get(item.local)
        if (!b) set(item.name, 'value')
        else if (b.kind) set(item.name, b.kind)
        else if (b.typeOnly) set(item.name, 'type')
        else set(item.name, kindIn(b.from, b.orig))
      }
    }
  }
  return out
}

/** The bare name set — what the first version of this check compared. */
export function sdkExportNames(src) {
  return new Set(sdkExports(src).keys())
}

const PLURAL = { types: 'type', fns: 'fn', comps: 'comp', consts: 'const', classes: 'class' }

/**
 * The rows of docs/sdk/reference.md, each attributed to the entry point whose
 * `## \`<entry>\` …` heading it sits under. A row's first cell names one or
 * more symbols (`A` / `B`); its second cell names their kinds in the same
 * order (`comp / type`). One kind covers every symbol; with fewer kinds than
 * symbols, the last kind covers the rest (`comp / types`).
 *
 * Also returns every backticked identifier OUTSIDE the symbol cells and code
 * fences — prose that names an export must name a real one.
 */
export function parseReference(md) {
  const rows = []
  const prose = []
  const problems = []
  let entry = null
  let fenced = false
  const lines = md.split('\n')
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]
    if (/^\s*```/.test(line)) {
      fenced = !fenced
      continue
    }
    if (fenced) continue
    // Every heading opens a section: an entry point's when it names one in
    // backticks, otherwise none — so a later "## Notes" table is prose, not
    // more rows for the last entry point above it.
    if (/^#{1,6}\s/.test(line)) entry = /`(@talaria\/sdk(?:\/[\w-]+)?)`/.exec(line)?.[1] ?? null
    const ids = (text) => [...text.matchAll(new RegExp('`(' + ID + ')`', 'g'))].map((m) => m[1])
    if (entry && /^\s*\|/.test(line)) {
      const cells = line.trim().replace(/^\||\|$/g, '').split('|').map((c) => c.trim())
      if (cells.length < 3 || /^:?-+:?$/.test(cells[0]) || cells[0] === 'symbol') continue
      const symbols = ids(cells[0])
      const kinds = cells[1].split('/').map((k) => k.trim()).filter(Boolean).map((k) => PLURAL[k] ?? k)
      prose.push(...ids(cells.slice(2).join('|')).map((name) => ({ name, line: i + 1 })))
      if (symbols.length === 0) continue
      const bad = kinds.filter((k) => !ROW_KINDS.includes(k))
      if (kinds.length === 0 || bad.length || kinds.length > symbols.length) {
        problems.push({ line: i + 1, text: `row kind "${cells[1]}" does not fit its ${symbols.length} symbol(s) (kinds: ${ROW_KINDS.join(', ')})` })
        continue
      }
      symbols.forEach((name, k) => rows.push({ entry, name, kind: kinds[Math.min(k, kinds.length - 1)], line: i + 1 }))
      continue
    }
    prose.push(...ids(line).map((name) => ({ name, line: i + 1 })))
  }
  return { rows, prose, problems }
}

/** A source kind satisfies a row kind when they are equal, or the source
 *  could not see past a runtime re-export and the row claims a value kind. */
export function kindFits(source, row) {
  if (source === row) return true
  return source === 'value' && row !== 'comp' && row !== 'type'
}

/**
 * Diff the exports against the reference. `exportsByEntry` maps an entry
 * point (`@talaria/sdk`) to its `sdkExports` map. Every finding is one
 * printable line.
 */
export function diffReference(exportsByEntry, ref) {
  const missing = []
  const ghosts = []
  const kinds = []
  const dupes = []
  const rowsByEntry = new Map()
  for (const row of ref.rows) {
    if (!rowsByEntry.has(row.entry)) rowsByEntry.set(row.entry, new Map())
    const seen = rowsByEntry.get(row.entry)
    if (seen.has(row.name)) dupes.push(`${row.name} — listed twice under ${row.entry} (lines ${seen.get(row.name).line} and ${row.line})`)
    else seen.set(row.name, row)
  }
  const elsewhere = (name, entry) =>
    [...exportsByEntry.keys()].filter((e) => e !== entry && exportsByEntry.get(e).has(name))

  for (const [entry, exports] of exportsByEntry) {
    const rows = rowsByEntry.get(entry) ?? new Map()
    for (const [name, kind] of [...exports].sort(([a], [b]) => a.localeCompare(b))) {
      const row = rows.get(name)
      if (!row) {
        const other = [...rowsByEntry].filter(([e, r]) => e !== entry && r.has(name)).map(([e]) => e)
        missing.push(other.length ? `${name} — exported from ${entry}, but its row sits under ${other.join(', ')}` : `${name} — exported from ${entry} (${kind})`)
      } else if (!kindFits(kind, row.kind)) {
        kinds.push(`${name} — the row says ${row.kind}, ${entry} exports a ${kind} (line ${row.line})`)
      }
    }
  }
  for (const [entry, rows] of rowsByEntry) {
    const exports = exportsByEntry.get(entry)
    for (const row of rows.values()) {
      if (exports?.has(row.name)) continue
      const other = elsewhere(row.name, entry)
      ghosts.push(other.length ? `${row.name} — listed under ${entry}, but only ${other.join(', ')} exports it (line ${row.line})` : `${row.name} — listed under ${entry}, which does not export it (line ${row.line})`)
    }
  }
  const all = new Set([...exportsByEntry.values()].flatMap((m) => [...m.keys()]))
  for (const p of ref.prose) {
    if (!all.has(p.name)) ghosts.push(`${p.name} — named in prose at line ${p.line}, exported by neither entry point`)
  }
  for (const p of ref.problems) kinds.push(`line ${p.line}: ${p.text}`)
  return { missing, ghosts: [...new Set(ghosts)], kinds, dupes }
}
