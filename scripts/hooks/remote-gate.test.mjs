// The remote gate's decisions: which ref a branch offloads to, which run is
// the push's own, and when a run is green, red, cancelled or still running.
// A wrong answer is a wrong "done" — an old run's green read back for a new
// push, or a red held until the slowest job finishes. Pure: no network, no
// GitHub. The end-to-end checks drive the script in a scratch repository with
// no remote, and stop before anything would leave the machine. Run by
// `bun run check`.
import { execFileSync, spawnSync } from 'node:child_process'
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  BASES,
  TOKENS,
  branchOfRef,
  jobLines,
  modeOfRef,
  needsFull,
  nextDelay,
  parseArgs,
  pickNewRun,
  proofLine,
  refFor,
  tokenFrom,
  trimLog,
  verdictOf,
  pickReviewRun,
  reviewClause,
  reviewFindingLines,
  reviewFromAnnotations,
  REVIEW_WAIT_MS,
} from './remote-gate.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
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

function ok(cond, msg) {
  if (!cond) throw new Error(msg)
}

// ── the command line ────────────────────────────────────────────────────────

check('parseArgs: the defaults, every flag, and both value forms', () => {
  eq(parseArgs([]), {
    full: false,
    detach: false,
    watch: null,
    rerun: false,
    json: false,
    timeoutMin: 60,
    keepRef: false,
    noCheck: false,
    noReview: false,
    help: false,
  })
  const all = parseArgs(['--full', '--detach', '--rerun', '--json', '--keep-ref', '--no-check', '--no-review', '--timeout', '15'])
  eq([all.full, all.detach, all.rerun, all.json, all.keepRef, all.noCheck, all.noReview, all.timeoutMin], [true, true, true, true, true, true, true, 15])
  eq(parseArgs(['--watch', '42951152974']).watch, 42951152974)
  eq(parseArgs(['--watch=42951152974']).watch, 42951152974)
  eq(parseArgs(['--timeout=1.5']).timeoutMin, 1.5)
  eq(parseArgs(['-h']).help, true)
  eq(parseArgs(['--rerun', '--watch', '7']).watch, 7)
})

check('parseArgs: an unknown flag, a stray word or a bad value is an error, never a quiet default', () => {
  for (const argv of [
    ['--ful'],
    ['--bogus'],
    ['wt/x'],
    ['--watch'],
    ['--watch', 'abc'],
    ['--watch', '--full'],
    ['--timeout', '0'],
    ['--timeout', 'soon'],
    ['--full=yes'],
    ['--detach', '--watch', '9'],
  ]) {
    const r = parseArgs(argv)
    ok(typeof r.error === 'string' && r.error.length > 0, `${argv.join(' ')} → ${JSON.stringify(r)}`)
  }
  ok(/unknown argument --ful/.test(parseArgs(['--ful']).error), 'names the argument')
})

// ── the ref ─────────────────────────────────────────────────────────────────

check('refFor: ci/<branch>, ci/full/<branch>, and a ci/ branch keeps its name', () => {
  eq(refFor('wt/x', false), 'ci/wt/x')
  eq(refFor('wt/x', true), 'ci/full/wt/x')
  eq(refFor('ci/foo', false), 'ci/foo')
  eq(refFor('ci/foo', true), 'ci/foo')
  eq(refFor('detached-0123456789ab', false), 'ci/detached-0123456789ab')
})

check('needsFull: each lane into rc, and nothing else', () => {
  for (const prefix of ['hotfix/', 'release/', 'sync/', 'promote/', 'backflow/']) eq(needsFull(`${prefix}x`), true)
  for (const branch of ['wt/x', 'feat/hotfix', 'agent/release-notes', 'fix/sync', 'testing', 'hotfix']) eq(needsFull(branch), false)
})

check('modeOfRef and branchOfRef: what ci-offload.yml makes of a ref', () => {
  eq(modeOfRef('ci/wt/x'), 'light')
  eq(modeOfRef('ci/full/wt/x'), 'full')
  eq(modeOfRef('ci/hotfix/x'), 'full') // surfaces.mjs runs that lane full even pushed light
  eq(modeOfRef('ci/full/hotfix/x'), 'full')
  eq(branchOfRef('ci/wt/x'), 'wt/x')
  eq(branchOfRef('ci/full/wt/x'), 'wt/x')
  eq(BASES, ['main', 'rc', 'testing'])
})

