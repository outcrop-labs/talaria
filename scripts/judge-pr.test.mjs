// The judge's pull-request-body reading: what counts as a filled section.
// The reader this replaced returned "" for every section, so the judge asked
// "no Verified section filled in" on every pull request opened from the
// template — filled or not — and filling it in never cleared the ask. A wrong
// answer the other way waves an untouched template through as an answer.
// Then the lanes, on a scratch repository: what the judge skips (promotions,
// snapshots, back-flows, syncs; the CHANGELOG.md checks on a roll), the musts
// that guard them (a hotfix migration, a promoted entry rewritten, a feature
// aimed at rc), and the diff base it picks when none is said.
// Run by `bun run check`.
import { spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, unlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { answer, section } from './pr-body.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const TEMPLATE = readFileSync(join(ROOT, '.github/PULL_REQUEST_TEMPLATE.md'), 'utf8')
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

function ok(cond, what) {
  if (!cond) throw new Error(what)
}

/** The template as an author leaves it after answering: the labels carry
 *  text, the changelog prompt is replaced by the entry. */
const FILLED = TEMPLATE.replace('- **Gate:**', '- **Gate:** `bun run gate` passed (check + the cli suite)')
  .replace('- **Exercised:**', '- **Exercised:** ran `talaria worktree probe` and read the seeded rows back')
  .replace(
    '<!-- Paste the entry from your changelog/ file, or say why the change is internal-only. -->',
    '- **The worktree waits for Postgres over TCP.** Verified: `bun run gate`, a real worktree.',
  )

check('the template still has the sections the judge reads', () => {
  ok(section(TEMPLATE, 'Verified') !== null, 'no ## Verified in the template')
  ok(section(TEMPLATE, 'CHANGELOG entry') !== null, 'no ## CHANGELOG entry in the template')
})

check('the template\'s checklist asks for a changelog/ file, never a hand edit of CHANGELOG.md', () => {
  // CHANGELOG.md is append-only and written by the roll (scripts/changelog-roll.mjs);
  // `bun run check` fails a hand-appended [Unreleased] bullet. A checklist that
  // names it sends the author to the one file they must not touch.
  const boxes = section(TEMPLATE, 'Checklist')
    .split('\n')
    .reduce((items, line) => {
      if (/^\s*- \[ \]/.test(line)) items.push(line)
      else if (items.length && /^\s+\S/.test(line)) items[items.length - 1] += ` ${line.trim()}`
      return items
    }, [])
  const entry = boxes.filter((b) => /changelog/i.test(b))
  eq(entry.length, 1)
  // It links the doc that explains an entry, never the directory: right after a
  // roll changelog/ is empty, git tracks nothing there, and a link to it has no
  // target (doc-link-target-missing).
  ok(/\[CONTRIBUTING\.md\]\(\.\.\/CONTRIBUTING\.md\)/.test(entry[0]), `the changelog item links no entry recipe: ${entry[0]}`)
  ok(!/\]\(\.\.\/changelog\/?\)/.test(entry[0]), `the changelog item links the changelog/ directory, which a roll empties: ${entry[0]}`)
  ok(/changelog\/YYYY-MM-DD-<slug>\.md/.test(entry[0]), `the changelog item gives no file name: ${entry[0]}`)
  ok(!/CHANGELOG\.md\S* carries/.test(entry[0]), `the changelog item still asks for CHANGELOG.md: ${entry[0]}`)
  ok(/written only by the release roll/.test(entry[0]), entry[0])
})

check('a filled Verified section reads as its text, not "" (the old reader always said "")', () => {
  const v = answer(FILLED, 'Verified', TEMPLATE)
  eq(v, [
    '- **Gate:** `bun run gate` passed (check + the cli suite)',
    '- **Exercised:** ran `talaria worktree probe` and read the seeded rows back',
  ].join('\n'))
})

check('the untouched template answers nothing: labels, paragraphs, code block and comment are scaffold', () => {
  // The section itself is long — the template writes a lot under Verified —
  // which is exactly why "filled" cannot be a length test on the raw section.
  ok(section(TEMPLATE, 'Verified').length > 200, 'the raw Verified section is expected to be long')
  eq(answer(TEMPLATE, 'Verified', TEMPLATE), '')
  eq(answer(TEMPLATE, 'CHANGELOG entry', TEMPLATE), '')
})

