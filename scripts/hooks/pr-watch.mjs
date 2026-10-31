#!/usr/bin/env node
// pr-watch — the gate the stop gate cannot be.
//
// WHY. `scripts/hooks/stop-check.mjs` and the pre-push hook see the local tree
// at commit and push time. CI runs the full tree, and `rc` keeps moving under
// an open pull request: a sibling merge conflicts the branch, or a check that
// passed locally fails on the PR. An agent that reports done at `gh pr create`
// is reporting a state it has not read back. This script is that read.
//
// WHAT IT DOES NOT DO. It does not edit, merge, push, comment, or re-run a
// job. Diagnosis and the self-fix belong to the agent, and the procedure is
// the last step of `.claude/skills/ship-a-change/SKILL.md`. A self-fix push
// goes to the agent's own branch — flow-guard refuses `main` and `rc` — and
// this script never decides to notify a reviewer. Whether a harness should
// spawn it on its own, and whether a re-push should ping, are open human
// calls; the default is the skill runs it, and a push stays silent.
//
// THE CONTRACT (scripts/hooks/README.md). stdin is tolerated and never read.
//   exit 0 — checks green AND GitHub reports the PR mergeable (no conflict
//            with its base, which must be `rc`). One proof line on stdout,
//            the only state an agent may cite. Nothing on stderr.
//   exit 2 — block. The first stderr line is exactly one of:
//            `pr-watch: red`, `pr-watch: conflict`, `pr-watch: pending`,
//            `pr-watch: timeout`, `pr-watch: closed`, `pr-watch: base`.
//            The rest is the reason and the next command. Not done.
//   else   — could not run (no gh, no PR, auth, usage). Not a pass.
//
// GREEN IS NARROW ON PURPOSE. A skipped job is a pass: ci.yml skips a surface
// with a job-level `if`, and branch protection counts that as success
// (docs/BRANCHES.md). NEUTRAL is a pass so an informational check cannot
// wedge done. FAILURE, CANCELLED, TIMED_OUT, ACTION_REQUIRED, STARTUP_FAILURE,
// STALE and ERROR are not. An empty rollup is pending, never green — checks
// that have not started have not passed. `mergeable: UNKNOWN` is pending too;
// GitHub computes it asynchronously, and "not yet known" is not "mergeable".
// CONFLICTING or mergeStateStatus DIRTY is a conflict even when checks are
// also red: the red run is against a head the merge will replace. BEHIND is
// not a conflict (`strict` is off on `rc`); it is named in the proof line and
// does not block.
//
// ONE VERDICT PER CHECK, AND ONLY FOR THE COMMIT YOU PUSHED. The rollup GitHub
// returns lists every check run on the head commit, superseded ones included:
// judge.yml re-runs on a body edit with cancel-in-progress, so editing the body
// right after `gh pr create` leaves a CANCELLED `judge (standards)` beside the
// newer green one. Checks are grouped by workflow and name and the latest wins
// (job id, then start time); a CANCELLED run that is itself the latest is
// still red. Green also needs (1) the PR's head to be the pushed commit — this
// checkout's HEAD when it is on the PR's branch (and pushed: `origin/<branch>`
// is what `git push` sent), or a PR head that contains it; the branch's head on
// GitHub from anywhere else. This script never fetches, so a PR head pushed
// from elsewhere is often not in this clone: git cannot say whether it contains
// HEAD (`merge-base` exits 128), and GitHub's compare answers instead.
// For a moment after `git push`, `gh pr view` still returns the previous head
// and its finished rollup; a head mismatch is pending, ahead of red and
// conflict, since both describe a head the push replaced. A mismatch that
// waiting cannot clear — HEAD not pushed, or a PR head that dropped it — exits
// `pending` at once, as `--once` would, instead of polling to the wall clock.
// (2) A CI run (ci.yml's
// `name:`; it has no paths filter, so it always runs) for that head, with
// every workflow run on the head completed. Right after a push the rollup can
// hold only the fastest workflow; "nothing pending" is not "everything ran".
// A workflow run that failed with no failing check in the rollup (a startup
// failure has no jobs) is red.
//
// WHOSE ci.yml. The CI workflow's name is the target repository's — the one
// the PR's URL names (gh hands that back, so it is where the PR lives), else
// `--repo` — read from GitHub at the PR's head. (A pull_request run reads the
// merge commit's copy, which carries the head's name unless the base renamed
// the workflow after the branch point.) Not this checkout's: run from a clone of
// another repository (or a stale branch), the cwd's ci.yml names a workflow
// the PR never runs, or none at all. When GitHub cannot answer, this
// checkout's copy stands in only if one of its remotes is the target, and
// failing that the name is `CI`. Either fallback is said, in the pending
// reason and the proof line: a guessed name is not a read one. `gh run list`
// and the failed-job logs go to the same repository, and every `gh api` read
// (the ci.yml, the branch's head, the compare) names that repository's host
// with `--hostname`: `gh api` takes no `--repo`, and left to itself it asks
// GH_HOST or gh's one logged-in host, not the PR's. `gh run list` and
// `gh run view` get `--repo HOST/OWNER/REPO` for the same reason: gh reads a
// bare `OWNER/REPO` against GH_HOST too.
//
// BOUNDS. First poll is immediate. Then exponential backoff from 20s, capped
// at 2min, until a 45-minute wall clock. The skill quotes these numbers; they
// are the contract. A poll that is still pending at the wall clock exits 2
// `timeout` — loud, never a silent give-up. Minimum sleep between polls is
// 1s, so a bad flag cannot busy-loop.
//
// USAGE
//   node scripts/hooks/pr-watch.mjs [--pr <n|url>] [--once] [--json]
//       [--repo owner/name] [--interval 20] [--max-interval 120]
//       [--backoff 2] [--deadline 2700] [--log-lines 80]
//
//   No --pr: the open PR for the current branch. Run from the repo root.
//   --repo: the PR's repository (gh's `[HOST/]OWNER/REPO`), when it is not
//   this checkout's. A PR URL passed to --pr names its repository itself.