// ── which run is the push's ─────────────────────────────────────────────────

const SHA = '5beeda7cb23cb8724ceb1af4bd906269bde656b2'
const OTHER = '37cacb388f29d86bbc321397847697f352108163'
const runRow = (id, over = {}) => ({
  id,
  name: 'CI offload',
  path: '.github/workflows/ci-offload.yml',
  head_branch: 'ci/wt/x',
  head_sha: SHA,
  event: 'push',
  status: 'in_progress',
  conclusion: null,
  run_attempt: 1,
  html_url: `https://github.com/outcrop-labs/talaria/actions/runs/${id}`,
  ...over,
})

check('pickNewRun: a run the ref had before the push is never the push\'s', () => {
  // The old green of the same commit (the ref was deleted and pushed again).
  const old = runRow(100, { status: 'completed', conclusion: 'success' })
  eq(pickNewRun([old], SHA, [100]), null)
  eq(pickNewRun([old, runRow(101)], SHA, [100])?.id, 101)
  // Snapshot ids may come back as numbers or strings.
  eq(pickNewRun([old, runRow(101)], SHA, ['100'])?.id, 101)
})

check('pickNewRun: another commit, another workflow, and the newest of several', () => {
  eq(pickNewRun([runRow(101, { head_sha: OTHER })], SHA, []), null)
  eq(pickNewRun([runRow(101, { path: '.github/workflows/ci.yml', name: 'CI' })], SHA, []), null)
  eq(pickNewRun([runRow(101), runRow(105), runRow(103)], SHA, [])?.id, 105)
  // A run row with no path (an older API shape) is matched by name.
  eq(pickNewRun([{ ...runRow(9), path: undefined }], SHA, [])?.id, 9)
  eq(pickNewRun(undefined, SHA, []), null)
})

// ── the verdict ─────────────────────────────────────────────────────────────

const job = (name, status, conclusion = null, id = name.length) => ({
  id,
  name,
  status,
  conclusion,
  html_url: `https://github.com/outcrop-labs/talaria/actions/runs/1/job/${id}`,
})
// The shape of run 42951152974 (light, 2026-11-11): the prune skipped outside
// the call, the platform legs skipped inside it.
const JOBS_GREEN = [
  job('offload / what changed', 'completed', 'success', 1),
  job('offload / invariants (no deps)', 'completed', 'success', 2),
  job('prune stale ci/** refs', 'completed', 'skipped', 3),
  job('offload / test + typecheck (ui)', 'completed', 'success', 4),
  job('offload / fmt + clippy + test (api)', 'completed', 'success', 5),
  job('offload / compile + link (mobile ios)', 'completed', 'skipped', 6),
]

check('jobLines: ran, skipped, failing and pending, with the call prefix dropped and the prune not counted', () => {
  const split = jobLines([
    ...JOBS_GREEN.slice(0, 4),
    job('offload / fmt + clippy + test (api)', 'in_progress', null, 5),
    job('offload / typecheck + test (cli)', 'completed', 'failure', 7),
    job('offload / mcp end-to-end (playwright)', 'queued', null, 8),
    job('offload / compile + link (mobile ios)', 'completed', 'skipped', 6),
    job('offload / test + assemble (mobile)', 'completed', 'timed_out', 9),
  ])
  eq(split.ran, ['what changed', 'invariants (no deps)', 'test + typecheck (ui)'])
  eq(split.skipped, ['compile + link (mobile ios)'])
  eq(split.pending, ['fmt + clippy + test (api)', 'mcp end-to-end (playwright)'])
  eq(split.failing.map((j) => `${j.name}:${j.conclusion}:${j.id}`), ['typecheck + test (cli):failure:7', 'test + assemble (mobile):timed_out:9'])
  ok(split.failing[0].url.endsWith('/job/7'), 'a failing job carries its url')
  eq(jobLines(undefined), { ran: [], skipped: [], failing: [], pending: [] })
})

