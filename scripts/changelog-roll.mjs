#!/usr/bin/env node
// changelog-roll — the CHANGELOG's Unreleased section as individual entry
// files, merged at release time.
//
//   bun scripts/changelog-roll.mjs --check        (wired into `bun run check`)
//   bun scripts/changelog-roll.mjs --migrate      (one-time cutover)
//   bun scripts/changelog-roll.mjs <heading>      (release: roll entries in)
//
// WHY. The single `## [Unreleased]` section was the repo's worst merge-conflict
// surface — most-touched file, and a PR's entry landed at the same anchor as
// every other open PR's, so merges reconciled bullets by hand. Entries now
// land as one file each under changelog/ (added, never merged against a
// neighbor); roll folds them into a version section and deletes them. The
// append-only contract on CHANGELOG.md itself is unchanged: version sections
// are still only ever appended.
//
// FILE SHAPE. changelog/YYYY-MM-DD-slug.md — the date orders entries within a
// release (lexical sort = chronological = merge order; no central counter to
// contend on), the slug disambiguates same-day entries. The body is the
// verbatim bullet: `- **Bold lead.**` + indented prose + a `Verified:`
// paragraph, exactly what used to be hand-appended.
//
// THE CHECK. Every entry file is well-formed (name shape, bold lead), and
// CHANGELOG.md's `## [Unreleased]` carries no bullets — after the cutover a
// hand-appended entry fails with the filename recipe instead of quietly
// reintroducing the conflict surface. The `Verified:` paragraph is NOT part
// of the hard check: the convention postdates most of the entries the
// cutover migrated, and frozen history is not rewritten to satisfy a gate —
// judge-pr.mjs enforces Verified: on NEW entries, where it is meaningful.
//
// Stdlib-only, like everything under scripts/.

import fs from 'node:fs'
import path from 'node:path'

const ROOT = path.resolve(import.meta.dirname, '..')
const CHANGELOG = path.join(ROOT, 'CHANGELOG.md')
const ENTRIES_DIR = path.join(ROOT, 'changelog')
const NAME_RE = /^\d{4}-\d{2}-\d{2}-[a-z0-9][a-z0-9-]*\.md$/

function entryFiles() {
  if (!fs.existsSync(ENTRIES_DIR)) return []
  return fs
    .readdirSync(ENTRIES_DIR)
    .filter((f) => f.endsWith('.md'))
    .sort()
}

/** The body of CHANGELOG.md's `## [Unreleased]` section (between the heading
 *  and the next `## `), as lines. */
function unreleasedLines(text) {
  const lines = text.split('\n')
  const start = lines.findIndex((l) => l.startsWith('## [Unreleased]'))
  if (start === -1) return null
  const end = lines.findIndex((l, i) => i > start && l.startsWith('## '))
  return lines.slice(start + 1, end === -1 ? undefined : end)
}

/** Split an Unreleased body into entries: an entry starts at a `^- **` line,
 *  everything indented (or blank) after it is its continuation. */
function splitEntries(bodyLines) {
  const entries = []
  for (const line of bodyLines) {
    if (/^- \*\*/.test(line)) entries.push([line])
    else if (entries.length) entries[entries.length - 1].push(line)
  }
  // Trim each entry's trailing blank lines.
  return entries.map((e) => {
    while (e.length && e[e.length - 1].trim() === '') e.pop()
    return e
  })
}

function slugify(leadLine) {
  const lead = leadLine.replace(/^- \*\*/, '').replace(/\*\*.*$/s, '')
  return (
    lead
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .split('-')
      .filter(Boolean)
      .slice(0, 6)
      .join('-') || 'entry'
  )
}

function today() {
  return new Date().toISOString().slice(0, 10)
}