import { spawnSync } from 'node:child_process'
import { existsSync, readFileSync, realpathSync } from 'node:fs'
import { resolve } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'

const argv = process.argv.slice(2)

const FLAGS_WITH_VALUE = new Set([
  '--pr',
  '--repo',
  '--interval',
  '--max-interval',
  '--backoff',
  '--deadline',
  '--log-lines',
])

const arg = (name) => {
  const i = argv.indexOf(name)
  if (i === -1) return undefined
  const value = argv[i + 1]
  if (value === undefined || value.startsWith('--')) return undefined
  return value
}
const flag = (name) => argv.includes(name)

const positional = []
for (let i = 0; i < argv.length; i++) {
  if (FLAGS_WITH_VALUE.has(argv[i])) {
    i++
    continue
  }
  if (argv[i].startsWith('--')) continue
  positional.push(argv[i])
}

const PASS = new Set(['SUCCESS', 'SKIPPED', 'NEUTRAL'])
const FAIL = new Set([
  'FAILURE',
  'CANCELLED',
  'TIMED_OUT',
  'ACTION_REQUIRED',
  'STARTUP_FAILURE',
  'STALE',
  'ERROR',
])

const FIELDS = [
  'number',
  'url',
  'state',
  'isDraft',
  'mergeable',
  'mergeStateStatus',
  'baseRefName',
  'headRefName',
  'headRefOid',
  'headRepository',
  'headRepositoryOwner',
  'statusCheckRollup',
  'title',
].join(',')

const RUN_FIELDS = 'databaseId,workflowName,event,status,conclusion,headSha,createdAt,url'

function couldNotRun(why) {
  console.error(`pr-watch: could not run — not a pass: ${why}`)
  process.exit(1)
}

function printHelp() {
  console.log(`pr-watch — poll a pull request's checks and mergeable state.

exit 0   checks green (the latest run of each) on the pushed head, the CI workflow's
         run for that head finished, and the PR mergeable against rc. Proof line on stdout.
exit 2   red | conflict | pending (--once, or a head that waiting cannot fix) | timeout |
         closed | base. Reason on stderr.
exit 1   could not run. Not a pass.

  node scripts/hooks/pr-watch.mjs [--pr <n|url>] [--once] [--json] [--repo owner/name]
      [--interval 20] [--max-interval 120] [--backoff 2] [--deadline 2700]

No --pr: the open PR for the current branch. --repo: the PR's repository when it is not
this checkout's; the CI workflow's name is read from that repository's ci.yml at the PR
head. The procedure is .claude/skills/ship-a-change/SKILL.md. This script does not edit or push.`)
  process.exit(0)
}

function numberArg(name, fallback, min) {
  const raw = arg(name)
  if (raw === undefined) return fallback
  const n = Number(raw)
  if (!Number.isFinite(n) || n < min) couldNotRun(`${name} must be a number >= ${min}, got ${raw}`)
  return n
}

const once = flag('--once')
const json = flag('--json')
const prArg = arg('--pr') ?? positional[0]
const repo = arg('--repo')
const intervalStart = numberArg('--interval', 20, 1)
const maxInterval = numberArg('--max-interval', 120, 1)
const backoff = numberArg('--backoff', 2, 1)
const deadlineSec = numberArg('--deadline', 45 * 60, 1)
const logLines = numberArg('--log-lines', 80, 1)


function gh(args, timeoutMs) {
  return spawnSync('gh', args, {
    cwd: process.cwd(),
    encoding: 'utf8',
    timeout: timeoutMs,
    env: { ...process.env, GH_PROMPT_DISABLED: '1', GH_NO_UPDATE_NOTIFIER: '1' },
  })
}

function ghFailure(run) {
  if (run.error) return run.error.message
  if (run.signal) return `gh exited on ${run.signal}`
  return (run.stderr || run.stdout || `gh exited ${run.status}`).trim()
}

function currentBranch() {
  const run = spawnSync('git', ['rev-parse', '--abbrev-ref', 'HEAD'], {
    cwd: process.cwd(),
    encoding: 'utf8',
  })
  if (run.status !== 0) return null
  const name = run.stdout.trim()
  return name && name !== 'HEAD' ? name : null
}

const short = (sha) => String(sha || '').slice(0, 12)

/** A check's job id, from an Actions URL (`/actions/runs/<run>/job/<job>`).
 *  Job ids are allocated when the job is created, so a newer run's job — or a
 *  re-run's — has the larger one. null for anything that is not an Actions job. */