check('verdictOf: one job failed while the others run is red now, not when the api job ends', () => {
  const v = verdictOf(runRow(1), [
    job('offload / what changed', 'completed', 'success', 1),
    job('offload / test + typecheck (ui)', 'completed', 'failure', 4),
    job('offload / fmt + clippy + test (api)', 'in_progress', null, 5),
  ])
  eq([v.kind, v.early, v.onlyCancelled], ['red', true, false])
  eq(v.pending, ['fmt + clippy + test (api)'])
  eq(v.failing.map((j) => j.name), ['test + typecheck (ui)'])
})

check('verdictOf: every job green on a finished successful run is green', () => {
  const v = verdictOf(runRow(1, { status: 'completed', conclusion: 'success' }), JOBS_GREEN)
  eq(v.kind, 'green')
  eq(v.skipped, ['compile + link (mobile ios)'])
  eq(v.ran.length, 4)
})

check('verdictOf: a superseded run is cancelled, not red; a startup failure too', () => {
  const superseded = verdictOf(runRow(1, { status: 'completed', conclusion: 'cancelled' }), [
    job('offload / what changed', 'completed', 'success', 1),
    job('offload / fmt + clippy + test (api)', 'completed', 'cancelled', 5),
  ])
  eq(superseded.kind, 'cancelled')
  eq(verdictOf(runRow(1, { status: 'completed', conclusion: 'startup_failure' }), []).kind, 'cancelled')
  // Cancelled with a real failure in it is red: the failure is the news.
  const failedThenCancelled = verdictOf(runRow(1, { status: 'completed', conclusion: 'cancelled' }), [
    job('offload / test + typecheck (ui)', 'completed', 'failure', 4),
    job('offload / fmt + clippy + test (api)', 'completed', 'cancelled', 5),
  ])
  eq(failedThenCancelled.kind, 'red')
})

check('verdictOf: in progress with no failure waits; only-cancelled jobs on a live run are flagged for the ref check', () => {
  eq(verdictOf(runRow(1), [job('offload / what changed', 'completed', 'success', 1), job('offload / x', 'queued', null, 2)]).kind, 'wait')
  eq(verdictOf(runRow(1), []).kind, 'wait')
  const timedOut = verdictOf(runRow(1), [job('offload / e2e', 'completed', 'cancelled', 3), job('offload / x', 'in_progress', null, 2)])
  eq([timedOut.kind, timedOut.early, timedOut.onlyCancelled], ['red', true, true])
  // A finished run that failed with no failing job (a called workflow that
  // never started its jobs) is red, never green.
  eq(verdictOf(runRow(1, { status: 'completed', conclusion: 'failure' }), JOBS_GREEN).kind, 'red')
  // Success with a failed job (continue-on-error) is not green either.
  eq(verdictOf(runRow(1, { status: 'completed', conclusion: 'success' }), [job('offload / x', 'completed', 'failure', 1)]).kind, 'red')
})

check('proofLine: the one line an agent may cite', () => {
  const v = verdictOf(runRow(42, { status: 'completed', conclusion: 'success' }), JOBS_GREEN)
  eq(
    proofLine({ run: runRow(42, { status: 'completed', conclusion: 'success' }), ref: 'ci/wt/x', sha: SHA, mode: 'light', verdict: v }),
    'remote-gate: green — CI offload https://github.com/outcrop-labs/talaria/actions/runs/42 · ci/wt/x @ 5beeda7cb23c · light · ran: what changed, invariants (no deps), test + typecheck (ui), fmt + clippy + test (api) · skipped: compile + link (mobile ios)',
  )
  const rerun = proofLine({ run: runRow(42, { run_attempt: 2 }), ref: 'ci/full/wt/x', sha: SHA, mode: 'full', verdict: { ran: [], skipped: [] } })
  ok(rerun.includes(' · full · attempt 2 · ran: none · skipped: none'), rerun)
})

// ── the log ─────────────────────────────────────────────────────────────────

