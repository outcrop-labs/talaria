// The post-PR watcher's verdict: when a pull request is green, and when it is
// not yet. A wrong answer either way is a wrong "done" — red on a superseded
// cancelled run sends an agent to fix a check that passed, and green on a
// partial rollup (or on the previous head's) lets it claim a state CI has not
// reached. The fixtures are recorded `gh pr view --json` and `gh run list
// --json` output (pr-watch.fixtures/README.md); the in-flight variants are
// derived from them. Run by `bun run check`.
import { spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  classify,
  headCheck,
  latestChecks,
  latestRuns,
  normalize,
  parseRepo,
  readHeadFacts,
  repoFlagOf,
  resolveCiWorkflow,
  targetRepoOf,
  workflowNameOf,
} from './pr-watch.mjs'

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

// #607 (this change's own PR), recorded 4, 17 and 21 seconds after `gh pr create`.
const at = (t) => [fixture(`pr-607-t${t}.json`), fixture(`runs-607-t${t}.json`)]

check('recorded at +4s: both runs queued and no check in the rollup yet — pending', () => {
  const [pr, runs] = at(4)
  eq(pr.statusCheckRollup.length, 0)
  const r = classify(pr, { head: HEAD_OK, runs, ciWorkflow: 'CI' })
  eq(r.kind, 'pending')
  ok(r.waiting.some((w) => /not finished: judge, CI/.test(w)), `waiting: ${r.waiting}`)
})

check('recorded at +17s: CI\'s skipped jobs registered, judge and invariants running — pending', () => {
  const [pr, runs] = at(17)
  const r = classify(pr, { head: HEAD_OK, runs, ciWorkflow: 'CI' })
  eq(r.kind, 'pending')
  eq(names(r.pending), ['invariants (no deps)', 'judge (standards)'])
  ok(r.waiting.some((w) => /not finished: CI/.test(w)), `waiting: ${r.waiting}`)
})

check('the +17s rollup with its last jobs done but the CI run not yet marked finished is still pending', () => {
  // The rollup alone now has nothing pending, which is all the old rule asked.
  const [pr, runs] = at(17)
  for (const c of pr.statusCheckRollup) if (c.status === 'IN_PROGRESS') Object.assign(c, { status: 'COMPLETED', conclusion: 'SUCCESS' })
  const r = classify(pr, { head: HEAD_OK, runs, ciWorkflow: 'CI' })
  eq(r.pending, [])
  eq(r.kind, 'pending')
  eq(classify(pr, { head: HEAD_OK, runs: at(21)[1], ciWorkflow: 'CI' }).kind, 'green')
})

check('recorded at +21s: every run finished on the pushed head — green, exactly as recorded', () => {
  const [pr, runs] = at(21)
  const head = headCheck(pr, { onBranch: true, local: pr.headRefOid, tracking: pr.headRefOid })
  const r = classify(pr, { head, runs, ciWorkflow: 'CI' })
  eq([r.kind, r.passing.length, r.superseded], ['green', 11, 0])
  // the same recording, read by a checkout that has already pushed the next commit
  const next = headCheck(pr, { onBranch: true, local: 'd'.repeat(40), tracking: 'd'.repeat(40), relation: 'behind' })
  eq(classify(pr, { head: next, runs, ciWorkflow: 'CI' }).kind, 'pending')
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
  const head = headCheck(pr, { onBranch: true, local, tracking: local, relation: 'behind' })
  eq([head.ok, Boolean(head.needsAction)], [false, false])
  ok(/has not registered the push of ffffffffffff/.test(head.why), head.why)
  const r = classify(pr, { head, runs: RUNS575, ciWorkflow: 'CI' })
  eq(r.kind, 'pending')
  eq(r.waiting, [head.why])
  // The stale head's checks are not this push's: none is reported, red or pending.
  eq([r.failing.length, r.pending.length, r.passing.length, r.stale], [0, 0, 0, true])
  // and over a stale conflict
  eq(classify({ ...pr, mergeable: 'CONFLICTING' }, { head, runs: RUNS575 }).kind, 'pending')
})

