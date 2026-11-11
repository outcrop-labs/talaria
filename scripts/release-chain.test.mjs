// The release chain's arithmetic: which version comes next, what the roll
// writes, and what the release notes say. The workflows are thin shell around
// these three scripts, so this is where their decisions are tested.
// Run by `bun run check`.
import { spawnSync } from 'node:child_process'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { bumpOf, lastStable, next, pending } from './release-version.mjs'
import { entriesOf, leadOf, migrationCount, render, sectionOf, upgradesOf } from './release-notes.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
let failed = 0
let ran = 0

function check(name, fn) {
  ran++
  try {
    fn()
  } catch (e) {
    failed++
    console.error(`FAIL ${name}: ${e instanceof Error ? e.stack : e}`)
  }
}

function eq(actual, expected) {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) throw new Error(`expected ${b}, got ${a}`)
}

function throws(fn, pattern) {
  try {
    fn()
  } catch (e) {
    if (pattern.test(e.message)) return
    throw new Error(`threw "${e.message}", not ${pattern}`)
  }
  throw new Error(`did not throw ${pattern}`)
}

const TAGS = ['v0.12.0', 'v0.13.0', 'v0.13.1', 'v0.13.2', 'v0.14.0-rc.1', 'desktop-v0.12.0', 'mobile-v0.2.0']
const changelog = (top) => `# Changelog\n\n## [Unreleased]\n\n## ${top} (2026-11-10)\n\n- **x.**\n`
const minor = '- **New: a thing.**\n\n<!-- bump: minor -->\n'

// ── release-version ─────────────────────────────────────────────────────────
check('the newest stable tag ignores RC, desktop and mobile tags', () => eq(lastStable(TAGS), '0.13.2'))
check('the default is the next patch, opening a new section', () =>
  eq(next({ tags: TAGS, changelog: changelog('0.13.2'), entries: ['- **Fixed: a.**\n'] }), { version: '0.13.3', mode: 'new', level: 'patch' }))
check('an entry asking minor raises it', () =>
  eq(next({ tags: TAGS, changelog: changelog('0.13.2'), entries: ['- **a.**\n', minor] }), { version: '0.14.0', mode: 'new', level: 'minor' }))
check('a rolled, untagged section is pending and the next roll extends it', () => {
  eq(pending({ tags: TAGS, changelog: changelog('0.13.3') }), '0.13.3')
  eq(next({ tags: TAGS, changelog: changelog('0.13.3'), entries: ['- **a.**\n'] }), { version: '0.13.3', mode: 'extend', level: 'patch' })
})
check('a tagged top section is not pending', () => eq(pending({ tags: TAGS, changelog: changelog('0.13.2') }), null))
check('a pending minor stays minor when a later entry asks only a patch', () =>
  eq(next({ tags: TAGS, changelog: changelog('0.14.0'), entries: ['- **a.**\n'] }), { version: '0.14.0', mode: 'extend', level: 'patch' }))
check('a pending patch becomes the minor a later entry asks for', () =>
  eq(next({ tags: TAGS, changelog: changelog('0.13.3'), entries: [minor] }), { version: '0.14.0', mode: 'extend', level: 'minor' }))
check('an override must be X.Y.Z and newer than the newest tag', () => {
  eq(next({ tags: TAGS, changelog: changelog('0.13.2'), entries: [], override: '1.0.0' }), { version: '1.0.0', mode: 'new', level: 'set' })
  throws(() => next({ tags: TAGS, changelog: changelog('0.13.2'), entries: [], override: '0.13.2' }), /not newer/)
  throws(() => next({ tags: TAGS, changelog: changelog('0.13.2'), entries: [], override: 'v1.0' }), /not X\.Y\.Z/)
})
check('an unknown bump level is refused', () => throws(() => bumpOf('- **a.**\n<!-- bump: huge -->\n'), /huge/))