check('trimLog: no timestamps or colour, cut at the failing step, not the post-job cleanup', () => {
  const log = [
    '2026-11-11T03:12:47.4926601Z ##[group]Run bunx vitest run',
    '2026-11-11T03:12:47.4926698Z \u001b[31m FAIL \u001b[39m src/a.test.ts > adds',
    '2026-11-11T03:12:47.4926867Z AssertionError: expected 2 to be 3',
    '2026-11-11T03:12:47.4927044Z ##[error]Process completed with exit code 1.',
    '2026-11-11T03:12:47.5077280Z Post job cleanup.',
    '2026-11-11T03:12:47.8683037Z [command]/usr/bin/git version',
    '',
  ].join('\r\n')
  eq(trimLog(log).split('\n'), [
    '##[group]Run bunx vitest run',
    ' FAIL  src/a.test.ts > adds',
    'AssertionError: expected 2 to be 3',
    '##[error]Process completed with exit code 1.',
  ])
  // No failing step (a cancelled job): end before the cleanup.
  eq(trimLog('2026-11-11T03:12:47Z a\n2026-11-11T03:12:47Z b\n2026-11-11T03:12:48Z Post job cleanup.\n2026-11-11T03:12:48Z c\n'), 'a\nb')
  eq(trimLog(''), '')
  eq(trimLog(undefined), '')
})

check('trimLog: passing-test lines are dropped, so the failure is what the tail holds', () => {
  // The shape of the cli job's red on run 42953027808: bun prints every pass.
  const bun = [
    ...Array.from({ length: 120 }, (_, i) => `2026-11-11T03:29:01.0000000Z (pass) box new > case ${i} [0.20ms]`),
    '2026-11-11T03:29:01.0000000Z src/zz.test.ts:',
    '2026-11-11T03:29:01.0000000Z error: expect(received).toBe(expected)',
    '2026-11-11T03:29:01.0000000Z Expected: 3',
    '2026-11-11T03:29:01.0000000Z Received: 2',
    '2026-11-11T03:29:01.0000000Z (fail) remote-gate red probe [0.18ms]',
    ...Array.from({ length: 120 }, (_, i) => `2026-11-11T03:29:02.0000000Z (pass) app new > case ${i} [0.03ms]`),
    '2026-11-11T03:29:02.0000000Z  482 pass',
    '2026-11-11T03:29:02.0000000Z  1 fail',
    '2026-11-11T03:29:02.0000000Z ##[error]Process completed with exit code 1.',
  ].join('\n')
  const tail = trimLog(bun)
  ok(tail.includes('Expected: 3') && tail.includes('(fail) remote-gate red probe'), tail)
  ok(!tail.includes('(pass)'), tail)
  ok(tail.includes(' 482 pass'), 'the summary is not a passing-test line')
  eq(trimLog('   ✓ src/a.test.ts > adds 2ms\n   × src/a.test.ts > subtracts 1ms'), '   × src/a.test.ts > subtracts 1ms')
})

check('trimLog: at most maxLines lines and maxChars characters, whole lines first', () => {
  const many = Array.from({ length: 500 }, (_, i) => `line ${i}`).join('\n')
  const tail = trimLog(many, 80, 12000).split('\n')
  eq(tail.length, 80)
  eq(tail[0], 'line 420')
  eq(tail[79], 'line 499')
  const wide = Array.from({ length: 100 }, (_, i) => `${String(i).padStart(3, '0')}${'x'.repeat(297)}`).join('\n')
  const cut = trimLog(wide, 80, 1000)
  ok(cut.length <= 1000, `length ${cut.length}`)
  ok(cut.split('\n').every((l) => l.length === 300), 'whole lines only')
  ok(cut.endsWith(`099${'x'.repeat(297)}`), 'the tail is kept')
  const giant = 'y'.repeat(50_000)
  eq(trimLog(giant, 80, 12000).length, 12000)
})

// ── polling and the token ───────────────────────────────────────────────────

check('nextDelay: 20s, doubling, capped at 60, never past the time left, never under 1', () => {
  const seq = []
  let d = null
  for (let i = 0; i < 5; i++) seq.push((d = nextDelay(d)))
  eq(seq, [20, 40, 60, 60, 60])
  eq(nextDelay(40, 7.9), 7)
  eq(nextDelay(null, 0), 1)
  eq(nextDelay(60, -5), 1)
})