function jobIdOf(url) {
  const m = /\/actions\/runs\/\d+\/job\/(\d+)/.exec(String(url || ''))
  return m ? Number(m[1]) : null
}

/** A start time in ms, or Infinity when the check has not started: gh renders
 *  a queued run's missing start as the zero time, and a queued run of a name
 *  is newer than the finished one it is replacing. */
function startOf(iso) {
  const t = Date.parse(iso || '')
  return Number.isFinite(t) && t > 0 ? t : Infinity
}

export function normalize(raw, index = 0) {
  const name = raw.name || raw.context || '(unnamed check)'
  const status = String(raw.status || '').toUpperCase()
  const conclusion = String(raw.conclusion || '').toUpperCase()
  const state = String(raw.state || '').toUpperCase()
  const settled = conclusion || (['SUCCESS', 'FAILURE', 'ERROR', 'PENDING'].includes(state) ? state : '')
  let verdict = 'pending'
  if (PASS.has(settled)) verdict = 'pass'
  else if (FAIL.has(settled)) verdict = 'fail'
  else if (settled && settled !== 'PENDING' && status === 'COMPLETED') verdict = 'fail'
  const url = raw.detailsUrl || raw.targetUrl || ''
  const workflow = raw.workflowName || ''
  return {
    name,
    verdict,
    conclusion: settled || status || 'PENDING',
    url,
    workflow,
    key: `${workflow}\u0000${name}`,
    jobId: jobIdOf(url),
    started: startOf(raw.startedAt),
    index,
  }
}

function newer(a, b) {
  if (a.jobId !== null && b.jobId !== null && a.jobId !== b.jobId) return a.jobId > b.jobId
  if (a.started !== b.started) return a.started > b.started
  return a.index > b.index
}

/** One entry per workflow + check name: the latest. The rollup also carries
 *  the runs that a newer one superseded (a cancelled judge beside the re-run
 *  a body edit started); `gh pr checks` drops those, and so does this. */
export function latestChecks(checks) {
  const latest = new Map()
  for (const c of checks) {
    const seen = latest.get(c.key)
    if (!seen || newer(c, seen)) latest.set(c.key, c)
  }
  return [...latest.values()].sort((a, b) => a.index - b.index)
}

/** `gh run list --commit <head>` rows → the latest run per workflow and event,
 *  with a verdict. A re-run keeps its id and reports its newest attempt. */
export function latestRuns(rows, headSha) {
  const latest = new Map()
  for (const r of rows || []) {
    if (headSha && r.headSha && r.headSha !== headSha) continue
    const key = `${r.workflowName}\u0000${r.event}`
    const seen = latest.get(key)
    const id = Number(r.databaseId) || 0
    const seenId = seen ? Number(seen.databaseId) || 0 : -1
    if (!seen || id > seenId || (id === seenId && startOf(r.createdAt) > startOf(seen.createdAt))) latest.set(key, r)
  }
  return [...latest.values()].map((r) => {
    const status = String(r.status || '').toUpperCase()
    const conclusion = String(r.conclusion || '').toUpperCase()
    let verdict = 'pending'
    if (status === 'COMPLETED') verdict = PASS.has(conclusion) ? 'pass' : 'fail'
    return {
      workflow: r.workflowName || '(unnamed workflow)',
      event: r.event || '',
      verdict,
      conclusion: status === 'COMPLETED' ? conclusion || 'UNKNOWN' : status || 'PENDING',
      url: r.url || '',
    }
  })
}

/** Is the PR's head the commit that was pushed? `facts` is readHeadFacts():
 *  on the PR's branch, this checkout's HEAD, its `origin/<branch>` ref (which
 *  `git push` sets to the commit it sent), where the PR head stands against
 *  HEAD (`relation`, in GitHub compare's words from the PR head's side), and —
 *  when that does not settle it — the branch's head on GitHub; anywhere else,
 *  just the branch's head on GitHub. `ok: false` is pending: the PR has not
 *  caught up yet, or (`needsAction`) it cannot without someone acting. */