// ── --check ──────────────────────────────────────────────────────────────────
if (process.argv.includes('--check')) {
  const failures = []
  const files = entryFiles()
  for (const f of files) {
    if (!NAME_RE.test(f)) {
      failures.push(`changelog/${f}: filename must be YYYY-MM-DD-slug.md`)
      continue
    }
    const body = fs.readFileSync(path.join(ENTRIES_DIR, f), 'utf8')
    if (!body.startsWith('- **')) failures.push(`changelog/${f}: body must start with the bold-lead bullet ("- **…")`)
  }
  const un = unreleasedLines(fs.readFileSync(CHANGELOG, 'utf8'))
  if (un === null) failures.push('CHANGELOG.md has no ## [Unreleased] heading — the roll anchor is gone')
  else if (un.some((l) => /^- \*\*/.test(l)))
    failures.push(
      'CHANGELOG.md carries bullets under ## [Unreleased] — entries go in changelog/<date>-<slug>.md, ' +
        'one file each; the section stays empty between releases',
    )
  if (failures.length) {
    console.error('FAIL changelog-drift')
    for (const f of failures) console.error(`  ${f}`)
    console.error('\nWHAT TO DO INSTEAD: add changelog/YYYY-MM-DD-<slug>.md containing the verbatim bullet\n' + '  (bold lead + prose + Verified: paragraph); bun scripts/changelog-roll.mjs --check re-runs this.')
    process.exit(1)
  }
  console.log(`changelog: ${files.length} unreleased entr${files.length === 1 ? 'y' : 'ies'}, all well-formed; [Unreleased] carries no bullets`)
  process.exit(0)
}

// ── --migrate (one-time cutover) ─────────────────────────────────────────────
if (process.argv.includes('--migrate')) {
  const text = fs.readFileSync(CHANGELOG, 'utf8')
  const un = unreleasedLines(text)
  if (un === null) throw new Error('no ## [Unreleased] section')
  const entries = splitEntries(un)
  if (!entries.length) throw new Error('nothing to migrate — [Unreleased] is already empty')
  fs.mkdirSync(ENTRIES_DIR, { recursive: true })
  const date = today()
  const used = new Set(entryFiles())
  let n = 0
  for (const e of entries) {
    let slug = slugify(e[0])
    let name = `${date}-${slug}.md`
    for (let i = 2; used.has(name); i++) name = `${date}-${slug}-${i}.md`
    used.add(name)
    fs.writeFileSync(path.join(ENTRIES_DIR, name), e.join('\n') + '\n')
    n++
  }
  // Rebuild the file with an empty Unreleased section.
  const lines = text.split('\n')
  const start = lines.findIndex((l) => l.startsWith('## [Unreleased]'))
  const end = lines.findIndex((l, i) => i > start && l.startsWith('## '))
  const rebuilt = [
    ...lines.slice(0, start + 1),
    '',
    ...lines.slice(end === -1 ? undefined : end),
  ]
  fs.writeFileSync(CHANGELOG, rebuilt.join('\n'))
  console.log(`migrated ${n} entries into changelog/ (${date}-*.md); [Unreleased] is now the empty anchor`)
  process.exit(0)
}

// ── roll <heading> ───────────────────────────────────────────────────────────
const headingArg = process.argv[2]
if (!headingArg || headingArg.startsWith('-')) {
  console.error('usage: bun scripts/changelog-roll.mjs [--check | --migrate | <version-heading>]')
  process.exit(2)
}
{
  const files = entryFiles()
  if (!files.length) {
    console.error('no changelog/*.md entries to roll — a release with no notes is a prompt, not a default')
    process.exit(1)
  }
  if (files.some((f) => !NAME_RE.test(f))) {
    console.error('refusing to roll: an entry file fails the name shape (run --check for the list)')
    process.exit(1)
  }
  const section = [`## ${headingArg} (${today()})`, '']
  for (const f of files) {
    section.push(fs.readFileSync(path.join(ENTRIES_DIR, f), 'utf8').replace(/\n$/, ''))
    section.push('')
  }
  const text = fs.readFileSync(CHANGELOG, 'utf8')
  const lines = text.split('\n')
  const start = lines.findIndex((l) => l.startsWith('## [Unreleased]'))
  if (start === -1) throw new Error('no ## [Unreleased] heading')
  const rebuilt = [...lines.slice(0, start + 1), '', ...section, ...lines.slice(start + 1).filter((l, i) => i > 0 || l.trim() !== '')]
  fs.writeFileSync(CHANGELOG, rebuilt.join('\n'))
  for (const f of files) fs.unlinkSync(path.join(ENTRIES_DIR, f))
  console.log(`rolled ${files.length} entries into "## ${headingArg} (${today()})"; changelog/ is empty until the next change`)
}