check('tokenFrom: GH_TOKEN, then GITHUB_TOKEN, then gh — and none is null', () => {
  let asked = 0
  const gh = () => {
    asked++
    return 'gho_fromgh\n'
  }
  eq(tokenFrom({ GH_TOKEN: 'a', GITHUB_TOKEN: 'b' }, gh), { token: 'a', source: 'GH_TOKEN' })
  eq(tokenFrom({ GH_TOKEN: '  ', GITHUB_TOKEN: 'b' }, gh), { token: 'b', source: 'GITHUB_TOKEN' })
  eq(asked, 0) // gh is not spawned when the environment has a token
  eq(tokenFrom({}, gh), { token: 'gho_fromgh', source: '`gh auth token`' })
  eq(tokenFrom({}, () => null), null)
  eq(tokenFrom({}, () => ''), null)
  eq(tokenFrom(undefined), null)
})

check('the tokens are the README\'s', () => {
  eq(TOKENS, ['dirty', 'base', 'has-pr', 'check', 'fmt', 'no-run', 'red', 'cancelled', 'timeout', 'pending'])
})

// ── the script itself, offline ──────────────────────────────────────────────

check('importing the gate pushes nothing; a direct run answers --help, and refuses an unknown flag', () => {
  const help = spawnSync(process.execPath, [join(HERE, 'remote-gate.mjs'), '--help'], { encoding: 'utf8' })
  eq(help.status, 0)
  ok(/remote-gate — compile and test on GitHub/.test(help.stdout), help.stdout)
  const bad = spawnSync(process.execPath, [join(HERE, 'remote-gate.mjs'), '--ful'], { encoding: 'utf8' })
  eq(bad.status, 1)
  ok(/^remote-gate: could not run — not a pass: unknown argument --ful/.test(bad.stderr), bad.stderr)
})

// A scratch repository holding just the gate and what it imports, with no
// remote at all: anything that reached the network would exit 1 instead.
function withScratchRepo(fn) {
  const dir = mkdtempSync(join(tmpdir(), 'talaria-remote-gate-'))
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith('GIT_')))
  const g = (...args) =>
    execFileSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@example.invalid', '-c', 'commit.gpgsign=false', ...args], {
      cwd: dir,
      env,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
  try {
    mkdirSync(join(dir, 'scripts', 'hooks'), { recursive: true })
    for (const rel of ['scripts/hooks/remote-gate.mjs', 'scripts/hooks/pr-watch.mjs', 'scripts/surfaces.mjs', 'scripts/judge-review-wire.mjs']) {
      copyFileSync(join(HERE, '..', '..', rel), join(dir, rel))
    }
    writeFileSync(join(dir, 'README'), 'one\n')
    g('init', '-q', '-b', 'main')
    g('add', '.')
    g('commit', '-q', '-m', 'scratch')
    const gate = (...args) => spawnSync(process.execPath, [join(dir, 'scripts/hooks/remote-gate.mjs'), ...args], { cwd: dir, env, encoding: 'utf8' })
    fn({ dir, g, gate })
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
}


// ── the model review of the same push ──────────────────────────────────────

const reviewRun = (id, o = {}) => ({ id, head_sha: SHA, event: 'push', path: '.github/workflows/judge-review.yml', status: 'completed', html_url: `https://github.com/o/r/actions/runs/${id}`, ...o })
check('review: the newest judge-review push run of this sha that the snapshot did not have', () => {
  const runs = [reviewRun(10), reviewRun(12), reviewRun(11), reviewRun(13, { head_sha: 'f'.repeat(40) }), reviewRun(14, { path: '.github/workflows/ci-offload.yml' }), reviewRun(15, { event: 'pull_request' })]
  eq(pickReviewRun(runs, SHA, [])?.id, 12)
  eq(pickReviewRun(runs, SHA, [12, 11])?.id, 10)
  eq(pickReviewRun(runs, SHA, [10, 11, 12]), null)
  eq(REVIEW_WAIT_MS, 12 * 60_000)
})

