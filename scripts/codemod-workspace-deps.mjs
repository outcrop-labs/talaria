#!/usr/bin/env node
// Codemod: collapse the api workspace's per-crate dependency declarations into
// [workspace.dependencies] / [workspace.package] inheritance.
//
//   node scripts/codemod-workspace-deps.mjs [--dry]
//
// Idempotent: a second run is a no-op (that property is the rebase-repair
// story — a session that conflicts on a crate manifest re-runs this instead of
// hand-merging). Stdlib-only like everything under scripts/.
//
// What it does:
//   · api/Cargo.toml        members list → ["crates/*"] glob; adds
//                           [workspace.package] and [workspace.dependencies]
//                           (external deps with the root's rationale comments,
//                           then every path dep); rewrites the root's own
//                           [dependencies] to `{ workspace = true }` refs.
//   · api/crates/*/Cargo.toml  [package] fields → `.workspace = true`;
//                           dep lines (dependencies / dev-dependencies /
//                           build-dependencies) → `{ workspace = true }`,
//                           preserving `optional = true` and local comments.
//
// The union of features per external dep is computed from every manifest, so
// the workspace-wide feature set matches what cargo already unified across the
// build graph — api/Cargo.lock must not change. Deps whose real feature set
// includes IMPLIED defaults (axum's default-features) carry an explicit
// override below, because a parsed union cannot see what was never written.
//
// Anything it cannot express mechanically (two version strings, a git dep, an
// unknown section with dep-like lines) is reported and left untouched for a
// human decision; it never guesses.

import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'

const ROOT = path.resolve(import.meta.dirname, '..', 'api')
const DRY = process.argv.includes('--dry')
const cratesDir = path.join(ROOT, 'crates')

// Curated specs where the parsed union would be wrong. axum: 24 crates build
// with defaults ON (never written in manifests) + multipart; 21 with
// default-features = false. The union is defaults spelled out explicitly plus
// multipart — every observed feature subsumed, nothing implicit left behind.
const OVERRIDES = {
  axum: 'axum = { version = "0.8", default-features = false, features = ["form", "http1", "json", "matched-path", "multipart", "original-uri", "query", "tokio", "tower-log", "tracing"] }',
}

const KNOWN_DEP_SECTIONS = new Set(['dependencies', 'dev-dependencies', 'build-dependencies'])
const KNOWN_SECTIONS = new Set([
  ...KNOWN_DEP_SECTIONS,
  'package',
  'lints',
  'features',
  'lib',
  'bin',
  'profile.dev',
  'profile.dev.package."*"',
  'profile.release',
  'workspace',
  'workspace.package',
  'workspace.dependencies',
  'workspace.lints',
  'workspace.lints.clippy',
])

// ── inline-table parsing ─────────────────────────────────────────────────────
function splitTopLevel(s) {
  const parts = []
  let depth = 0
  let quote = null
  let cur = ''
  for (let i = 0; i < s.length; i++) {
    const ch = s[i]
    if (quote) {
      cur += ch
      if (ch === quote && s[i - 1] !== '\\') quote = null
      continue
    }
    if (ch === '"' || ch === "'") {
      quote = ch
      cur += ch
      continue
    }
    if (ch === '[' || ch === '{') depth++
    if (ch === ']' || ch === '}') depth--
    if (ch === ',' && depth === 0) {
      parts.push(cur.trim())
      cur = ''
      continue
    }
    cur += ch
  }
  if (cur.trim()) parts.push(cur.trim())
  return parts
}