export function headCheck(pr, facts) {
  const want = pr.headRefOid
  const branch = pr.headRefName
  if (!facts) return { ok: false, why: 'the pushed head was not read' }
  if (facts.onBranch) {
    const { local, tracking, relation, remote } = facts
    if (local === want) return { ok: true }
    if (relation === 'ahead') {
      return { ok: true, note: `the PR head is ahead of this checkout (${short(local)})` }
    }
    if (tracking !== local) {
      const at = tracking ? `origin/${branch} is ${short(tracking)}` : `there is no origin/${branch}`
      return {
        ok: false,
        needsAction: true,
        why: `this checkout's HEAD ${short(local)} is not pushed (${at}) — push it: git push -u origin ${branch}`,
      }
    }
    // HEAD is pushed, as far as this clone knows, and the PR reads another commit.
    if (relation === 'behind' || remote === local) {
      return { ok: false, why: `GitHub has not registered the push of ${short(local)} yet — the PR still reads ${short(want)}` }
    }
    if (remote && remote !== want) {
      return { ok: false, why: `${branch} is at ${short(remote)} and the PR still reads ${short(want)} — GitHub has not registered the push yet` }
    }
    if (remote === want && relation === 'diverged') {
      return {
        ok: false,
        needsAction: true,
        why: `the PR head ${short(want)} does not contain this checkout's HEAD ${short(local)} — ${branch} moved on GitHub without it. Reconcile before claiming done: git fetch origin ${branch} && git log --oneline --left-right HEAD...origin/${branch}`,
      }
    }
    if (remote === want) {
      return {
        ok: false,
        why: `could not tell whether the PR head ${short(want)} contains this checkout's HEAD ${short(local)} (${facts.compareError || 'no answer'}) — git fetch origin ${branch} lets git answer it`,
      }
    }
    if (facts.noHeadRepo) {
      return { ok: false, needsAction: true, why: `the PR reads ${short(want)}, not this checkout's HEAD ${short(local)}, and its head repository is gone` }
    }
    return {
      ok: false,
      why: `the PR reads ${short(want)}, not this checkout's pushed HEAD ${short(local)}, and ${branch} on GitHub could not be read: ${facts.remoteError || 'no answer'}`,
    }
  }
  if (facts.noHeadRepo) return { ok: true, note: 'the head repository is gone, so the head was not compared' }
  if (!facts.remote) return { ok: false, why: `could not read the pushed head of ${branch}: ${facts.remoteError || 'no answer'}` }
  if (facts.remote === want) return { ok: true }
  return {
    ok: false,
    why: `${branch} is at ${short(facts.remote)} and the PR still reads ${short(want)} — GitHub has not registered the push yet`,
  }
}

/** The verdict. `ctx.head` is headCheck()'s answer, `ctx.runs` the raw
 *  `gh run list` rows for the head (or null with `ctx.runsError`), and
 *  `ctx.ciWorkflow` the CI workflow's name. Merged and closed PRs skip the
 *  head and run checks: there is no push left to wait for. */
export function classify(pr, ctx = {}) {
  const all = (pr.statusCheckRollup || []).map(normalize)
  const checks = latestChecks(all)
  const failing = checks.filter((c) => c.verdict === 'fail')
  const pending = checks.filter((c) => c.verdict === 'pending')
  const passing = checks.filter((c) => c.verdict === 'pass')
  const runs = Array.isArray(ctx.runs) ? latestRuns(ctx.runs, pr.headRefOid) : null
  // A run that failed with no failing check to show for it — a startup failure
  // has no jobs, so the rollup alone would read it as "nothing pending".
  for (const r of runs || []) {
    if (r.verdict !== 'fail' || failing.some((c) => c.workflow === r.workflow)) continue
    failing.push({ name: `${r.workflow} (workflow run)`, verdict: 'fail', conclusion: r.conclusion, url: r.url, workflow: r.workflow })
  }
  const mergeable = String(pr.mergeable || 'UNKNOWN').toUpperCase()
  const mergeState = String(pr.mergeStateStatus || 'UNKNOWN').toUpperCase()
  const conflict = mergeable === 'CONFLICTING' || mergeState === 'DIRTY'
  const base = {
    failing,
    pending,
    passing,
    mergeable,
    mergeState,
    checks: checks.length,
    superseded: all.length - checks.length,
    waiting: [],
    note: '',
  }

  if (pr.state === 'CLOSED') return { kind: 'closed', ...base }
  if (pr.baseRefName !== 'rc') return { kind: 'base', ...base }

  if (pr.state !== 'MERGED') {
    // Before red and conflict: both are about a head this push may have replaced.
    // So are its checks — a stale head's verdict lists are not reported at all.
    const head = ctx.head ?? { ok: false, why: 'the pushed head was not read' }
    if (!head.ok) {
      return {
        kind: 'pending',
        ...base,
        failing: [],
        pending: [],
        passing: [],
        checks: 0,
        superseded: 0,
        stale: true,
        needsAction: Boolean(head.needsAction),
        waiting: [head.why],
      }
    }
    base.note = head.note || ''
  }

  if (conflict) return { kind: 'conflict', ...base }
  if (failing.length) return { kind: 'red', ...base }
  if (pr.state === 'MERGED') return { kind: 'green', ...base, merged: true }

  const ci = ctx.ciWorkflow || 'CI'
  const waiting = []
  if (!runs) {
    waiting.push(`workflow runs for ${short(pr.headRefOid)} not read${ctx.runsError ? `: ${ctx.runsError}` : ''}`)
  } else {
    if (!runs.some((r) => r.workflow === ci)) {
      waiting.push(`no ${ci} workflow run for ${short(pr.headRefOid)} yet${ctx.ciNote ? ` (${ctx.ciNote})` : ''}`)
    }
    const unsettled = runs.filter((r) => r.verdict === 'pending')
    if (unsettled.length) waiting.push(`workflow run(s) not finished: ${unsettled.map((r) => r.workflow).join(', ')}`)
  }

  const mergeKnown = mergeable === 'MERGEABLE'
  if (!mergeKnown || pending.length || checks.length === 0 || waiting.length) {
    return { kind: 'pending', ...base, waiting }
  }
  return { kind: 'green', ...base }
}

/** A workflow file's top-level `name:` — what `gh run list` calls it — or
 *  null. Only column 0 counts: a job's `name:` is indented. */