const ann = (gate, findings) => [
  { title: 'judge-review verdict', message: `status=reviewed head=${SHA.slice(0, 12)} base=na target=testing listed=${findings.length} more=2 gating=0 would=0 gate=${gate} cost=0.0123 model=m provider=p reason=none reused=none partial=no — x` },
  ...findings.map((id, i) => ({ title: `judge-review ${id} major correctness`, path: 'ui/src/a.ts', start_line: i + 1, end_line: i + 1, message: `a \u001b[31mtitle\u001b[0m ${i} — the failure text (the PR comment has the fix)` })),
]
check('review: the proof clause and the finding lines; no failure text reaches the terminal', () => {
  const r = reviewFromAnnotations(ann('report', ['jr-00000001']))
  eq(reviewClause(r), 'review: 1 to read (2 more) $0.0123')
  eq(reviewFindingLines(r), ['remote-gate: review — jr-00000001 major correctness ui/src/a.ts:1 — a title 0'])
  eq(reviewClause(reviewFromAnnotations([{ title: 'judge-review verdict', message: 'status=not-reviewed head=abcdef012345 reason=no-reviewer-on-trusted — x' }])), 'review: not reviewed (no-reviewer-on-trusted)')
  eq(reviewClause(null), 'review: none')
  eq(reviewClause({ status: 'unread', reason: 'HTTP 502' }), 'review: not read (HTTP 502)')
  const line = proofLine({ run: runRow(42, { status: 'completed', conclusion: 'success' }), ref: 'ci/wt/x', sha: SHA, mode: 'light', verdict: { ran: ['a'], skipped: [] }, review: r })
  ok(line.endsWith(' · review: 1 to read (2 more) $0.0123'), line)
})

check('review: the exit never depends on it, and the ref is deleted only after the wait', () => {
  const src = readFileSync(join(HERE, 'remote-gate.mjs'), 'utf8')
  const green = src.slice(src.indexOf("if (verdict.kind === 'green') {"))
  const wait = green.indexOf('await awaitReview(')
  const del = green.indexOf('--force-with-lease')
  ok(wait > 0 && del > wait, `awaitReview at ${wait}, the deletion at ${del}`)
  ok(!/block\([^)]*review/.test(src) && !/Exit\(2\)[^\n]*review/.test(src), 'a review result reaches an exit')
  ok(green.slice(0, green.indexOf('throw new Exit(0)')).includes('awaitReview'), 'the review is read on the green path')
})

check('a tracked edit is `dirty` at once, staged or not; an untracked file only warns', () => {
  withScratchRepo(({ dir, g, gate }) => {
    g('switch', '-q', '-c', 'wt/x')
    writeFileSync(join(dir, 'README'), 'two\n')
    let r = gate()
    eq(r.status, 2)
    eq(r.stderr.split('\n')[0], 'remote-gate: dirty')
    ok(r.stderr.includes(' M README'), r.stderr)
    eq(r.stdout, '')
    g('add', 'README')
    r = gate()
    eq([r.status, r.stderr.split('\n')[0]], [2, 'remote-gate: dirty'])
    g('commit', '-q', '-m', 'two')
    // Clean but for an untracked file: past the tree check, to the first
    // thing that needs GitHub, which this scratch repo does not have.
    writeFileSync(join(dir, 'notes.txt'), 'scratch\n')
    r = gate()
    eq(r.status, 1)
    ok(/^remote-gate: untracked — 1 untracked file\(s\), which CI cannot see: notes\.txt$/m.test(r.stderr), r.stderr)
    ok(/remote-gate: could not run — not a pass: origin is not a GitHub repository/.test(r.stderr), r.stderr)
  })
})

check('main, rc and testing are `base`: never offloaded', () => {
  withScratchRepo(({ g, gate }) => {
    for (const branch of ['main', 'rc', 'testing']) {
      if (branch !== 'main') g('switch', '-q', '-c', branch)
      const r = gate('--full')
      eq([branch, r.status, r.stderr.split('\n')[0]], [branch, 2, 'remote-gate: base'])
    }
  })
})

if (failed) {
  console.error(`remote-gate: ${failed} of ${ran} failed`)
  process.exit(1)
}
console.log(`remote-gate: ${ran} checks, all clean`)