function parseInlineTable(body) {
  const out = {}
  for (const part of splitTopLevel(body)) {
    const eq = part.indexOf('=')
    if (eq === -1) continue
    const key = part.slice(0, eq).trim()
    const val = part.slice(eq + 1).trim()
    if (val.startsWith('[') && val.endsWith(']')) {
      out[key] = splitTopLevel(val.slice(1, -1)).map((v) => v.trim().replace(/^["']|["']$/g, ''))
    } else if (val === 'true' || val === 'false') {
      out[key] = val === 'true'
    } else {
      out[key] = val.replace(/^["']|["']$/g, '')
    }
  }
  return out
}

const DEP_LINE = /^(\s*)([A-Za-z0-9_.-]+)\s*=\s*(.+?),?\s*$/

function classifyValue(raw) {
  const value = raw.trim().replace(/,\s*$/, '')
  if (/^"/.test(value)) return { kind: 'simple', version: value.replace(/^"|"$/g, '') }
  if (value.startsWith('{') && value.endsWith('}')) {
    const spec = parseInlineTable(value.slice(1, -1))
    if (spec.workspace === true) return { kind: 'inherited' }
    if (spec.git !== undefined || spec.branch !== undefined || spec.registry !== undefined)
      return { kind: 'unsupported' }
    return { kind: 'table', spec }
  }
  return { kind: 'unsupported' }
}

// ── survey ───────────────────────────────────────────────────────────────────
const warnings = []
const anomalies = []
const externals = new Map() // name → union record
const rootFile = path.join(ROOT, 'Cargo.toml')
const crateDirs = fs
  .readdirSync(cratesDir)
  .filter((d) => fs.existsSync(path.join(cratesDir, d, 'Cargo.toml')))
const crateFiles = crateDirs.map((d) => path.join(cratesDir, d, 'Cargo.toml'))
const crateNames = crateDirs.slice().sort()

function recordExternal(name, spec, isRoot) {
  let e = externals.get(name)
  if (!e) {
    e = { versions: new Set(), defaultFalse: 0, defaultTrue: 0, features: new Set(), optional: false, atRoot: false }
    externals.set(name, e)
  }
  if (isRoot) e.atRoot = true
  if (spec.version !== undefined) e.versions.add(spec.version)
  if (spec['default-features'] === false) e.defaultFalse++
  else e.defaultTrue++
  for (const f of spec.features ?? []) e.features.add(f)
  if (spec.optional === true) e.optional = true
}

function survey(file, isRoot) {
  let section = null
  for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
    const sec = line.match(/^\[([^\]]+)\]$/)
    if (sec) {
      section = sec[1]
      if (
        !KNOWN_SECTIONS.has(section) &&
        !section.startsWith('target.') &&
        !section.startsWith('bin.') &&
        !section.startsWith('profile.')
      )
        warnings.push(`${path.relative(ROOT, file)}: unknown section [${section}] left untouched`)
      continue
    }
    if (!KNOWN_DEP_SECTIONS.has(section ?? '')) continue
    const m = line.match(DEP_LINE)
    if (!m) continue
    const c = classifyValue(m[3])
    if (c.kind === 'inherited' || c.kind === 'table' && c.spec.path !== undefined) continue
    if (c.kind === 'unsupported') {
      warnings.push(`${path.relative(ROOT, file)}: dep ${m[2]} has an unsupported spec — left untouched`)
      continue
    }
    recordExternal(m[2], c.kind === 'simple' ? { version: c.version } : c.spec, isRoot)
  }
}

survey(rootFile, true)
for (const f of crateFiles) survey(f, false)

for (const [name, e] of externals) {
  if (e.versions.size > 1) anomalies.push(`${name}: multiple version strings — ${[...e.versions].join(' , ')}`)
  if (e.defaultFalse && e.defaultTrue && !(name in OVERRIDES))
    anomalies.push(`${name}: mixed default-features (${e.defaultFalse} false / ${e.defaultTrue} true)`)
}
for (const [name, spec] of Object.entries(OVERRIDES)) {
  const e = externals.get(name)
  if (!e) continue
  const overrideFeatures = new Set((spec.match(/features = \[([^\]]*)\]/)?.[1] ?? '').split(',').map((s) => s.trim().replace(/"/g, '')))
  const missing = [...e.features].filter((f) => !overrideFeatures.has(f) && f !== 'default')
  if (missing.length) anomalies.push(`${name}: override omits observed features ${missing.join(', ')}`)
}

// ── emission ─────────────────────────────────────────────────────────────────
// default-features stance is DERIVED from the survey: a dep every declaration
// had as default-features=false stays false (boa_engine's bare JS engine must
// not grow temporal/ICU extras just because it moved tables); a dep with mixed
// usage is only legal through an explicit OVERRIDES entry (axum).
const DEFAULT_OFF = new Set(
  [...externals]
    .filter(([name, e]) => e.defaultFalse > 0 && e.defaultTrue === 0 && !(name in OVERRIDES))
    .map(([name]) => name)
    .concat(Object.keys(OVERRIDES)),
)

function emitSpec(name) {
  if (name in OVERRIDES) return OVERRIDES[name]
  const e = externals.get(name)
  let version = [...e.versions][0]
  if (e.versions.has('0.10') && e.versions.has('0.10.0')) version = '0.10.0' // serde_yaml_ng cosmetic skew
  const bits = []
  if (version !== undefined) bits.push(`version = "${version}"`)
  if (DEFAULT_OFF.has(name)) bits.push('default-features = false')
  if (e.features.size) bits.push(`features = [${[...e.features].sort().map((f) => `"${f}"`).join(', ')}]`)
  return `${name} = { ${bits.join(', ')} }`
}

// ── root reconstruction ──────────────────────────────────────────────────────
function parseRoot() {
  const lines = fs.readFileSync(rootFile, 'utf8').split('\n')
  const headerComments = []
  const lintsBlock = []
  const rootExternals = [] // { name, comments } in file order
  let i = 0
  for (; i < lines.length; i++) {
    if (/^\[workspace\]/.test(lines[i])) break
    headerComments.push(lines[i])
  }
  let section = null
  let pending = []
  for (; i < lines.length; i++) {
    const line = lines[i]
    const sec = line.match(/^\[([^\]]+)\]$/)
    if (sec) {
      section = sec[1]
      if (section === 'workspace.lints.clippy') lintsBlock.push(line)
      continue
    }
    if (section === 'workspace.lints.clippy') {
      lintsBlock.push(line)
      continue
    }
    if (section !== 'dependencies') continue
    if (line.trim() === '' || line.trimStart().startsWith('#')) {
      pending.push(line)
      continue
    }
    const m = line.match(DEP_LINE)
    if (!m) continue
    const c = classifyValue(m[3])
    if (c.kind === 'inherited' || c.kind === 'unsupported') continue
    if (c.kind === 'table' && c.spec.path !== undefined) continue
    rootExternals.push({ name: m[2], comments: pending.filter((l) => l.trim() !== '') })
    pending = []
  }
  // trim trailing blank header lines
  while (headerComments.length && headerComments.at(-1).trim() === '') headerComments.pop()
  while (lintsBlock.length && lintsBlock.at(-1).trim() === '') lintsBlock.pop()
  return { headerComments, lintsBlock, rootExternals }
}

const { headerComments, lintsBlock, rootExternals } = parseRoot()
// Root reconstruction is only valid from an ORIGINAL root: re-running against
// a converted tree would lose crate-only externals (inherited lines carry no
// version/features to survey). Idempotency for the root is detect-and-skip.
const rootConverted = (() => {
  let section = null
  for (const line of fs.readFileSync(rootFile, 'utf8').split('\n')) {
    const sec = line.match(/^\[([^\]]+)\]$/)
    if (sec) section = sec[1]
    if (KNOWN_DEP_SECTIONS.has(section ?? '') && /\{\s*workspace\s*=\s*true/.test(line)) return true
  }
  return false
})()
const rootExternalNames = rootExternals.map((r) => r.name)
const crateOnlyExternals = [...externals.keys()].filter((n) => !rootExternalNames.includes(n)).sort()

const depEntries = []
for (const { name, comments } of rootExternals) {
  if (comments.length) depEntries.push(...comments)
  depEntries.push(emitSpec(name))
  depEntries.push('')
}
for (const name of crateOnlyExternals) {
  depEntries.push('# Declared by crates only (not by the root binary); union of crate specs.')
  depEntries.push(emitSpec(name))
  depEntries.push('')
}
for (const name of crateNames) depEntries.push(`${name} = { path = "crates/${name}" }`, '')

const newRoot = [
  ...headerComments,
  '',
  '[workspace]',
  '# Glob membership: every manifest-bearing dir under crates/ is a member. A new',
  '# crate is picked up by existing; a stray dir without a manifest fails loudly at',
  '# the next cargo invocation rather than being silently absent from a hand list.',
  'members = ["crates/*"]',
  'resolver = "3"',
  '',
  '[workspace.package]',
  'version = "0.1.0"',
  'edition = "2024"',
  'rust-version = "1.97"',
  'publish = false',
  '',
  '[workspace.dependencies]',
  '# External deps first (the root binary\'s rationale comments travel with their',
  '# entries), then every workspace crate as a path dep. Manifests say only',
  '# `name = { workspace = true }` — version and features live here, once.',
  ...depEntries.slice(0, -1),
  '',
  ...lintsBlock,
  '',
  '[package]',
  'name = "talaria-api"',
  'version.workspace = true',
  'edition.workspace = true',
  'rust-version.workspace = true',
  'publish.workspace = true',
  '',
  '[lints]',
  'workspace = true',
  '',
  '[dependencies]',
  ...crateNames.map((n) => `${n} = { workspace = true }`),
  ...rootExternalNames.map((n) => `${n} = { workspace = true }`),
  // crate-only externals stay OUT of the root package's own deps — they live
  // in [workspace.dependencies] for crates to inherit, and declaring them here
  // would add edges the original root never had (a Cargo.lock diff).
  '',
  '# Dev-loop compile: optimize our crate a little, deps a lot, keep unwind',
  '# (tests + tower-http catch-panic). debug=line-tables-only cuts link size',
  '# without dropping backtraces.',
  '[profile.dev]',
  'opt-level = 1',
  'debug = "line-tables-only"',
  '',
  '[profile.dev.package."*"]',
  'opt-level = 3',
  '',
].join('\n')

// ── crate rewrite ────────────────────────────────────────────────────────────
function rewriteCrate(file) {
  const out = []
  let section = null
  let changed = 0
  for (const line of fs.readFileSync(file, 'utf8').split('\n')) {
    const sec = line.match(/^\[([^\]]+)\]$/)
    if (sec) {
      section = sec[1]
      out.push(line)
      continue
    }
    if (section === 'package') {
      if (/^version\s*=/.test(line)) { out.push('version.workspace = true'); changed++; continue }
      if (/^edition\s*=/.test(line)) { out.push('edition.workspace = true'); changed++; continue }
      if (/^rust-version\s*=/.test(line)) { out.push('rust-version.workspace = true'); changed++; continue }
      if (/^publish\s*=/.test(line)) { out.push('publish.workspace = true'); changed++; continue }
      out.push(line)
      continue
    }
    if (!KNOWN_DEP_SECTIONS.has(section ?? '')) {
      out.push(line)
      continue
    }
    const m = line.match(DEP_LINE)
    if (!m) {
      out.push(line)
      continue
    }
    const [, indent, name, raw] = m
    const c = classifyValue(raw)
    if (c.kind === 'inherited' || c.kind === 'unsupported') {
      out.push(line)
      continue
    }
    if (c.kind === 'table' && c.spec.path !== undefined) {
      out.push(`${indent}${name} = { workspace = true${c.spec.optional === true ? ', optional = true' : ''} }`)
      changed++
      continue
    }
    // external
    const optional = c.kind === 'table' && c.spec.optional === true
    out.push(`${indent}${name} = { workspace = true${optional ? ', optional = true' : ''} }`)
    changed++
  }
  if (changed && !DRY) fs.writeFileSync(file, out.join('\n'))
  return changed
}

// ── run ──────────────────────────────────────────────────────────────────────
if (rootConverted) {
  console.log('api/Cargo.toml already converted — root left untouched (crate-only externals are only discoverable from an unconverted tree)')
} else {
  if (!DRY) fs.writeFileSync(rootFile, newRoot)
  console.log(`${DRY ? '[dry] ' : ''}api/Cargo.toml reconstructed`)
}
const changedFiles = crateFiles.map((f) => ({ f, n: rewriteCrate(f) })).filter((x) => x.n > 0)
console.log(`${DRY ? '[dry] ' : ''}${changedFiles.length}/${crateFiles.length} crate manifests rewritten (${changedFiles.reduce((a, b) => a + b.n, 0)} lines)`)

if (anomalies.length) {
  console.log('\nANOMALIES (need a human decision):')
  for (const a of anomalies) console.log(`  · ${a}`)
}
if (warnings.length) {
  console.log('\nWARNINGS (lines left untouched):')
  for (const w of warnings) console.log(`  · ${w}`)
}
if (!anomalies.length && !warnings.length) console.log('\nsurvey clean: no version skew, no unsupported specs')