check('a checkout with commits the PR does not have says to push', () => {
  const pr = asOpen(PR599)
  const head = headCheck(pr, { onBranch: true, local: 'a'.repeat(40), tracking: pr.headRefOid, relation: 'behind' })
  eq([head.ok, head.needsAction], [false, true])
  ok(/is not pushed .* git push -u origin wt\/w4-triggers/.test(head.why), head.why)
  const never = headCheck(pr, { onBranch: true, local: 'a'.repeat(40), tracking: null, relation: null })
  ok(/there is no origin\/wt\/w4-triggers/.test(never.why), never.why)
  // Waiting cannot push it, so the watcher answers at once instead of polling.
  eq(classify(pr, { head, runs: RUNS599, ciWorkflow: 'CI' }).needsAction, true)
})

check('the head this checkout pushed, or a PR head ahead of it, passes the head check', () => {
  const pr = asOpen(PR599)
  eq(headCheck(pr, { onBranch: true, local: pr.headRefOid, tracking: pr.headRefOid }), { ok: true })
  const ahead = headCheck(pr, { onBranch: true, local: 'b'.repeat(40), tracking: 'b'.repeat(40), relation: 'ahead' })
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

// ── whose ci.yml names the CI workflow ────────────────────────────────────
//
// With --repo (or a PR URL from another repository) the watcher used to read
// the CI workflow's name from the cwd's ci.yml: a workflow the PR never runs,
// so the floor waited on it to the wall clock — or, with no ci.yml here, on a
// guessed `CI` it did not say was a guess.

check('workflowNameOf reads the top-level name only', () => {
  eq(workflowNameOf('# CI\nname: CI\non: [push]\n'), 'CI')
  eq(workflowNameOf("name: 'Talaria CI'   # the tripwire\n"), 'Talaria CI')
  eq(workflowNameOf('on: push\njobs:\n  a:\n    name: not this\n'), null)
  eq(workflowNameOf(null), null)
  eq(workflowNameOf(readFileSync(join(HERE, '..', '..', '.github/workflows/ci.yml'), 'utf8')), 'CI')
})

check('parseRepo reads every spelling gh and git use for a repository', () => {
  const gh = { host: 'github.com', slug: 'outcrop-labs/talaria' }
  eq(parseRepo('outcrop-labs/talaria'), gh)
  eq(parseRepo('Outcrop-Labs/Talaria'), gh)
  eq(parseRepo('github.com/outcrop-labs/talaria'), gh)
  eq(parseRepo('https://github.com/outcrop-labs/talaria/pull/615'), gh)
  eq(parseRepo('https://github.com/outcrop-labs/talaria.git'), gh)
  eq(parseRepo('git@github.com:outcrop-labs/talaria.git'), gh)
  eq(parseRepo('ssh://git@github.com/outcrop-labs/talaria.git'), gh)
  eq(parseRepo('ghe.example.com/team/app'), { host: 'ghe.example.com', slug: 'team/app' })
  eq(parseRepo('https://ghe.example.com:8443/team/app/pull/3'), { host: 'ghe.example.com', slug: 'team/app' })
  eq([parseRepo(''), parseRepo(undefined), parseRepo('talaria'), parseRepo('/x')], [null, null, null, null])
  eq(repoFlagOf(gh), 'outcrop-labs/talaria')
  eq(repoFlagOf({ host: 'ghe.example.com', slug: 'team/app' }), 'ghe.example.com/team/app')
})

check('the target repository is the one the PR URL names, else --repo', () => {
  const pr = { url: 'https://github.com/other-org/other-repo/pull/9' }
  eq(targetRepoOf(pr, undefined), { host: 'github.com', slug: 'other-org/other-repo' })
  // gh's URL is the PR it found; a --repo that disagrees does not move the reads off it.
  eq(targetRepoOf(pr, 'outcrop-labs/talaria'), { host: 'github.com', slug: 'other-org/other-repo' })
  eq(targetRepoOf({}, 'outcrop-labs/talaria'), { host: 'github.com', slug: 'outcrop-labs/talaria' })
  eq(targetRepoOf({ url: 'not a url' }, 'ghe.example.com/team/app'), { host: 'ghe.example.com', slug: 'team/app' })
  eq(targetRepoOf({}, undefined), null)
})

/** A fake io for resolveCiWorkflow: `contents` is gh's answer for the
 *  contents read (a string is a 0 exit with that body), `remotes` the
 *  checkout's `git remote -v`, `local` its ci.yml. Calls are recorded. */
function ciIo({ contents, remotes = '', local = 'name: Local CI\n' } = {}) {
  const calls = { gh: [], local: 0 }
  return {
    calls,
    gh: (args) => {
      calls.gh.push(args)
      if (typeof contents === 'string') return { status: 0, stdout: contents, stderr: '' }
      return contents || { status: 1, stdout: '', stderr: 'gh: Not Found (HTTP 404)\n' }
    },
    git: () => ({ ok: true, out: remotes, status: 0 }),
    localCi: () => {
      calls.local++
      return local
    },
  }
}
const OTHER = { host: 'github.com', slug: 'other-org/other-repo' }
const PR_OTHER = { url: 'https://github.com/other-org/other-repo/pull/9', headRefOid: 'a'.repeat(40) }

check('the CI workflow is named by the target repository\'s ci.yml at the PR head, not the cwd\'s', () => {
  const x = ciIo({ contents: '# theirs\nname: Pipeline\non: pull_request\n' })
  const ci = resolveCiWorkflow(PR_OTHER, OTHER, x)
  eq(ci, { name: 'Pipeline', source: 'other-org/other-repo@aaaaaaaaaaaa', note: '' })
  eq(x.calls.gh, [[
    'api',
    `repos/other-org/other-repo/contents/.github/workflows/ci.yml?ref=${'a'.repeat(40)}`,
    '-H',
    'Accept: application/vnd.github.raw',
  ]])
  eq(x.calls.local, 0)
})

check('a GitHub Enterprise target reads through its own host', () => {
  const x = ciIo({ contents: 'name: Build\n' })
  eq(resolveCiWorkflow(PR_OTHER, { host: 'ghe.example.com', slug: 'team/app' }, x).name, 'Build')
  ok(x.calls.gh[0].join(' ').includes('--hostname ghe.example.com'), x.calls.gh[0].join(' '))
})

check('unreadable, and this checkout is not the target: CI, said to be a guess, the cwd\'s ci.yml unused', () => {
  const x = ciIo({ remotes: 'origin\tgit@github.com:outcrop-labs/talaria.git (fetch)\norigin\tgit@github.com:outcrop-labs/talaria.git (push)' })
  const ci = resolveCiWorkflow(PR_OTHER, OTHER, x)
  eq([ci.name, ci.source, x.calls.local], ['CI', 'fallback', 0])
  ok(/^CI is a guess: other-org\/other-repo's ci.yml at aaaaaaaaaaaa was not read \(gh: Not Found \(HTTP 404\)\); this checkout is not other-org\/other-repo/.test(ci.note), ci.note)
})

check('unreadable, and a remote of this checkout is the target: its ci.yml stands in, and says so', () => {
  const x = ciIo({ remotes: 'upstream\thttps://github.com/Other-Org/other-repo.git (fetch)' })
  const ci = resolveCiWorkflow(PR_OTHER, OTHER, x)
  eq([ci.name, ci.source], ['Local CI', 'this checkout'])
  ok(/^Local CI is this checkout's ci.yml; other-org\/other-repo's ci.yml at aaaaaaaaaaaa was not read: gh: Not Found/.test(ci.note), ci.note)
  // The checkout is the target but has no ci.yml either: still a said guess.
  const none = resolveCiWorkflow(PR_OTHER, OTHER, ciIo({ remotes: 'origin\tgit@github.com:other-org/other-repo (fetch)', local: null }))
  eq(none.name, 'CI')
  ok(/^CI is a guess: .* was not read \(gh: Not Found/.test(none.note) && !/is not other-org/.test(none.note), none.note)
})

check('a ci.yml with no top-level name falls back the same way, and says why', () => {
  const ci = resolveCiWorkflow(PR_OTHER, OTHER, ciIo({ contents: 'on: push\njobs: {}\n' }))
  eq(ci.name, 'CI')
  ok(/\(it has no top-level name:\)/.test(ci.note), ci.note)
})

check('a guessed CI name is named in the pending reason', () => {
  const r = classify(asOpen(PR599), { head: HEAD_OK, runs: RUNS599, ciWorkflow: 'Pipeline', ciNote: 'Pipeline is a test' })
  eq(r.kind, 'pending')
  ok(r.waiting.some((w) => /^no Pipeline workflow run for b74073f04448 yet \(Pipeline is a test\)$/.test(w)), r.waiting.join(' | '))
})

// ── the head facts, read by real git from a scratch repository ────────────
//
// The watcher never fetches. A PR head pushed from another checkout is then
// not in this clone, `git merge-base --is-ancestor` exits 128 (not 1), and the
// question goes to GitHub's compare — faked here; git is not.

const BRANCH = 'wt/x'
const scratch = mkdtempSync(join(tmpdir(), 'pr-watch-git-'))
const cleanEnv = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith('GIT_')))
function sgit(args) {
  const run = spawnSync(
    'git',
    ['-c', 'user.name=pr-watch', '-c', 'user.email=pr-watch@example.invalid', '-c', 'core.hooksPath=/dev/null', ...args],
    { cwd: scratch, encoding: 'utf8', env: cleanEnv },
  )
  return { ok: run.status === 0, out: (run.stdout || '').trim(), status: run.status }
}
sgit(['init', '-q'])
sgit(['symbolic-ref', 'HEAD', `refs/heads/${BRANCH}`])
sgit(['commit', '-q', '--allow-empty', '--no-gpg-sign', '-m', 'a'])
const A = sgit(['rev-parse', 'HEAD']).out
sgit(['commit', '-q', '--allow-empty', '--no-gpg-sign', '-m', 'b'])
const B = sgit(['rev-parse', 'HEAD']).out
const C = sgit(['commit-tree', '--no-gpg-sign', `${A}^{tree}`, '-p', A, '-m', 'c']).out // beside B
const U = 'e'.repeat(40) // pushed from elsewhere; never in this clone
sgit(['update-ref', `refs/remotes/origin/${BRANCH}`, B]) // B is pushed

/** A fake `gh` that answers the compare and git-ref reads, and records them. */
function io({ compare, ref } = {}) {
  const calls = []
  const answer = (a) => (a === undefined ? { status: 1, stderr: 'not asked for' } : typeof a === 'string' ? { status: 0, stdout: `${a}\n` } : a)
  return {
    calls,
    branch: BRANCH,
    git: sgit,
    gh: (args) => {
      calls.push(args[1])
      return answer(/\/compare\//.test(args[1]) ? compare : ref)
    },
  }
}
const prAt = (sha) => ({ headRefName: BRANCH, headRefOid: sha, headRepository: { name: 'talaria' }, headRepositoryOwner: { login: 'outcrop-labs' } })

check('the scratch repository is what the cases below assume', () => {
  ok([A, B, C].every((s) => /^[0-9a-f]{40}$/.test(s)) && new Set([A, B, C]).size === 3, `commits: ${A} ${B} ${C}`)
  eq(sgit(['merge-base', '--is-ancestor', B, U]).status, 128)
  eq(sgit(['merge-base', '--is-ancestor', B, C]).status, 1)
})

check('a PR head ahead of this checkout but not in this clone passes: GitHub compare says it contains HEAD', () => {
  const x = io({ compare: 'ahead' })
  const facts = readHeadFacts(prAt(U), x)
  eq([facts.local, facts.tracking, facts.relation], [B, B, 'ahead'])
  eq(x.calls, [`repos/outcrop-labs/talaria/compare/${B}...${U}?per_page=1`])
  const head = headCheck(prAt(U), facts)
  eq(head.ok, true)
  ok(/ahead of this checkout/.test(head.note), head.note)
  // The reader before this one took exit 128 for "no" and said the push had not registered.
  ok(!/has not registered/.test(head.why || ''), head.why)
})

check('a PR head that dropped this checkout\'s pushed HEAD cannot be waited out', () => {
  const x = io({ compare: 'diverged', ref: U })
  const head = headCheck(prAt(U), readHeadFacts(prAt(U), x))
  eq([head.ok, head.needsAction], [false, true])
  ok(/does not contain this checkout's HEAD .* git fetch origin wt\/x/.test(head.why), head.why)
  eq(x.calls.length, 2)
})

check('a compare that does not answer is pending with a fetch hint, and still waited on', () => {
  const x = io({ compare: { status: 1, stderr: 'HTTP 502: Bad Gateway' }, ref: U })
  const head = headCheck(prAt(U), readHeadFacts(prAt(U), x))
  eq([head.ok, Boolean(head.needsAction)], [false, false])
  ok(/could not tell .*HTTP 502.* git fetch origin wt\/x/.test(head.why), head.why)
})

check('a PR head in this clone is settled by git, and GitHub is asked only what git cannot say', () => {
  // A is B's parent: the PR has not caught up with the push. No gh call at all.
  const older = io()
  const lag = headCheck(prAt(A), readHeadFacts(prAt(A), older))
  eq([lag.ok, Boolean(lag.needsAction), older.calls], [false, false, []])
  ok(/has not registered the push of .* the PR still reads/.test(lag.why), lag.why)
  // C sits beside B. Where the branch is on GitHub decides which story is true.
  const moved = headCheck(prAt(C), readHeadFacts(prAt(C), io({ ref: C })))
  eq([moved.ok, moved.needsAction], [false, true])
  const pushed = headCheck(prAt(C), readHeadFacts(prAt(C), io({ ref: B })))
  eq([pushed.ok, Boolean(pushed.needsAction)], [false, false])
  ok(/has not registered the push/.test(pushed.why), pushed.why)
  const later = headCheck(prAt(C), readHeadFacts(prAt(C), io({ ref: U })))
  ok(/wt\/x is at eeeeeeeeeeee and the PR still reads/.test(later.why), later.why)
})

check('an unpushed HEAD exits pending at once, without --once, instead of polling to the wall clock', () => {
  sgit(['update-ref', `refs/remotes/origin/${BRANCH}`, A]) // B was never pushed
  try {
    const head = headCheck(prAt(A), readHeadFacts(prAt(A), io()))
    eq([head.ok, head.needsAction], [false, true])
    // End to end: a fake gh on PATH serves the PR; the real watcher runs in the scratch repo.
    const bin = mkdtempSync(join(tmpdir(), 'pr-watch-bin-'))
    try {
      const pr = { ...asOpen(PR599), ...prAt(A), number: 1, url: 'https://example.invalid/pull/1', baseRefName: 'rc' }
      writeFileSync(join(bin, 'pr.json'), JSON.stringify(pr))
      writeFileSync(join(bin, 'gh'), `#!/bin/sh\n[ "$1" = pr ] && exec cat "${join(bin, 'pr.json')}"\necho "unexpected: gh $*" >&2\nexit 1\n`, { mode: 0o755 })
      const t0 = Date.now()
      const run = spawnSync(process.execPath, [join(HERE, 'pr-watch.mjs'), '--pr', '1'], {
        cwd: scratch,
        encoding: 'utf8',
        timeout: 15_000,
        env: { ...cleanEnv, PATH: `${bin}:${process.env.PATH}` },
      })
      eq(run.status, 2)
      eq(run.stderr.split('\n')[0], 'pr-watch: pending')
      ok(/is not pushed .* git push -u origin wt\/x/.test(run.stderr), run.stderr)
      ok(/Waiting will not change this/.test(run.stderr), run.stderr)
      ok(Date.now() - t0 < 10_000, `took ${Date.now() - t0} ms`)
    } finally {
      rmSync(bin, { recursive: true, force: true })
    }
  } finally {
    sgit(['update-ref', `refs/remotes/origin/${BRANCH}`, B])
  }
})

check('end to end: --repo, or a PR URL, sends the CI name read and gh run list to the target repository', () => {
  // The scratch checkout has its own ci.yml, naming a workflow the target never runs.
  mkdirSync(join(scratch, '.github', 'workflows'), { recursive: true })
  writeFileSync(join(scratch, '.github', 'workflows', 'ci.yml'), 'name: Local CI\non: push\n')
  const bin = mkdtempSync(join(tmpdir(), 'pr-watch-bin-'))
  try {
    const pr = { ...asOpen(PR599), ...prAt(B), number: 1, url: 'https://github.com/other-org/other-repo/pull/1', baseRefName: 'rc' }
    const runs = RUNS599.map((r) => ({ ...r, headSha: B, workflowName: r.workflowName === 'CI' ? 'Pipeline' : r.workflowName }))
    writeFileSync(join(bin, 'pr.json'), JSON.stringify(pr))
    writeFileSync(join(bin, 'runs.json'), JSON.stringify(runs))
    const log = join(bin, 'calls.log')
    writeFileSync(
      join(bin, 'gh'),
      [
        '#!/bin/sh',
        `echo "$*" >> "${log}"`,
        'case "$1" in',
        `  pr) exec cat "${join(bin, 'pr.json')}" ;;`,
        `  run) exec cat "${join(bin, 'runs.json')}" ;;`,
        `  api) case "$2" in repos/other-org/other-repo/contents/*) [ -f "${join(bin, 'ci.yml')}" ] && exec cat "${join(bin, 'ci.yml')}" ;; esac`,
        "       echo 'gh: Not Found (HTTP 404)' >&2; exit 1 ;;",
        'esac',
        'echo "unexpected: gh $*" >&2',
        'exit 1',
        '',
      ].join('\n'),
      { mode: 0o755 },
    )
    const watch = (...args) => {
      writeFileSync(log, '')
      const run = spawnSync(process.execPath, [join(HERE, 'pr-watch.mjs'), '--once', ...args], {
        cwd: scratch,
        encoding: 'utf8',
        timeout: 15_000,
        env: { ...cleanEnv, PATH: `${bin}:${process.env.PATH}` },
      })
      return { ...run, calls: readFileSync(log, 'utf8').trim().split('\n') }
    }

    // The target's ci.yml names Pipeline, and Pipeline ran: green, from --repo or the PR URL alike.
    // A --repo that disagrees with the URL is the third case: gh answers with the URL's PR (real gh
    // does), so the URL it hands back is where the reads go, and wrong-org's 404 is never asked.
    writeFileSync(join(bin, 'ci.yml'), 'name: Pipeline\non: pull_request\n')
    const url = 'https://github.com/other-org/other-repo/pull/1'
    for (const args of [['--repo', 'other-org/other-repo', '--pr', '1'], ['--pr', url], ['--repo', 'wrong-org/wrong-repo', '--pr', url]]) {
      const run = watch(...args, '--json')
      eq([run.status, run.stderr], [0, ''])
      const out = JSON.parse(run.stdout)
      eq([out.token, out.ciWorkflow], ['green', { name: 'Pipeline', source: `other-org/other-repo@${B.slice(0, 12)}`, note: '' }])
      ok(run.calls.includes(`run list --commit ${B} --limit 100 --json databaseId,workflowName,event,status,conclusion,headSha,createdAt,url --repo other-org/other-repo`), run.calls.join('\n'))
    }
    const line = watch('--repo', 'other-org/other-repo', '--pr', '1')
    eq(line.status, 0)
    ok(/, Pipeline run settled, mergeable=MERGEABLE/.test(line.stdout), line.stdout)

    // GitHub cannot answer and this clone is not the target: a said guess, never "Local CI".
    rmSync(join(bin, 'ci.yml'))
    const guess = watch('--repo', 'other-org/other-repo', '--pr', '1')
    eq([guess.status, guess.stderr.split('\n')[0]], [2, 'pr-watch: pending'])
    ok(/no CI workflow run for \S+ yet \(CI is a guess: other-org\/other-repo's ci.yml at \S+ was not read \(gh: Not Found \(HTTP 404\)\); this checkout is not other-org\/other-repo/.test(guess.stderr), guess.stderr)
    ok(!/Local CI/.test(guess.stderr), guess.stderr)

    // ...and when this clone is the target, its own ci.yml stands in, named as such.
    sgit(['remote', 'add', 'origin', 'https://github.com/Other-Org/other-repo.git'])
    try {
      const local = watch('--pr', '1')
      eq(local.status, 2)
      ok(/no Local CI workflow run for \S+ yet \(Local CI is this checkout's ci.yml; other-org\/other-repo's ci.yml at \S+ was not read: gh: Not Found/.test(local.stderr), local.stderr)
    } finally {
      sgit(['remote', 'remove', 'origin'])
      sgit(['update-ref', `refs/remotes/origin/${BRANCH}`, B])
    }
  } finally {
    rmSync(bin, { recursive: true, force: true })
    rmSync(join(scratch, '.github'), { recursive: true, force: true })
  }
})

rmSync(scratch, { recursive: true, force: true })

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