// ── changelog-roll ──────────────────────────────────────────────────────────
function rollIn(changelogText, entries, args) {
  const dir = mkdtempSync(join(tmpdir(), 'release-chain-'))
  try {
    mkdirSync(join(dir, 'scripts'))
    mkdirSync(join(dir, 'changelog'))
    for (const f of ['changelog-roll.mjs', 'release-version.mjs']) copyFileSync(join(ROOT, 'scripts', f), join(dir, 'scripts', f))
    writeFileSync(join(dir, 'CHANGELOG.md'), changelogText)
    for (const [name, body] of Object.entries(entries)) writeFileSync(join(dir, 'changelog', name), body)
    const r = spawnSync(process.execPath, [join(dir, 'scripts', 'changelog-roll.mjs'), ...args], { encoding: 'utf8' })
    if (r.status !== 0) throw new Error(`roll exited ${r.status}: ${r.stderr}`)
    return { text: readFileSync(join(dir, 'CHANGELOG.md'), 'utf8'), left: readdirSync(join(dir, 'changelog')) }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}
const today = new Date().toISOString().slice(0, 10)
const base = '# Changelog\n\n## [Unreleased]\n\n## 0.13.3 (2026-11-01)\n\n- **Old pending.**\n\n## 0.13.2 (2026-10-01)\n\n- **Released.**\n'

check('a new roll opens a section and strips the bump line', () => {
  const { text, left } = rollIn('# Changelog\n\n## [Unreleased]\n\n## 0.13.2 (2026-10-01)\n\n- **Released.**\n', { '2026-11-02-a.md': minor }, ['0.14.0'])
  eq(left, [])
  eq(text, `# Changelog\n\n## [Unreleased]\n\n## 0.14.0 (${today})\n\n- **New: a thing.**\n\n## 0.13.2 (2026-10-01)\n\n- **Released.**\n`)
})
check('--extend folds entries into the pending section and re-heads it', () => {
  const { text } = rollIn(base, { '2026-11-02-a.md': '- **Late fix.**\n\n  Verified: yes.\n' }, ['0.13.3', '--extend'])
  eq(text, `# Changelog\n\n## [Unreleased]\n\n## 0.13.3 (${today})\n\n- **Old pending.**\n\n- **Late fix.**\n\n  Verified: yes.\n\n## 0.13.2 (2026-10-01)\n\n- **Released.**\n`)
})
check('--check names an entry with an unknown bump level', () => {
  const dir = mkdtempSync(join(tmpdir(), 'release-chain-'))
  try {
    mkdirSync(join(dir, 'scripts'))
    mkdirSync(join(dir, 'changelog'))
    for (const f of ['changelog-roll.mjs', 'release-version.mjs']) copyFileSync(join(ROOT, 'scripts', f), join(dir, 'scripts', f))
    writeFileSync(join(dir, 'CHANGELOG.md'), '# Changelog\n\n## [Unreleased]\n')
    writeFileSync(join(dir, 'changelog', '2026-11-02-a.md'), '- **a.**\n<!-- bump: big -->\n')
    const r = spawnSync(process.execPath, [join(dir, 'scripts', 'changelog-roll.mjs'), '--check'], { encoding: 'utf8' })
    eq(r.status, 1)
    if (!/2026-11-02-a\.md: bump: big/.test(r.stderr)) throw new Error(r.stderr)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

// ── release-notes ───────────────────────────────────────────────────────────
const SECTION_TEXT = [
  '# Changelog', '', '## [Unreleased]', '', '## 0.13.3 (2026-11-12)', '',
  '- **New: a panel.** It lists',
  '  things.', '',
  '  Upgrade: roll your Developer Agents after the upgrade.', '',
  '  Verified: on a stack.', '',
  '- **Fixed:** the crash on reload. And more prose.', '',
  '## 0.13.2 (2026-11-10)', '', '- **Older.**', '',
].join('\n')

check('a section is its body without the heading, and splits into entries', () => {
  const s = sectionOf(SECTION_TEXT, '0.13.3')
  eq(s[0], '- **New: a panel.** It lists')
  eq(entriesOf(s).length, 2)
  eq(sectionOf(SECTION_TEXT, '0.9.9'), null)
})
check('a lead is the bold span, or a bare label and its first sentence', () => {
  const [a, b] = entriesOf(sectionOf(SECTION_TEXT, '0.13.3'))
  eq(leadOf(a), 'New: a panel.')
  eq(leadOf(b), 'Fixed: the crash on reload.')
})
check('Upgrade: paragraphs are gathered without the label, across wrapped lines', () =>
  eq(upgradesOf(['- **a.**', '', '  Upgrade: roll the', '  agents.', '']), ['roll the agents.']))
check('migrations are counted the way check-invariants counts them', () => {
  const src = [
    'const MIGRATIONS: string[] = [',
    '  // `comment` with backticks is not a statement',
    '  `create table a (id int)`,',
    '  /* a block',
    '     `still not one` */',
    '  `alter table a add column b int`,',
    ']',
  ].join('\n')
  eq(migrationCount(src), 2)
  eq(migrationCount('nothing here'), null)
})
check('the notes carry the migration count, upgrade steps and pull requests', () => {
  const section = sectionOf(SECTION_TEXT, '0.13.3')
  const prs = new Map([[section[0], 731]])
  const body = render({ version: '0.13.3', repo: 'o/t', section, prs, migrations: { before: 558, after: 560 }, desktop: 'desktop-v0.12.0', prev: 'v0.13.2' })
  for (const want of [
    'Talaria 0.13.3: 2 changes since v0.13.2.',
    '- **Images:** `ghcr.io/o/t:0.13.3` and the moving `latest`.',
    '- **Pull requests:** #731.',
    '- [ ] **2 database migrations run on start.** Take a backup first.',
    '- [ ] roll your Developer Agents after the upgrade. (#731)',
    '- New: a panel. (#731)',
    '- Fixed: the crash on reload.',
    'blob/v0.13.3/CHANGELOG.md',
  ])
    if (!body.includes(want)) throw new Error(`missing ${JSON.stringify(want)} in:\n${body}`)
})
check('with nothing to do, the notes say so; an RC names the rc tag', () => {
  const section = ['- **Fixed: a.**']
  const body = render({ version: '0.14.0-rc.1', repo: 'o/t', section, prs: new Map(), migrations: { before: 1, after: 1 }, desktop: null, prev: 'v0.13.2' })
  if (!body.includes('- **No database migrations.**\n- No entry in this release asks for a step')) throw new Error(body)
  if (!body.includes('and the moving `rc`')) throw new Error(body)
})
check('a section past the body limit keeps the list and drops the full text', () => {
  const section = ['- **Big.**', '', `  ${'x'.repeat(130_000)}`]
  const body = render({ version: '1.0.0', repo: 'o/t', section, prs: new Map(), migrations: null, desktop: null, prev: null })
  if (body.length > 120_000 || body.includes('<details>') || !body.includes('- Big.')) throw new Error(`length ${body.length}`)
})

if (failed) {
  console.error(`release-chain: ${failed} of ${ran} failed`)
  process.exit(1)
}
console.log(`release-chain: ${ran} checks pass`)
