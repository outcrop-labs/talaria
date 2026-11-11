#!/usr/bin/env node
// release-notes — the GitHub Release body for a version, built from the
// version's CHANGELOG.md section and git history alone. release.yml posts it.
//
//   node scripts/release-notes.mjs <version>     (body on stdout)
//   node scripts/release-notes.mjs <version> --leads
//       (only the "What changed" list: promote.yml's promotion body uses it)
//
// WHAT IT SAYS, AND WHERE EACH PART COMES FROM.
//   - The header: the image tags, the newest desktop-v release (Desktop ships
//     on its own line), and the pull requests whose entries the section holds.
//     A pull request is found from git: the merge on the ancestry path that
//     first carried the commit adding the entry's file, by its "(#N)" subject.
//   - "Before you upgrade": the number of migrations that run on start,
//     counted from pg.ts's MIGRATIONS at the previous stable tag and at this
//     one, then every entry's `Upgrade:` paragraph. A step an operator has to
//     take is the entry author's to write; the count is not, so it is counted.
//   - "What changed": each entry's bold lead, one line each, then the whole
//     section (prose and Verified: paragraphs) folded under a details block.
//
// WHY DETERMINISTIC. The changelog is the record a reader has
//   (CONTRIBUTING.md), reviewed with each pull request. Notes generated from it
//   say nothing a reviewer did not see; a summary written at release time
//   would be the one unreviewed text in the chain.
//
// Stdlib-only, like everything under scripts/.
import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { cmp, lastStable, parse } from './release-version.mjs'

const ROOT = path.resolve(import.meta.dirname, '..')
const PG = 'ui/src/server/db/pg.ts'
// GitHub refuses a release body over 125,000 characters.
const BODY_LIMIT = 120_000

/** The body lines of `## <version> (…)`, without its heading, or null. */
export function sectionOf(changelog, version) {
  const lines = changelog.split('\n')
  const start = lines.findIndex((l) => l.startsWith(`## ${version} (`))
  if (start === -1) return null
  const end = lines.findIndex((l, i) => i > start && l.startsWith('## '))
  const body = lines.slice(start + 1, end === -1 ? undefined : end)
  while (body.length && body.at(-1).trim() === '') body.pop()
  while (body.length && body[0].trim() === '') body.shift()
  return body
}

/** A section's entries: each starts at a `- **` line and runs to the next. */
export function entriesOf(section) {
  const entries = []
  for (const line of section) {
    if (/^- \*\*/.test(line)) entries.push([line])
    else if (entries.length) entries.at(-1).push(line)
  }
  return entries.map((e) => {
    while (e.length && e.at(-1).trim() === '') e.pop()
    return e
  })
}

/** The entry's paragraphs (blank-line separated), each as one line. */
function paragraphs(entry) {
  const out = []
  let cur = []
  for (const line of entry) {
    if (line.trim() === '') {
      if (cur.length) out.push(cur.join(' '))
      cur = []
    } else cur.push(line.trim())
  }
  if (cur.length) out.push(cur.join(' '))
  return out
}

/** The bold lead, the way promote.yml reads it: the first bold span, or for a
 *  bare label like **Fixed:**, the label and the sentence after it. */
export function leadOf(entry) {
  const first = paragraphs(entry)[0].replace(/^- /, '')
  const m = /\*\*(.+?)\*\*\s*(.*)/.exec(first)
  if (!m) return first
  if (!m[1].endsWith(':')) return m[1]
  return `${m[1]} ${m[2].replace(/([.!?])(\s|$).*/, '$1')}`.trim()
}

/** Every `Upgrade:` paragraph in the entry, without the label. */
export function upgradesOf(entry) {
  return paragraphs(entry)
    .filter((p) => /^Upgrade:\s/.test(p))
    .map((p) => p.replace(/^Upgrade:\s*/, ''))
}

/** MIGRATIONS' statement count in a pg.ts source — the rule check-invariants.mjs
 *  counts by: whole-line comments blanked first (the prose between statements
 *  quotes identifiers in backticks), then one statement per backtick pair. */
