#!/usr/bin/env node
// release-version — which version the next release is, worked out from git
// tags and CHANGELOG.md alone. The release chain in CI calls it; nothing here
// writes a file or a tag.
//
//   node scripts/release-version.mjs next [--version X.Y.Z]
//       prints "<version> <new|extend> <patch|minor|major|set>": the version
//       the roll should write, whether it opens a new section or extends the
//       pending one, and the bump the entries asked for (`set` for --version).
//   node scripts/release-version.mjs pending
//       prints the top CHANGELOG section's version when it is rolled but not
//       yet tagged (newer than every vX.Y.Z tag), and nothing otherwise.
//
// WHY THE TAG IS STILL THE AUTHORITY. A version exists once `vX.Y.Z` is
//   tagged; a rolled section above the newest tag is a release in waiting.
//   So "pending" is a section newer than the newest stable tag, and the next
//   roll EXTENDS that section rather than opening another above it. An entry
//   that merges between the roll and the promotion lands in the same release
//   instead of riding into the tag outside its section.
//
// WHY PATCH UNLESS AN ENTRY ASKS. The default is the next patch. An entry file
//   raises it with a `<!-- bump: minor -->` (or `major`) line, and the largest
//   ask among the entries wins. The roll strips the line, so CHANGELOG.md never
//   carries it. A dispatch can name the version outright with --version, which
//   must be newer than the newest tag.
//
// Stdlib-only, like everything under scripts/.
import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'

const ROOT = path.resolve(import.meta.dirname, '..')
const STABLE_RE = /^(\d+)\.(\d+)\.(\d+)$/
export const BUMP_RE = /^<!--\s*bump:\s*(\S+)\s*-->\s*$/
const LEVELS = ['patch', 'minor', 'major']

export function parse(v) {
  const m = STABLE_RE.exec(v)
  return m ? m.slice(1, 4).map(Number) : null
}

export function cmp(a, b) {
  const [x, y] = [parse(a), parse(b)]
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] - y[i]
  return 0
}

export function bump(v, level) {
  const [major, minor, patch] = parse(v)
  if (level === 'major') return `${major + 1}.0.0`
  if (level === 'minor') return `${major}.${minor + 1}.0`
  return `${major}.${minor}.${patch + 1}`
}

/** The newest stable version among `git tag` names (`vX.Y.Z` only — an RC,
 *  a desktop-v or mobile-v tag is a different line). */
export function lastStable(tags) {
  const versions = tags.filter((t) => t.startsWith('v')).map((t) => t.slice(1)).filter(parse)
  return versions.sort(cmp).at(-1) ?? null
}

/** The version of CHANGELOG.md's first released section (`## X.Y.Z (date)`),
 *  or null. [Unreleased] is the roll's anchor, not a section. */
export function topSection(changelog) {
  for (const line of changelog.split('\n')) {
    const m = /^## (\S+) \(/.exec(line)
    if (m) return m[1]
  }
  return null
}

/** The bump an entry file asks for: 'patch' unless it carries a marker line.
 *  An unknown level throws, so `--check` can name the file. */
export function bumpOf(entry) {
  for (const line of entry.split('\n')) {
    const m = BUMP_RE.exec(line)
    if (!m) continue
    if (!LEVELS.includes(m[1])) throw new Error(`bump: ${m[1]} is not one of ${LEVELS.join(', ')}`)
    return m[1]
  }
  return 'patch'
}

/** The pending version: a top section newer than the newest stable tag. */
export function pending({ tags, changelog }) {
  const top = topSection(changelog)
  if (!top || !parse(top)) return null
  const last = lastStable(tags)
  return !last || cmp(top, last) > 0 ? top : null
}

export function next({ tags, changelog, entries, override }) {
  const last = lastStable(tags)
  const held = pending({ tags, changelog })
  const mode = held ? 'extend' : 'new'
  if (override) {
    if (!parse(override)) throw new Error(`--version ${override} is not X.Y.Z`)
    if (last && cmp(override, last) <= 0) throw new Error(`--version ${override} is not newer than v${last}`)
    return { version: override, mode, level: 'set' }
  }
  const level = entries.map(bumpOf).reduce((a, b) => (LEVELS.indexOf(b) > LEVELS.indexOf(a) ? b : a), 'patch')
  if (!last) return { version: held ?? '0.1.0', mode, level }
  const asked = bump(last, level)
  // A pending 0.14.0 stays 0.14.0 when a later entry asks only for a patch;
  // a pending 0.13.3 becomes 0.14.0 when one asks for a minor.
  return { version: held && cmp(held, asked) > 0 ? held : asked, mode, level }
}

function read() {
  const tags = execFileSync('git', ['tag', '--list', 'v*'], { cwd: ROOT, encoding: 'utf8' }).split('\n').filter(Boolean)
  const changelog = fs.readFileSync(path.join(ROOT, 'CHANGELOG.md'), 'utf8')
  const dir = path.join(ROOT, 'changelog')
  const entries = fs.existsSync(dir)
    ? fs.readdirSync(dir).filter((f) => f.endsWith('.md')).sort().map((f) => fs.readFileSync(path.join(dir, f), 'utf8'))
    : []
  return { tags, changelog, entries }
}

if (import.meta.main ?? process.argv[1] === import.meta.filename) {
  const [cmd, flag, value] = process.argv.slice(2)
  try {
    if (cmd === 'pending') {
      const v = pending(read())
      if (v) console.log(v)
    } else if (cmd === 'next') {
      const { version, mode, level } = next({ ...read(), override: flag === '--version' ? value : undefined })
      console.log(`${version} ${mode} ${level}`)
    } else {
      console.error('usage: node scripts/release-version.mjs next [--version X.Y.Z] | pending')
      process.exit(2)
    }
  } catch (e) {
    console.error(`release-version: ${e.message}`)
    process.exit(1)
  }
}