check('one label answered is an answer; the other, still empty, is dropped', () => {
  const body = TEMPLATE.replace('- **Gate:**', '- **Gate:** bun run gate')
  eq(answer(body, 'Verified', TEMPLATE), '- **Gate:** bun run gate')
})

check('the last section in the body reads to the end of the input', () => {
  eq(answer(FILLED, 'CHANGELOG entry', TEMPLATE), '- **The worktree waits for Postgres over TCP.** Verified: `bun run gate`, a real worktree.')
  eq(section('## Verified\nran it, twice', 'Verified'), 'ran it, twice')
})

check('CRLF bodies (GitHub web edits) read the same as LF', () => {
  const crlf = FILLED.replace(/\n/g, '\r\n')
  eq(answer(crlf, 'Verified', TEMPLATE), answer(FILLED, 'Verified', TEMPLATE))
  eq(answer(crlf, 'CHANGELOG entry', TEMPLATE), answer(FILLED, 'CHANGELOG entry', TEMPLATE))
  eq(answer(TEMPLATE.replace(/\n/g, '\r\n'), 'Verified', TEMPLATE), '')
})

check('a section ends at the next level-2 heading, not at a level-3 one', () => {
  const body = '## Verified\n\n### Gate\n\nbun run gate\n\n## Branch flow\n\nno\n'
  eq(section(body, 'Verified'), '### Gate\n\nbun run gate')
  eq(section(body, 'Branch flow'), 'no')
})

check('a `## ` line inside a code fence does not cut the section', () => {
  const body = '## Verified\n\n```md\n## not a heading\n```\nexercised it\n\n## Next\n'
  eq(section(body, 'Verified'), '```md\n## not a heading\n```\nexercised it')
})

check('a fence closes on a bare run at least as long as its opener, never on an info string or a shorter run', () => {
  // A ```bash line inside an open fence is text. Closing on it would read `## inner`
  // as a section, then reopen a fence that swallows the real heading after it.
  const body = '## Verified\n```\n```bash\n## inner\n```\nx\n## CHANGELOG entry\ny\n'
  eq(section(body, 'Verified'), '```\n```bash\n## inner\n```\nx')
  eq(section(body, 'inner'), null)
  eq(section(body, 'CHANGELOG entry'), 'y')
  const longer = '## Verified\n````md\n```\n## not yet\n````\nran it\n## Next\nz\n'
  eq(section(longer, 'Verified'), '````md\n```\n## not yet\n````\nran it')
  eq(section(longer, 'Next'), 'z')
  // tildes and backticks do not close each other
  eq(section('## Verified\n~~~\n```\n## no\n~~~\n## Next\nz\n', 'Next'), 'z')
})

check('a heading inside an HTML comment does not render, so it does not count', () => {
  eq(section('<!--\n## Verified\nhidden\n-->\n## Other\n', 'Verified'), null)
})

check('heading names: case-insensitive, closing hashes, and `##Verified` is not a heading', () => {
  eq(section('## verified ##\nran it\n', 'Verified'), 'ran it')
  eq(section('##Verified\nran it\n', 'Verified'), null)
})

check('no such heading is null (a custom body is not an unfilled template)', () => {
  eq(answer('## Summary\n\nDid a thing.\n', 'Verified', TEMPLATE), null)
  eq(answer('', 'Verified', TEMPLATE), null)
})

check('a bullet that is only a bold sentence is an answer; `- **Gate**:` with nothing after is not', () => {
  eq(answer('## CHANGELOG entry\n\n- **Fixed the reader.**\n', 'CHANGELOG entry', TEMPLATE), '- **Fixed the reader.**')
  eq(answer('## Verified\n\n- **Gate**:\n- **Exercised:**  \n', 'Verified', TEMPLATE), '')
})

check('with no template on disk, the labels still do not count', () => {
  eq(answer('## Verified\n\n- **Gate:**\n- **Exercised:**\n', 'Verified', ''), '')
})