export function workflowNameOf(text) {
  const m = /^name:[ \t]*['"]?([^'"#\n]+?)['"]?[ \t]*(?:#.*)?$/m.exec(String(text ?? ''))
  return m ? m[1].trim() : null
}

/** A repository as gh names one — `OWNER/REPO`, `HOST/OWNER/REPO`, an https
 *  or ssh URL (a PR's URL included), or a `git@host:owner/repo` remote — as
 *  `{ host, slug }`, or null. Lowercased: GitHub's names are not case-sensitive. */
export function parseRepo(text) {
  const s = String(text ?? '').trim()
  const m =
    /^[a-z][a-z0-9+.-]*:\/\/(?:[^@/]+@)?([^/:]+)(?::\d+)?\/([^/]+)\/([^/]+)/i.exec(s) ||
    /^[^@/\s]+@([^:/]+):([^/]+)\/([^/]+)$/.exec(s) ||
    /^(?:([^/\s]+)\/)?([^/\s]+)\/([^/\s]+)$/.exec(s)
  if (!m) return null
  const name = m[3].replace(/\.git$/i, '')
  if (!m[2] || !name) return null
  return { host: (m[1] || 'github.com').toLowerCase(), slug: `${m[2]}/${name}`.toLowerCase() }
}

/** The repository the PR lives in: the one its URL names, else `--repo`.
 *  The URL is gh's answer for the PR it found, so it wins over a flag that
 *  disagrees. That is where its workflows run, and whose ci.yml names them. */
export function targetRepoOf(pr, repoFlag) {
  return parseRepo(pr?.url) || parseRepo(repoFlag)
}

/** A parsed repository as the verdict names it: `OWNER/REPO` on github.com,
 *  `HOST/OWNER/REPO` elsewhere. Words only — gh gets repoArgOf(). */
export function repoFlagOf(target) {
  if (!target) return null
  return target.host === 'github.com' ? target.slug : `${target.host}/${target.slug}`
}

/** The target as `--repo` for `gh run list` and `gh run view`: always
 *  `HOST/OWNER/REPO`. gh resolves a bare `OWNER/REPO` against GH_HOST (or its
 *  one logged-in host), so a github.com PR's runs were read from another host
 *  whenever GH_HOST named one. */
export function repoArgOf(target) {
  return target ? `${target.host}/${target.slug}` : null
}

/** A `gh api` call to the target's host. `gh api` does not take `--repo`:
 *  left to itself it asks GH_HOST, or the one host gh is logged in to, else
 *  github.com — not the PR's. A GitHub Enterprise PR's branch-head and
 *  compare reads went to github.com, and a GH_HOST naming another host moved a
 *  github.com PR's. So the host is always named, github.com included. With no
 *  target there is none to name, and gh's own default stands. */
export function ghApiArgs(target, path, ...rest) {
  return ['api', path, ...(target?.host ? ['--hostname', target.host] : []), ...rest]
}

/** The CI workflow's name for this PR: the target repository's ci.yml at the
 *  PR's head, from GitHub. When GitHub cannot answer, this checkout's ci.yml
 *  if one of its remotes is the target; else `CI`. `source` says which, and
 *  `note` (empty on a read) why a fallback stands in. `io` is injected for
 *  the test: `gh` and `git` runners, and `localCi`, the cwd's ci.yml or null. */
export function resolveCiWorkflow(pr, target, io) {
  const head = String(pr.headRefOid || '')
  const where = target ? `${repoFlagOf(target)}'s ci.yml at ${short(head)}` : 'the target repository\'s ci.yml'
  let why
  if (!target) why = `no repository in --repo or the PR URL (${pr.url || 'none'})`
  else {
    const path = `repos/${target.slug}/contents/.github/workflows/ci.yml?ref=${encodeURIComponent(head)}`
    const run = io.gh(ghApiArgs(target, path, '-H', 'Accept: application/vnd.github.raw'), 60_000)
    if (run.status === 0) {
      const name = workflowNameOf(run.stdout)
      if (name) return { name, source: `${repoFlagOf(target)}@${short(head)}`, note: '' }
      why = 'it has no top-level name:'
    } else why = ghFailure(run).split('\n')[0]
  }
  const remotes = io.git(['remote', '-v']).out.split('\n').map((l) => parseRepo(l.split(/\s+/)[1]))
  const here = target && remotes.some((r) => r && r.host === target.host && r.slug === target.slug)
  const local = here ? workflowNameOf(io.localCi()) : null
  if (local) {
    return { name: local, source: 'this checkout', note: `${local} is this checkout's ci.yml; ${where} was not read: ${why}` }
  }
  const unrelated = target && !here ? `; this checkout is not ${repoFlagOf(target)}, so its ci.yml was not used` : ''
  return { name: 'CI', source: 'fallback', note: `CI is a guess: ${where} was not read (${why})${unrelated}` }
}

function localCiFile() {
  const file = resolve(process.cwd(), '.github/workflows/ci.yml')
  return existsSync(file) ? readFileSync(file, 'utf8') : null
}

function git(args) {
  const run = spawnSync('git', args, { cwd: process.cwd(), encoding: 'utf8' })
  return { ok: run.status === 0, out: (run.stdout || '').trim(), status: run.status }
}

function headRepoPath(pr) {
  const owner = pr.headRepositoryOwner?.login
  const name = pr.headRepository?.name
  return owner && name ? `${owner}/${name}` : null
}

/** The branch's head on GitHub, from the git ref API. A head repository
 *  (a fork included) lives on the PR's host, so `target`'s host is asked. */
function readRemoteHead(pr, target, io) {
  const path = headRepoPath(pr)
  if (!path) return { noHeadRepo: true }
  const ref = String(pr.headRefName).split('/').map(encodeURIComponent).join('/')
  const run = io.gh(ghApiArgs(target, `repos/${path}/git/ref/heads/${ref}`, '--jq', '.object.sha'), 60_000)
  if (run.status !== 0) return { remote: null, remoteError: ghFailure(run).split('\n')[0] }
  return { remote: String(run.stdout).trim() }
}

const RELATIONS = new Set(['ahead', 'behind', 'diverged'])

/** Where the PR head `want` stands against `local`, in the words of GitHub's
 *  compare from the PR head's side: 'ahead' (it contains local), 'behind'
 *  (local contains it), 'diverged', or null with `compareError`. git answers
 *  when both commits are in this clone. This script never fetches, so a head
 *  pushed from another checkout often is not, and `merge-base` exits 128 —
 *  which is not a "no". GitHub's compare answers that case. */
function relationOf(pr, local, want, target, io) {
  const ancestor = (a, b) => io.git(['merge-base', '--is-ancestor', a, b]).status
  const up = ancestor(local, want)
  if (up === 0) return { relation: 'ahead' }
  if (up === 1) {
    const down = ancestor(want, local)
    if (down === 0) return { relation: 'behind' }
    if (down === 1) return { relation: 'diverged' }
  }
  const path = headRepoPath(pr)
  if (!path) return { relation: null, compareError: 'the head repository is gone' }
  const run = io.gh(ghApiArgs(target, `repos/${path}/compare/${local}...${want}?per_page=1`, '--jq', '.status'), 60_000)
  if (run.status !== 0) return { relation: null, compareError: `compare: ${ghFailure(run).split('\n')[0]}` }
  const status = String(run.stdout).trim()
  if (status === 'identical') return { relation: 'ahead' }
  if (RELATIONS.has(status)) return { relation: status }
  return { relation: null, compareError: `compare answered ${status || 'nothing'}` }
}

/** What headCheck() needs, read from this checkout and GitHub. `target` is
 *  targetRepoOf(): its host is the one GitHub's answers come from. `io` is the
 *  git and gh runners and the current branch, injected so the test can drive
 *  the real git reads against a scratch repository. */
export function readHeadFacts(pr, target, io = { branch: currentBranch(), git, gh }) {
  const want = String(pr.headRefOid)
  if (io.branch !== pr.headRefName) return { onBranch: false, ...readRemoteHead(pr, target, io) }
  const local = io.git(['rev-parse', 'HEAD']).out
  const tracking = io.git(['rev-parse', '--verify', '-q', `refs/remotes/origin/${pr.headRefName}`])
  const facts = { onBranch: true, local, tracking: tracking.ok ? tracking.out : null }
  if (local === want) return facts
  Object.assign(facts, relationOf(pr, local, want, target, io))
  // Settled without GitHub's branch: the PR contains HEAD, HEAD is not pushed,
  // or the PR is an older commit of HEAD's own history (the push not registered).
  if (facts.relation === 'ahead' || facts.relation === 'behind' || facts.tracking !== local) return facts
  return { ...facts, ...readRemoteHead(pr, target, io) }
}

function readRuns(sha, repoFlag) {
  const args = ['run', 'list', '--commit', String(sha), '--limit', '100', '--json', RUN_FIELDS]
  if (repoFlag) args.push('--repo', repoFlag)
  const run = gh(args, 60_000)
  if (run.status !== 0) return { runs: null, runsError: ghFailure(run).split('\n')[0] }
  try {
    return { runs: JSON.parse(run.stdout) }
  } catch (e) {
    return { runs: null, runsError: `gh run list did not return JSON (${e.message})` }
  }
}

function readPr() {
  const args = ['pr', 'view']
  if (prArg) args.push(String(prArg))
  args.push('--json', FIELDS)
  if (repo) args.push('--repo', repo)
  const run = gh(args, 60_000)
  if (run.status !== 0) return { error: ghFailure(run) }
  try {
    return { pr: JSON.parse(run.stdout) }
  } catch (e) {
    return { error: `gh pr view did not return JSON (${e.message})` }
  }
}

function checkLine(c) {
  const wf = c.workflow ? ` [${c.workflow}]` : ''
  return `  ${c.name}${wf}  ${c.conclusion}  ${c.url || '(no url)'}`
}

function idsFromUrl(url) {
  const m = String(url).match(/\/actions\/runs\/(\d+)(?:\/job\/(\d+))?/)
  if (!m) return null
  return { runId: m[1], jobId: m[2] || null }
}

function logTail(url, repoFlag) {
  const ids = idsFromUrl(url)
  if (!ids) return `(no Actions run id in ${url})`
  const args = ['run', 'view', ids.runId, '--log-failed']
  if (ids.jobId) args.push('--job', ids.jobId)
  if (repoFlag) args.push('--repo', repoFlag)
  const run = gh(args, 120_000)
  if (run.status !== 0) {
    const why = ghFailure(run).split('\n')[0]
    return `(log fetch failed: ${why})\n${url}`
  }
  const lines = (run.stdout || '').split('\n')
  const tail = lines.slice(-logLines).join('\n')
  if (tail.length > 12_000) return tail.slice(-12_000)
  return tail || '(log fetch returned empty)'
}

function payloadOf(pr, result, ci) {
  return {
    token: result.kind,
    number: pr.number,
    url: pr.url,
    state: pr.state,
    isDraft: pr.isDraft,
    baseRefName: pr.baseRefName,
    headRefName: pr.headRefName,
    headRefOid: pr.headRefOid,
    mergeable: result.mergeable,
    mergeStateStatus: result.mergeState,
    failing: result.failing.map((c) => ({ name: c.name, conclusion: c.conclusion, url: c.url })),
    pending: result.pending.map((c) => ({ name: c.name, conclusion: c.conclusion, url: c.url })),
    waiting: result.waiting,
    needsAction: Boolean(result.needsAction),
    passing: result.passing.length,
    skipped: result.passing.filter((c) => c.conclusion === 'SKIPPED').length,
    superseded: result.superseded,
    ciWorkflow: ci ? { name: ci.name, source: ci.source, note: ci.note } : null,
  }
}

function emit(code, line, body, payload) {
  const token = line.startsWith('pr-watch: ') ? line.slice('pr-watch: '.length) : line
  if (json) console.log(JSON.stringify({ token, ...payload }, null, 2))
  if (code === 0) {
    if (!json) console.log(`${line}\n${body.trim()}\n`)
    process.exit(0)
  }
  console.error(`${line}\n${body.trim()}\n`)
  process.exit(code)
}

function branchNote(pr) {
  const here = currentBranch()
  if (!here || here === pr.headRefName) return ''
  return (
    `This checkout is on \`${here}\`; #${pr.number} is \`${pr.headRefName}\`. ` +
    'Reading only — a self-fix push belongs on the PR branch, not here.\n'
  )
}

function redBody(pr, result) {
  const repoFlag = repoArgOf(targetRepoOf(pr, repo))
  const logs = result.failing.slice(0, 3).map((c) => {
    const tail = logTail(c.url, repoFlag)
    return `--- log: ${c.name} (last ${logLines} lines) ---\n${tail}`
  })
  const more =
    result.failing.length > 3
      ? `\n${result.failing.length - 3} more failing check(s) not logged; the URLs are above.`
      : ''
  return `${branchNote(pr)}#${pr.number} is not done — failing checks. Do not claim done.
${pr.url}
head ${pr.headRefName} @ ${String(pr.headRefOid).slice(0, 12)}

${result.failing.map(checkLine).join('\n')}

Diagnose from the log, fix, re-run \`bun run gate\` (it compiles only what this diff touches; if the log names a crate the diff did not, \`cargo test -p <that crate>\` — never a workspace cargo, never \`api:check\`), and push to \`${pr.headRefName}\` only. Never \`main\`, never \`rc\`, never \`--force\`. Then run this watcher again.

The same check still red after a push that was supposed to fix it is not self-fixable. A product decision, a review request, or infra you cannot fix from this branch (runner, registry, a secret) is not self-fixable either. Report the job link. Do not claim done.
${more}
${logs.join('\n\n')}`
}

function conflictBody(pr, result) {
  const also = result.failing.length
    ? `\nAlso failing (the merge will replace this head — fix the conflict first):\n${result.failing.map(checkLine).join('\n')}\n`
    : ''
  return `${branchNote(pr)}#${pr.number} conflicts with \`${pr.baseRefName}\` (mergeable=${result.mergeable}, mergeStateStatus=${result.mergeState}). Do not claim done.
${pr.url}
head ${pr.headRefName} @ ${String(pr.headRefOid).slice(0, 12)}
${also}
Merge \`rc\` in, resolve, re-run the gates, push to \`${pr.headRefName}\` only (never \`main\`, never \`rc\`, never \`--force\`), then run this watcher again:

  git fetch origin rc && git merge origin/rc

A resolution you cannot make honestly — product behavior, or a generated file you do not know how to regenerate — is not self-fixable. Report this state instead of pushing a guess.`
}

function pendingSummary(result) {
  const parts = [...(result.waiting || [])]
  if (result.stale) return parts.join('; ')
  if (result.pending.length) parts.push(result.pending.map((c) => c.name).join(', '))
  else if (result.checks === 0) parts.push('no checks reported yet')
  if (result.mergeable !== 'MERGEABLE') parts.push(`mergeable=${result.mergeable}`)
  return parts.join('; ') || 'not settled'
}

async function main() {
  const started = Date.now()
  // One read of the CI workflow's name per PR head (a push can rename it). A
  // fallback is not kept: a read that failed once is tried again next poll.
  const ciByHead = new Map()
  const deadlineMs = deadlineSec * 1000
  let interval = intervalStart
  let sawRead = false
  let readFailures = 0

  for (;;) {
    const read = readPr()
    if (read.error) {
      readFailures++
      const why = read.error.split('\n')[0]
      if (once || (!sawRead && readFailures >= 3)) couldNotRun(why)
      const elapsed = Date.now() - started
      if (elapsed >= deadlineMs) couldNotRun(`${why} (still failing at the ${deadlineSec}s wall clock)`)
      const wait = Math.min(interval, Math.ceil((deadlineMs - elapsed) / 1000))
      console.error(`pr-watch: read failed — not a pass yet: ${why} — retrying in ${wait}s`)
      await sleep(wait * 1000)
      interval = Math.min(interval * backoff, maxInterval)
      continue
    }

    sawRead = true
    readFailures = 0
    const pr = read.pr
    const open = pr.state !== 'MERGED' && pr.state !== 'CLOSED'
    const target = targetRepoOf(pr, repo)
    const ctx = {}
    let ci = null
    if (open) {
      ctx.head = headCheck(pr, readHeadFacts(pr, target))
      if (ctx.head.ok) {
        ci = ciByHead.get(pr.headRefOid) ?? resolveCiWorkflow(pr, target, { gh, git, localCi: localCiFile })
        if (!ci.note) ciByHead.set(pr.headRefOid, ci)
        Object.assign(ctx, { ciWorkflow: ci.name, ciNote: ci.note }, readRuns(pr.headRefOid, repoArgOf(target)))
      }
    }
    const result = classify(pr, ctx)
    const payload = payloadOf(pr, result, ci)

    if (result.kind === 'green') {
      const skipped = result.passing.filter((c) => c.conclusion === 'SKIPPED').length
      const draft = pr.isDraft ? ' draft' : ''
      const behind = result.mergeState === 'BEHIND' ? ' behind rc (strict is off; not a conflict).' : ''
      const merged = result.merged ? ' already merged.' : ''
      const superseded = result.superseded ? `, ${result.superseded} superseded run(s) ignored` : ''
      const ran = result.merged ? '' : `, ${ci?.name || 'CI'} run settled${ci?.note ? ` (${ci.note})` : ''}`
      const note = result.note ? ` (${result.note})` : ''
      emit(
        0,
        'pr-watch: green',
        `#${pr.number}${draft} ${pr.url} — ${result.passing.length} checks passed (${skipped} skipped${superseded})${ran}, mergeable=${result.mergeable} at ${short(pr.headRefOid)}${note}.${behind}${merged}`,
        payload,
      )
    }

    if (result.kind === 'conflict') emit(2, 'pr-watch: conflict', conflictBody(pr, result), payload)
    if (result.kind === 'red') emit(2, 'pr-watch: red', redBody(pr, result), payload)
    if (result.kind === 'closed') {
      emit(
        2,
        'pr-watch: closed',
        `${branchNote(pr)}#${pr.number} is closed, not merged. Not done.\n${pr.url}`,
        payload,
      )
    }
    if (result.kind === 'base') {
      emit(
        2,
        'pr-watch: base',
        `${branchNote(pr)}#${pr.number} targets \`${pr.baseRefName}\`, not \`rc\`. Retarget before claiming done.\n${pr.url}`,
        payload,
      )
    }

    const elapsed = Date.now() - started
    const left = Math.max(0, Math.ceil((deadlineMs - elapsed) / 1000))
    // Waiting cannot clear a HEAD nobody pushed, so the answer comes now.
    if (once || result.needsAction) {
      const next = result.needsAction
        ? 'Waiting will not change this: do what it says, then run this watcher again.'
        : 'Re-run without --once to wait, or run this again.'
      emit(
        2,
        'pr-watch: pending',
        `${branchNote(pr)}#${pr.number} is not done — ${pendingSummary(result)}.\n${pr.url}\n${result.pending.map(checkLine).join('\n')}\n${next} Do not claim done.`,
        payload,
      )
    }
    if (left <= 0) {
      emit(
        2,
        'pr-watch: timeout',
        `${branchNote(pr)}#${pr.number} is not done — ${pendingSummary(result)}.\n${pr.url}\n${result.pending.map(checkLine).join('\n')}\nStill pending after ${deadlineSec}s. Not done, and not a pass. Report the PR URL. Do not claim done.`,
        payload,
      )
    }

    const wait = Math.max(1, Math.min(interval, left))
    console.error(
      `pr-watch: pending — ${pendingSummary(result)} — next poll in ${wait}s, ${left}s left\n${pr.url}`,
    )
    await sleep(wait * 1000)
    interval = Math.min(interval * backoff, maxInterval)
  }
}

function invokedDirectly() {
  const file = process.argv[1]
  if (!file) return false
  try {
    return realpathSync(file) === realpathSync(fileURLToPath(import.meta.url))
  } catch {
    return pathToFileURL(resolve(file)).href === import.meta.url
  }
}

// The verdict above is imported by pr-watch.test.mjs; only a direct run polls.
if (invokedDirectly()) {
  if (flag('--help') || flag('-h')) printHelp()
  if (positional.length > 1) couldNotRun(`unexpected arguments: ${positional.slice(1).join(' ')}`)
  if (maxInterval < intervalStart) couldNotRun('--max-interval must be >= --interval')
  main().catch((err) => couldNotRun(err.message))
}