export function migrationCount(src) {
  let inBlock = false
  const stripped = src
    .split('\n')
    .map((line) => {
      const t = line.trim()
      if (inBlock) {
        if (t.includes('*/')) inBlock = false
        return ''
      }
      if (t.startsWith('/*')) {
        if (!t.includes('*/')) inBlock = true
        return ''
      }
      return t.startsWith('//') || t.startsWith('*') ? '' : line
    })
    .join('\n')
  const start = stripped.indexOf('const MIGRATIONS: string[] = [')
  if (start === -1) return null
  const body = stripped.slice(start, stripped.indexOf('\n]', start))
  return [...body.matchAll(/`[^`]*`/g)].length
}

const prRef = (n) => (n ? ` (#${n})` : '')

/** One line per entry: its lead and its pull request. */
export function leadLines(section, prs) {
  return entriesOf(section).map((e) => `- ${leadOf(e)}${prRef(prs.get(e[0]))}`)
}

export function render({ version, repo, section, prs, migrations, desktop, prev }) {
  const entries = entriesOf(section)
  const pre = !parse(version)
  const moving = pre ? 'rc' : 'latest'
  const numbers = [...new Set(entries.map((e) => prs.get(e[0])).filter(Boolean))].sort((a, b) => a - b)
  const out = []
  out.push(`Talaria ${version}: ${entries.length} change${entries.length === 1 ? '' : 's'}${prev ? ` since ${prev}` : ''}.`, '')
  out.push(`- **Images:** \`ghcr.io/${repo}:${version}\` and the moving \`${moving}\`.`)
  if (desktop)
    out.push(`- **Desktop:** not part of this release. Talaria Desktop ships as its own releases; the newest is [${desktop}](https://github.com/${repo}/releases/tag/${desktop}).`)
  if (numbers.length) out.push(`- **Pull requests:** ${numbers.map((n) => `#${n}`).join(', ')}.`)
  out.push('', '## Before you upgrade', '')
  const steps = []
  if (migrations) {
    const added = migrations.after - migrations.before
    if (added > 0) steps.push(`- [ ] **${added} database migration${added === 1 ? '' : 's'} run${added === 1 ? 's' : ''} on start.** Take a backup first.`)
    else steps.push('- **No database migrations.**')
  }
  for (const e of entries) for (const u of upgradesOf(e)) steps.push(`- [ ] ${u}${prRef(prs.get(e[0]))}`)
  if (steps.length === (migrations ? 1 : 0)) steps.push('- No entry in this release asks for a step beyond the upgrade itself.')
  out.push(...steps)
  out.push('', '## What changed', '')
  out.push(...leadLines(section, prs))
  const full = ['', '<details>', '<summary>Every entry, with what was verified</summary>', '', ...section, '', '</details>']
  const link = ['', `Full changelog: [CHANGELOG.md](https://github.com/${repo}/blob/v${version}/CHANGELOG.md).`]
  const body = [...out, ...full, ...link].join('\n') + '\n'
  // A section past GitHub's limit keeps its one-line list and links the rest.
  return body.length <= BODY_LIMIT ? body : [...out, ...link].join('\n') + '\n'
}

const git = (...args) => execFileSync('git', args, { cwd: ROOT, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] })

/** First line of each entry file added since `prev` → its pull request. */
function pullRequests(prev, head) {
  const range = prev ? [`${prev}..${head}`] : [head]
  const prs = new Map()
  const log = git('log', '--full-history', '--no-merges', '--diff-filter=A', '--name-only', '--format=@%H', ...range, '--', 'changelog/*.md')
  let commit = null
  for (const line of log.split('\n')) {
    if (line.startsWith('@')) commit = line.slice(1)
    else if (line.trim() && commit) {
      // The file's last version before the roll removed it: that is the text
      // the section holds, so its first line is the key.
      let text
      try {
        const gone = git('log', '--full-history', '-1', '--diff-filter=D', '--format=%H', ...range, '--', line).trim()
        text = git('show', `${gone ? `${gone}^` : head}:${line}`)
      } catch {
        continue
      }
      const subjects = git('log', '--merges', '--ancestry-path', '--reverse', '--format=%s', `${commit}..${head}`).split('\n')
      const own = git('log', '-1', '--format=%s', commit).trim()
      const n = /\(#(\d+)\)$/.exec([own, ...subjects].find((s) => /\(#\d+\)$/.test(s)) ?? '')?.[1]
      if (n) prs.set(text.split('\n')[0], Number(n))
    }
  }
  return prs
}

if (import.meta.main ?? process.argv[1] === import.meta.filename) {
  const version = process.argv[2]
  if (!version) {
    console.error('usage: node scripts/release-notes.mjs <version>')
    process.exit(2)
  }
  const changelog = fs.readFileSync(path.join(ROOT, 'CHANGELOG.md'), 'utf8')
  const section = sectionOf(changelog, version)
  if (!section) {
    console.error(`release-notes: CHANGELOG.md has no "## ${version} (" section`)
    process.exit(1)
  }
  const tags = git('tag', '--list').split('\n').filter(Boolean)
  // The previous stable tag below this version (an RC compares by its base).
  const base = version.replace(/-.*/, '')
  const older = tags.filter((t) => /^v\d+\.\d+\.\d+$/.test(t) && cmp(t.slice(1), base) < 0)
  const prevVersion = lastStable(older)
  const prev = prevVersion ? `v${prevVersion}` : null
  const desktop = tags
    .filter((t) => /^desktop-v\d+\.\d+\.\d+$/.test(t))
    .sort((a, b) => cmp(a.slice(9), b.slice(9)))
    .at(-1)
  let migrations = null
  if (prev) {
    const before = migrationCount(git('show', `${prev}:${PG}`))
    const after = migrationCount(fs.readFileSync(path.join(ROOT, PG), 'utf8'))
    if (before !== null && after !== null) migrations = { before, after }
  }
  if (process.argv.includes('--leads')) {
    console.log(leadLines(section, pullRequests(prev, 'HEAD')).join('\n'))
    process.exit(0)
  }
  process.stdout.write(
    render({
      version,
      repo: process.env.GITHUB_REPOSITORY || 'outcrop-labs/talaria',
      section,
      prs: pullRequests(prev, 'HEAD'),
      migrations,
      desktop,
      prev,
    }),
  )
}