// ── the judge itself, end to end, on an empty diff ─────────────────────────
//
// `--base HEAD` makes the range empty, so the only findings are the body's.

function judge(body) {
  const dir = mkdtempSync(join(tmpdir(), 'judge-pr-body-'))
  try {
    const file = join(dir, 'body.md')
    writeFileSync(file, body)
    // Not the runner's own target: on the promotion pull request CI sets
    // GITHUB_BASE_REF=main, and a judge that inherited it would add
    // `targets-main` (a must, exit 2) to every body below.
    const env = { ...process.env }
    delete env.GITHUB_BASE_REF
    delete env.GITHUB_HEAD_REF
    const run = spawnSync(process.execPath, ['scripts/judge-pr.mjs', '--base', 'HEAD', '--pr-body', file, '--json'], {
      cwd: ROOT,
      encoding: 'utf8',
      env,
    })
    if (run.status !== 0) throw new Error(`judge-pr exited ${run.status}: ${run.stderr}`)
    return JSON.parse(run.stdout).findings.map((f) => f.id).sort()
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}

check('judge: the untouched template asks for Verified and for the entry', () => {
  eq(judge(TEMPLATE), ['pr-body-no-entry', 'pr-body-unfilled'])
})

check('judge: a filled body asks nothing about the body', () => {
  eq(judge(FILLED), [])
  eq(judge(FILLED.replace(/\n/g, '\r\n')), [])
})

check('judge: a body without the template headings is not judged as an unfilled template', () => {
  eq(judge('## Summary\n\nWhat changed.\n\n## Proof\n\nWhat ran.\n'), [])
})

check('judge: an empty body is named as empty', () => {
  eq(judge(''), ['empty-pr-body'])
})

// ── the lanes, end to end, on a scratch repository ─────────────────────────
//
// rc and testing are seeded, and origin/rc and origin/testing point at them.
// Each case is a branch with one commit; the judge runs in the scratch repo
// (it judges the tree it stands in) with the target and source said by hand.

const JUDGE = join(ROOT, 'scripts/judge-pr.mjs')
const repo = mkdtempSync(join(tmpdir(), 'judge-pr-lanes-'))
// The runner's own pull request must not leak in: on a promotion CI sets
// GITHUB_BASE_REF=main and GITHUB_HEAD_REF=rc.
const laneEnv = Object.fromEntries(
  Object.entries(process.env).filter(([k]) => !k.startsWith('GIT_') && !['GITHUB_BASE_REF', 'GITHUB_HEAD_REF', 'GITHUB_STEP_SUMMARY'].includes(k)),
)
function rgit(...args) {
  const run = spawnSync(
    'git',
    ['-c', 'user.name=judge', '-c', 'user.email=judge@example.invalid', '-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgsign=false', ...args],
    { cwd: repo, encoding: 'utf8', env: laneEnv },
  )
  if (run.status !== 0) throw new Error(`git ${args.join(' ')}: ${run.stderr}`)
  return run.stdout.trim()
}
const put = (path, text) => {
  mkdirSync(dirname(join(repo, path)), { recursive: true })
  writeFileSync(join(repo, path), text)
}
const PG_PATH = 'ui/src/server/db/pg.ts'
const PG = [
  "import { x } from './x'",
  '',
  'export function helper() {',
  '  return 1',
  '}',
  '',
  'const MIGRATIONS: string[] = [',
  "  `create table a (id int, tags text[] not null default '{}')`,",
  "  // a comment with a ] and an apostrophe's [",
  "  'select array[1, 2]',",
  ']',
  '',
  'export function after() {',
  '  return MIGRATIONS.length',
  '}',
  '',
].join('\n')
const PG_ARRAY_END = 11 // the `]` line; a line added before it is inside the array

rgit('init', '-q')
rgit('symbolic-ref', 'HEAD', 'refs/heads/rc')
put(PG_PATH, PG)
put('ui/src/app.ts', 'export const app = 1\n')
put('changelog/README.md', '# entries\n')
put('changelog/2026-01-01-promoted.md', '- **Already on rc.** Verified: `bun run gate`.\n')
put('CHANGELOG.md', '# Changelog\n\n## [Unreleased]\n')
rgit('add', '-A')
rgit('commit', '-q', '-m', 'base: seed the fixture — the lanes start here')
rgit('update-ref', 'refs/remotes/origin/rc', 'HEAD')
rgit('switch', '-q', '-c', 'testing')
put('changelog/2026-01-02-testing-only.md', '- **Only on testing.** Verified: CI.\n')
rgit('add', '-A')
rgit('commit', '-q', '-m', 'changelog: an entry rc does not have yet — testing is ahead')
rgit('update-ref', 'refs/remotes/origin/testing', 'HEAD')

/** A branch off `from` with one commit made by `edit`. */
function branch(name, from, edit, subject = 'ui: a change for the judge — the fixture needs one') {
  rgit('switch', '-q', '-c', name, from)
  edit()
  rgit('add', '-A')
  rgit('commit', '-q', '-m', subject)
}
const lines = (text) => text.split('\n')
const withLine = (text, at, line) => {
  const l = lines(text)
  l.splice(at - 1, 0, line)
  return l.join('\n')
}
// pg.ts is user-visible: the hotfixes carry an entry and a test, so the only
// finding left to see is the one under test.
const entryAndTest = (name) => {
  put(`changelog/2026-01-04-${name}.md`, `- **Hotfix ${name}.** Verified: CI.\n`)
  put('ui/src/server/db/pg.test.ts', `// ${name}\n`)
}
// a migration appended inside the array (before its closing `]`)
branch('hotfix/mig', 'rc', () => {
  put(PG_PATH, withLine(PG, PG_ARRAY_END, "  'create table b (id int)',"))
  entryAndTest('mig')
})
// pg.ts code outside the array: above it, and below it
branch('hotfix/code', 'rc', () => {
  put(PG_PATH, PG.replace('  return 1', '  return 2').replace('  return MIGRATIONS.length', '  return MIGRATIONS.length + 0'))
  entryAndTest('code')
})
// testing: a promoted entry rewritten, deleted, and the safe edits
branch('edit/promoted', 'testing', () => put('changelog/2026-01-01-promoted.md', '- **Already on rc, reworded.** Verified: CI.\n'), 'changelog: reword an entry — the fixture needs one')
branch('delete/promoted', 'testing', () => unlinkSync(join(repo, 'changelog/2026-01-01-promoted.md')), 'changelog: drop an entry — the fixture needs one')
branch('edit/unpromoted', 'testing', () => {
  put('changelog/2026-01-02-testing-only.md', '- **Only on testing, reworded.** Verified: CI.\n')
  put('changelog/README.md', '# entries, one per change\n')
}, 'changelog: reword an unpromoted entry — the fixture needs one')
// the roll: CHANGELOG.md takes the bullets (and here a ui/ change, so the CHANGELOG checks run at all)
branch('release/roll', 'rc', () => {
  put('CHANGELOG.md', '# Changelog\n\n## [Unreleased]\n\n## 0.1.0\n\n- **Already on rc.** Verified: `bun run gate`.\n')
  unlinkSync(join(repo, 'changelog/2026-01-01-promoted.md'))
  put('ui/src/app.ts', 'export const app = 2\n')
}, 'changelog: roll 0.1.0 — the fixture needs one')
// ui changes whose entry names its verification several ways
const claim = (name, verified) =>
  branch(`claim/${name}`, 'testing', () => {
    put('ui/src/app.ts', `export const app = '${name}'\n`)
    put(`changelog/2026-01-03-${name}.md`, `- **The app says ${name}.** ${verified}\n`)
  })
claim('remote', 'Verified: `bun run gate:remote` green.')
claim('ci', "Verified: CI: this PR's checks.")
claim('nothing', 'Verified: read it twice.')
claim('ciyml', 'Verified: the ci.yml step ran.')
rgit('switch', '-q', 'rc')

function judgeLane({ head, base = 'origin/rc', target, source, json = true, env = {} }) {
  // base: null says no --base at all, so the judge picks its own.
  const args = [JUDGE, ...(base === null ? [] : ['--base', base]), '--head', head]
  if (target) args.push('--target', target)
  if (source) args.push('--source', source)
  if (json) args.push('--json')
  const run = spawnSync(process.execPath, args, { cwd: repo, encoding: 'utf8', env: { ...laneEnv, ...env } })
  if (run.status !== 0 && run.status !== 2) throw new Error(`judge-pr exited ${run.status}: ${run.stderr}`)
  return json ? { status: run.status, ...JSON.parse(run.stdout) } : { status: run.status, stdout: run.stdout }
}
const ids = (r) => r.findings.map((f) => f.id).sort()

check('lanes: a promotion, a snapshot, a back-flow and a sync are skipped — no findings, exit 0', () => {
  // Each on a diff that otherwise draws a must, so a skip that did not happen shows.
  const lanesSeen = []
  for (const [target, source, head] of [
    ['main', 'rc', 'release/roll'],
    ['rc', 'promote/testing-0123456789ab', 'hotfix/mig'],
    ['testing', 'backflow/rc-0123456789ab', 'edit/promoted'],
    ['rc', 'sync/main-into-rc', 'hotfix/mig'],
  ]) {
    const base = target === 'testing' ? 'origin/testing' : 'origin/rc'
    const r = judgeLane({ head, base, target, source })
    eq([r.status, r.findings, r.counts], [0, [], { must: 0, should: 0, ask: 0 }])
    ok(typeof r.lane === 'string' && r.lane.length > 0, JSON.stringify(r))
    lanesSeen.push(r.lane)
    const plain = judgeLane({ head, base, target, source, json: false })
    eq(plain.status, 0)
    ok(plain.stdout.includes(`${r.lane} — each change was judged in its own pull request.`), plain.stdout)
  }
  eq(new Set(lanesSeen).size, 4)
})

check('lanes: --summary appends the lane report to the job summary', () => {
  const summary = join(repo, '.summary.md')
  writeFileSync(summary, '')
  try {
    const r = judgeLane({ head: 'release/roll', target: 'main', source: 'rc', json: false, env: { GITHUB_STEP_SUMMARY: summary } })
    eq(r.status, 0)
    // --summary was not passed: nothing written
    eq(readFileSync(summary, 'utf8'), '')
    const args = [JUDGE, '--base', 'origin/rc', '--head', 'release/roll', '--target', 'main', '--source', 'rc', '--summary']
    const run = spawnSync(process.execPath, args, { cwd: repo, encoding: 'utf8', env: { ...laneEnv, GITHUB_STEP_SUMMARY: summary } })
    eq(run.status, 0)
    ok(/the promotion of `rc` into `main` — each change was judged in its own pull request\./.test(readFileSync(summary, 'utf8')), readFileSync(summary, 'utf8'))
  } finally {
    rmSync(summary, { force: true })
  }
})

check('lanes: GITHUB_HEAD_REF is the default source, as CI sets it', () => {
  const r = judgeLane({ head: 'hotfix/mig', env: { GITHUB_BASE_REF: 'rc', GITHUB_HEAD_REF: 'promote/testing-abc' } })
  eq([r.status, r.findings.length, typeof r.lane], [0, 0, 'string'])
})

check('lanes: anything else into main is still targets-main — a fork\'s rc included', () => {
  for (const source of ['wt/x', 'someone/talaria:rc', undefined]) {
    const r = judgeLane({ head: 'hotfix/code', target: 'main', source })
    eq([r.status, ids(r).includes('targets-main'), r.lane], [2, true, null])
  }
})

check('lanes: a pull request into testing draws no targets-testing finding', () => {
  const r = judgeLane({ head: 'claim/remote', base: 'origin/testing', target: 'testing', source: 'wt/x' })
  ok(!ids(r).includes('targets-testing'), ids(r).join(' '))
  eq(r.counts.must, 0)
})

check('the roll (release/* into rc) does not trip changelog-not-a-file; anything else appending to CHANGELOG.md does', () => {
  const roll = judgeLane({ head: 'release/roll', target: 'rc', source: 'release/roll' })
  ok(!ids(roll).includes('changelog-not-a-file'), ids(roll).join(' '))
  eq(roll.counts.must, 0)
  const other = judgeLane({ head: 'release/roll', target: 'rc', source: 'wt/x' })
  ok(ids(other).includes('changelog-not-a-file'), ids(other).join(' '))
  // a release/ head anywhere but rc is not the roll
  ok(ids(judgeLane({ head: 'release/roll', target: 'testing', source: 'release/roll' })).includes('changelog-not-a-file'), 'release/ into testing')
})

check('hotfix-adds-migration: a line inside MIGRATIONS on a hotfix into rc is a must', () => {
  const r = judgeLane({ head: 'hotfix/mig', target: 'rc', source: 'hotfix/mig' })
  eq([r.status, ids(r)], [2, ['hotfix-adds-migration']])
  const f = r.findings[0]
  eq(f.severity, 'must')
  ok(/a hotfix may not add migrations: the array index is a migration's identity and testing's array would diverge\. Land it through testing\./.test(f.what), f.what)
  ok(/line\(s\) 11 inside MIGRATIONS \(lines 7–12\)/.test(f.detail[0]), f.detail[0])
})

check('hotfix-adds-migration: pg.ts code outside the array, or a migration that is not a hotfix into rc, is not', () => {
  eq(ids(judgeLane({ head: 'hotfix/code', target: 'rc', source: 'hotfix/code' })), [])
  // a feature into rc is the wrong door (its own must), not a hotfix
  eq(ids(judgeLane({ head: 'hotfix/mig', target: 'rc', source: 'wt/x' })), ['feature-targets-rc'])
  eq(ids(judgeLane({ head: 'hotfix/mig', target: 'testing', source: 'hotfix/mig' })).includes('hotfix-adds-migration'), false)
  // no source said: not a hotfix — and not a lane into rc either
  eq(ids(judgeLane({ head: 'hotfix/mig', target: 'rc' })), ['feature-targets-rc'])
})

check('feature-targets-rc: a feature into rc is a must, with the retarget to testing', () => {
  for (const source of ['wt/x', 'agent/x', 'backflow/rc-0123456789ab', 'someone/talaria:hotfix/x', undefined]) {
    const r = judgeLane({ head: 'hotfix/code', target: 'rc', source })
    eq([source, r.status, ids(r)], [source, 2, ['feature-targets-rc']])
    const f = r.findings[0]
    eq(f.severity, 'must')
    ok(f.fix.some((l) => l.includes('gh pr edit <n> --base testing')), JSON.stringify(f.fix))
  }
})

check('feature-targets-rc: the lanes into rc, and anything into testing, are not', () => {
  for (const source of ['hotfix/code', 'release/roll-0.1.0', 'sync/main-into-rc', 'promote/testing-0123456789ab']) {
    ok(!ids(judgeLane({ head: 'hotfix/code', target: 'rc', source })).includes('feature-targets-rc'), source)
  }
  ok(!ids(judgeLane({ head: 'claim/remote', base: 'origin/testing', target: 'testing', source: 'wt/x' })).includes('feature-targets-rc'), 'into testing')
  // no target said: a local run is not a claim about where the change goes
  ok(!ids(judgeLane({ head: 'hotfix/code' })).includes('feature-targets-rc'), 'no target')
})

check('feature-targets-rc: dormant while origin/rc predates the rule, as pr-target is', () => {
  // rc's own flow-guard decides: without its rc-head rule, rc still takes features
  const rc = rgit('rev-parse', 'refs/remotes/origin/rc')
  const withGuard = (text) => {
    rgit('switch', '-q', '--detach', rc)
    put('scripts/flow-guard.mjs', text)
    rgit('add', 'scripts/flow-guard.mjs')
    rgit('commit', '-q', '-m', 'flow: a guard on rc — the fixture needs one')
    rgit('update-ref', 'refs/remotes/origin/rc', 'HEAD')
    rgit('switch', '-q', 'rc')
  }
  try {
    withGuard('// the policy before the cutover\nexport function decidePrBase() {}\n')
    ok(!ids(judgeLane({ head: 'hotfix/code', target: 'rc', source: 'wt/x' })).includes('feature-targets-rc'), 'before the cutover')
    withGuard('// the policy after it\nfunction decideRcHead() {}\n')
    ok(ids(judgeLane({ head: 'hotfix/code', target: 'rc', source: 'wt/x' })).includes('feature-targets-rc'), 'after the cutover')
  } finally {
    rgit('update-ref', 'refs/remotes/origin/rc', rc)
  }
})

check('targets-main: the fix says retarget at testing', () => {
  const f = judgeLane({ head: 'hotfix/code', target: 'main', source: 'wt/x' }).findings.find((x) => x.id === 'targets-main')
  ok(f.fix.join(' ').includes('Retarget the pull request at `testing`'), JSON.stringify(f.fix))
})

check('the diff base: the pull request\'s own base in CI, else origin/testing, else origin/rc', () => {
  // CI: GITHUB_BASE_REF names it
  eq(judgeLane({ head: 'claim/remote', base: null, env: { GITHUB_BASE_REF: 'rc' } }).base, 'rc')
  eq(judgeLane({ head: 'claim/remote', base: null, env: { GITHUB_BASE_REF: 'testing' } }).base, 'testing')
  // locally: testing, which this clone has
  eq(judgeLane({ head: 'claim/remote', base: null }).base, 'testing')
  // a clone without origin/testing falls back to rc
  const testing = rgit('rev-parse', 'refs/remotes/origin/testing')
  rgit('update-ref', '-d', 'refs/remotes/origin/testing')
  try {
    eq(judgeLane({ head: 'hotfix/code', base: null }).base, 'rc')
  } finally {
    rgit('update-ref', 'refs/remotes/origin/testing', testing)
  }
})

check('the diff base: one that does not resolve could not run, and names the fetch', () => {
  const run = spawnSync(process.execPath, [JUDGE, '--base', 'origin/nope', '--head', 'hotfix/code'], { cwd: repo, encoding: 'utf8', env: laneEnv })
  eq(run.status, 1)
  ok(run.stderr.includes('git fetch origin testing rc main'), run.stderr)
})

check('changelog-entry-rewritten: editing or deleting an entry origin/rc has, into testing, is a must', () => {
  for (const head of ['edit/promoted', 'delete/promoted']) {
    const r = judgeLane({ head, base: 'origin/testing', target: 'testing', source: head })
    eq([r.status, ids(r)], [2, ['changelog-entry-rewritten']])
    ok(/rc's roll owns it from here\. Add a new entry instead — an edit makes the rc → testing back-flow conflict\./.test(r.findings[0].what), r.findings[0].what)
    eq(r.findings[0].detail, ['  changelog/2026-01-01-promoted.md (on origin/rc)'])
  }
  // into rc it is not this rule's business
  eq(ids(judgeLane({ head: 'edit/promoted', base: 'origin/testing', target: 'rc', source: 'wt/x' })).includes('changelog-entry-rewritten'), false)
})

check('changelog-entry-rewritten: an entry rc does not have yet, and the README, are free to edit', () => {
  const r = judgeLane({ head: 'edit/unpromoted', base: 'origin/testing', target: 'testing', source: 'wt/x' })
  eq([r.status, ids(r)], [0, []])
})

check('claims: `gate:remote` and "CI: this PR\'s checks" cover the surface; nothing, or a lowercase ci.yml, does not', () => {
  const claimIds = (name) => ids(judgeLane({ head: `claim/${name}`, base: 'origin/testing', target: 'testing', source: `claim/${name}` }))
  ok(!claimIds('remote').includes('claim-misses-surface'), claimIds('remote').join(' '))
  ok(!claimIds('ci').includes('claim-misses-surface'), claimIds('ci').join(' '))
  ok(claimIds('nothing').includes('claim-misses-surface'), claimIds('nothing').join(' '))
  ok(claimIds('ciyml').includes('claim-misses-surface'), claimIds('ciyml').join(' '))
  const hint = judgeLane({ head: 'claim/nothing', base: 'origin/testing', target: 'testing' }).findings.find((f) => f.id === 'claim-misses-surface')
  eq(hint.fix, ["Name the gate: `bun run gate:remote`, or 'CI: this PR's checks' (put run links in the PR body, not the entry)."])
})

rmSync(repo, { recursive: true, force: true })

if (failed) {
  console.error(`judge-pr: ${failed} of ${ran} failed`)
  process.exit(1)
}
console.log(`judge-pr: ${ran} checks, all clean`)
