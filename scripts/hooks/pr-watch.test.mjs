// The post-PR watcher's verdict: when a pull request is green, and when it is
// not yet. A wrong answer either way is a wrong "done" — red on a superseded
// cancelled run sends an agent to fix a check that passed, and green on a
// partial rollup (or on the previous head's) lets it claim a state CI has not
// reached. The fixtures are recorded `gh pr view --json` and `gh run list
// --json` output (pr-watch.fixtures/README.md); the in-flight variants are
// derived from them. Run by `bun run check`.
import { spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { classify, headCheck, latestChecks, latestRuns, normalize } from './pr-watch.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const fixture = (name) => JSON.parse(readFileSync(join(HERE, 'pr-watch.fixtures', name), 'utf8'))
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

/** A recorded PR as it stood while open: GitHub reports merged PRs with
 *  `mergeable: UNKNOWN`, so the open moment is MERGEABLE + CLEAN. */
function asOpen(pr, over = {}) {
  return { ...structuredClone(pr), state: 'OPEN', mergeable: 'MERGEABLE', mergeStateStatus: 'CLEAN', ...over }
}

const HEAD_OK = { ok: true }
const names = (list) => list.map((c) => c.name).sort()

// #599: judge was re-run by a body edit seconds after `gh pr create`.
const PR599 = fixture('pr-599.json')
const RUNS599 = fixture('runs-599.json')
// #575: CI's api, ui and e2e jobs failed on the head.
const PR575 = fixture('pr-575.json')
const RUNS575 = fixture('runs-575.json')

check('the #599 fixture really holds a superseded CANCELLED judge beside a newer SUCCESS one', () => {
  const judges = PR599.statusCheckRollup.filter((c) => c.name === 'judge (standards)').map((c) => c.conclusion)
  eq(judges.sort(), ['CANCELLED', 'SUCCESS'])
})

// ── (a) superseded runs ────────────────────────────────────────────────────

check('a superseded CANCELLED judge beside the newer green one is green, not red', () => {
  const r = classify(asOpen(PR599), { head: HEAD_OK, runs: RUNS599, ciWorkflow: 'CI' })
  eq(r.kind, 'green')
  eq(r.superseded, 1)
  eq(r.failing, [])
  eq(r.checks, 13)
})

check('the rollup order does not matter: the latest is chosen by job id, not position', () => {
  const pr = asOpen(PR599)
  pr.statusCheckRollup.reverse()
  eq(classify(pr, { head: HEAD_OK, runs: RUNS599, ciWorkflow: 'CI' }).kind, 'green')
})

check('a CANCELLED run that is itself the latest stays red', () => {
  const pr = asOpen(PR599)
  pr.statusCheckRollup = pr.statusCheckRollup.filter((c) => !(c.name === 'judge (standards)' && c.conclusion === 'SUCCESS'))
  const runs = RUNS599.filter((r) => r.conclusion !== 'success' || r.workflowName !== 'judge')
  const r = classify(pr, { head: HEAD_OK, runs, ciWorkflow: 'CI' })
  eq(r.kind, 'red')
  eq(names(r.failing), ['judge (standards)'])
})

check('a queued re-run beside the cancelled one it replaces is pending, not red', () => {
  const pr = asOpen(PR599)
  const success = pr.statusCheckRollup.findIndex((c) => c.name === 'judge (standards)' && c.conclusion === 'SUCCESS')
  // gh renders a run that has not started with the zero time and no conclusion.
  pr.statusCheckRollup[success] = {
    ...pr.statusCheckRollup[success],
    status: 'QUEUED',
    conclusion: '',
    startedAt: '0001-01-01T00:00:00Z',
    completedAt: '0001-01-01T00:00:00Z',
  }
  const r = classify(pr, { head: HEAD_OK, runs: RUNS599, ciWorkflow: 'CI' })
  eq(r.kind, 'pending')
  eq(names(r.pending), ['judge (standards)'])
  // And with no job id to compare, the unstarted one is still the newer.
  pr.statusCheckRollup[success].detailsUrl = ''
  eq(classify(pr, { head: HEAD_OK, runs: RUNS599, ciWorkflow: 'CI' }).kind, 'pending')
})

check('status contexts (no workflow, no job id) dedupe by context and start time', () => {
  const ctx = [
    { __typename: 'StatusContext', context: 'deploy/preview', state: 'ERROR', startedAt: '2026-10-26T18:00:00Z', targetUrl: 'x' },
    { __typename: 'StatusContext', context: 'deploy/preview', state: 'SUCCESS', startedAt: '2026-10-26T18:05:00Z', targetUrl: 'y' },
  ].map(normalize)
  const latest = latestChecks(ctx)
  eq(latest.map((c) => c.conclusion), ['SUCCESS'])
  eq(latestChecks([...ctx].reverse()).map((c) => c.conclusion), ['SUCCESS'])
})

check('the recorded #575 failures stay red: three CI jobs, by name', () => {
  const r = classify(asOpen(PR575), { head: HEAD_OK, runs: RUNS575, ciWorkflow: 'CI' })
  eq(r.kind, 'red')
  eq(names(r.failing), ['fmt + clippy + test (api)', 'mcp end-to-end (playwright)', 'test + typecheck (ui)'])
  eq(r.superseded, 0)
})

// ── (b) a partial rollup, and the head ─────────────────────────────────────

check('judge green with no CI run for the head is pending, not green', () => {
  const pr = asOpen(PR599)
  pr.statusCheckRollup = pr.statusCheckRollup.filter((c) => c.workflowName === 'judge')
  const runs = RUNS599.filter((r) => r.workflowName === 'judge')
  const r = classify(pr, { head: HEAD_OK, runs, ciWorkflow: 'CI' })
  eq(r.kind, 'pending')
  eq(r.pending, [])
  ok(r.waiting.some((w) => /no CI workflow run/.test(w)), `waiting: ${r.waiting}`)
})

check('CI started but still running — its first job done, the rest not yet created — is pending', () => {
  const pr = asOpen(PR599)
  pr.statusCheckRollup = pr.statusCheckRollup.filter((c) => c.workflowName !== 'CI' || c.name === 'what changed')
  const runs = RUNS599.map((r) => (r.workflowName === 'CI' ? { ...r, status: 'in_progress', conclusion: '' } : r))
  const r = classify(pr, { head: HEAD_OK, runs, ciWorkflow: 'CI' })
  eq(r.kind, 'pending')
  ok(r.waiting.some((w) => /not finished: CI/.test(w)), `waiting: ${r.waiting}`)
})

check('workflow runs that could not be read are pending, never a pass', () => {
  const r = classify(asOpen(PR599), { head: HEAD_OK, runs: null, runsError: 'HTTP 502', ciWorkflow: 'CI' })
  eq(r.kind, 'pending')
  ok(r.waiting.some((w) => /not read: HTTP 502/.test(w)), `waiting: ${r.waiting}`)
})

check('a workflow run that failed with no job in the rollup (startup failure) is red', () => {
  const pr = asOpen(PR599)
  pr.statusCheckRollup = pr.statusCheckRollup.filter((c) => c.workflowName !== 'CI')
  const runs = RUNS599.map((r) => (r.workflowName === 'CI' ? { ...r, conclusion: 'startup_failure' } : r))
  const r = classify(pr, { head: HEAD_OK, runs, ciWorkflow: 'CI' })
  eq(r.kind, 'red')
  eq(r.failing.map((c) => [c.name, c.conclusion]), [['CI (workflow run)', 'STARTUP_FAILURE']])
})

check('a failed run whose jobs are in the rollup is named once, by its jobs', () => {
  const r = classify(asOpen(PR575), { head: HEAD_OK, runs: RUNS575, ciWorkflow: 'CI' })
  ok(!r.failing.some((c) => c.name === 'CI (workflow run)'), 'the CI run was named beside its failing jobs')
})

check('a PR head that is not this checkout\'s pushed HEAD is pending — even over a stale red rollup', () => {
  const pr = asOpen(PR575)
  const local = 'f'.repeat(40)
  const head = headCheck(pr, { onBranch: true, local, tracking: local, prContainsLocal: false })
  eq(head.ok, false)
  ok(/has not registered the push of ffffffffffff/.test(head.why), head.why)
  const r = classify(pr, { head, runs: RUNS575, ciWorkflow: 'CI' })
  eq(r.kind, 'pending')
  eq(r.waiting, [head.why])
  // and over a stale conflict
  eq(classify({ ...pr, mergeable: 'CONFLICTING' }, { head, runs: RUNS575 }).kind, 'pending')
})

check('a checkout with commits the PR does not have says to push', () => {
  const pr = asOpen(PR599)
  const head = headCheck(pr, { onBranch: true, local: 'a'.repeat(40), tracking: pr.headRefOid, prContainsLocal: false })
  eq(head.ok, false)
  ok(/is not pushed .* git push -u origin wt\/w4-triggers/.test(head.why), head.why)
  const never = headCheck(pr, { onBranch: true, local: 'a'.repeat(40), tracking: null, prContainsLocal: false })
  ok(/there is no origin\/wt\/w4-triggers/.test(never.why), never.why)
})

check('the head this checkout pushed, or a PR head ahead of it, passes the head check', () => {
  const pr = asOpen(PR599)
  eq(headCheck(pr, { onBranch: true, local: pr.headRefOid, tracking: pr.headRefOid }), { ok: true })
  const ahead = headCheck(pr, { onBranch: true, local: 'b'.repeat(40), tracking: 'b'.repeat(40), prContainsLocal: true })
  eq(ahead.ok, true)
  const r = classify(pr, { head: ahead, runs: RUNS599, ciWorkflow: 'CI' })
  eq(r.kind, 'green')
  ok(/ahead of this checkout/.test(r.note), r.note)
})

check('from another checkout, the head is compared with the branch on GitHub', () => {
  const pr = asOpen(PR599)
  eq(headCheck(pr, { onBranch: false, remote: pr.headRefOid }), { ok: true })
  const lag = headCheck(pr, { onBranch: false, remote: 'c'.repeat(40) })
  eq(lag.ok, false)
  ok(/is at cccccccccccc and the PR still reads b74073f04448/.test(lag.why), lag.why)
  const unread = headCheck(pr, { onBranch: false, remote: null, remoteError: 'HTTP 404' })
  eq(unread.ok, false)
  eq(headCheck(pr, { onBranch: false, noHeadRepo: true }).ok, true)
})

check('no head facts is pending, never green', () => {
  eq(classify(asOpen(PR599), { runs: RUNS599, ciWorkflow: 'CI' }).kind, 'pending')
  eq(headCheck(asOpen(PR599), undefined).ok, false)
})

// ── the rest of the contract still holds ──────────────────────────────────

check('mergeable UNKNOWN is pending; CONFLICTING is a conflict once the head is confirmed', () => {
  eq(classify(asOpen(PR599, { mergeable: 'UNKNOWN' }), { head: HEAD_OK, runs: RUNS599 }).kind, 'pending')
  eq(classify(asOpen(PR599, { mergeable: 'CONFLICTING' }), { head: HEAD_OK, runs: RUNS599 }).kind, 'conflict')
  eq(classify(asOpen(PR575, { mergeStateStatus: 'DIRTY' }), { head: HEAD_OK, runs: RUNS575 }).kind, 'conflict')
})

check('an empty rollup is pending even with every run settled', () => {
  eq(classify(asOpen(PR599, { statusCheckRollup: [] }), { head: HEAD_OK, runs: RUNS599 }).kind, 'pending')
})

check('merged needs no head or run read; closed and a non-rc base keep their tokens', () => {
  const merged = classify(PR599, {})
  eq([merged.kind, merged.merged], ['green', true])
  eq(classify({ ...PR599, state: 'CLOSED' }, {}).kind, 'closed')
  eq(classify(asOpen(PR599, { baseRefName: 'main' }), { head: HEAD_OK, runs: RUNS599 }).kind, 'base')
})

check('latestRuns: one per workflow and event, the newest id, only for the head', () => {
  const latest = latestRuns(RUNS599, PR599.headRefOid)
  eq(latest.map((r) => `${r.workflow}:${r.conclusion}`).sort(), ['CI:SUCCESS', 'api-integration:SUCCESS', 'judge:SUCCESS', 'migrations:SUCCESS'])
  eq(latestRuns([...RUNS599].reverse(), PR599.headRefOid).find((r) => r.workflow === 'judge').conclusion, 'SUCCESS')
  eq(latestRuns(RUNS599, 'not-this-head'), [])
})

check('importing the watcher does not poll; a direct run still answers --help', () => {
  const run = spawnSync(process.execPath, [join(HERE, 'pr-watch.mjs'), '--help'], { encoding: 'utf8' })
  eq(run.status, 0)
  ok(/pr-watch — poll a pull request/.test(run.stdout), run.stdout)
})

if (failed) {
  console.error(`pr-watch: ${failed} of ${ran} failed`)
  process.exit(1)
}
console.log(`pr-watch: ${ran} checks, all clean`)
