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

import { spawnSync } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'

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
  'statusCheckRollup',
  'title',
].join(',')

function couldNotRun(why) {
  console.error(`pr-watch: could not run — not a pass: ${why}`)
  process.exit(1)
}

if (flag('--help') || flag('-h')) {
  console.log(`pr-watch — poll a pull request's checks and mergeable state.

exit 0   checks green and the PR is mergeable against rc. Proof line on stdout.
exit 2   red | conflict | pending (--once) | timeout | closed | base. Reason on stderr.
exit 1   could not run. Not a pass.

  node scripts/hooks/pr-watch.mjs [--pr <n|url>] [--once] [--json]
      [--interval 20] [--max-interval 120] [--backoff 2] [--deadline 2700]

No --pr: the open PR for the current branch. The procedure is
.claude/skills/ship-a-change/SKILL.md. This script does not edit or push.`)
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

if (positional.length > 1) couldNotRun(`unexpected arguments: ${positional.slice(1).join(' ')}`)
if (maxInterval < intervalStart) couldNotRun('--max-interval must be >= --interval')

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

function normalize(raw) {
  const name = raw.name || raw.context || '(unnamed check)'
  const status = String(raw.status || '').toUpperCase()
  const conclusion = String(raw.conclusion || '').toUpperCase()
  const state = String(raw.state || '').toUpperCase()
  const settled = conclusion || (['SUCCESS', 'FAILURE', 'ERROR', 'PENDING'].includes(state) ? state : '')
  let verdict = 'pending'
  if (PASS.has(settled)) verdict = 'pass'
  else if (FAIL.has(settled)) verdict = 'fail'
  else if (settled && settled !== 'PENDING' && status === 'COMPLETED') verdict = 'fail'
  return {
    name,
    verdict,
    conclusion: settled || status || 'PENDING',
    url: raw.detailsUrl || raw.targetUrl || '',
    workflow: raw.workflowName || '',
  }
}

function classify(pr) {
  const checks = (pr.statusCheckRollup || []).map(normalize)
  const failing = checks.filter((c) => c.verdict === 'fail')
  const pending = checks.filter((c) => c.verdict === 'pending')
  const passing = checks.filter((c) => c.verdict === 'pass')
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
  }

  if (pr.state === 'CLOSED') return { kind: 'closed', ...base }
  if (pr.baseRefName !== 'rc') return { kind: 'base', ...base }
  if (conflict) return { kind: 'conflict', ...base }
  if (failing.length) return { kind: 'red', ...base }
  if (pr.state === 'MERGED') return { kind: 'green', ...base, merged: true }

  const mergeKnown = mergeable === 'MERGEABLE'
  if (!mergeKnown || pending.length || checks.length === 0) return { kind: 'pending', ...base }
  return { kind: 'green', ...base }
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

function logTail(url) {
  const ids = idsFromUrl(url)
  if (!ids) return `(no Actions run id in ${url})`
  const args = ['run', 'view', ids.runId, '--log-failed']
  if (ids.jobId) args.push('--job', ids.jobId)
  if (repo) args.push('--repo', repo)
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

function payloadOf(pr, result) {
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
    passing: result.passing.length,
    skipped: result.passing.filter((c) => c.conclusion === 'SKIPPED').length,
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
  const logs = result.failing.slice(0, 3).map((c) => {
    const tail = logTail(c.url)
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

Diagnose from the log, fix, re-run the local gates (\`bun run verify\`, plus \`bun run api:check\` / \`bun run desktop:check\` if those surfaces moved), and push to \`${pr.headRefName}\` only. Never \`main\`, never \`rc\`, never \`--force\`. Then run this watcher again.

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
  if (result.pending.length) return result.pending.map((c) => c.name).join(', ')
  if (result.checks === 0) return 'no checks reported yet'
  if (result.mergeable !== 'MERGEABLE') return `mergeable=${result.mergeable}`
  return 'not settled'
}

async function main() {
  const started = Date.now()
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
    const result = classify(pr)
    const payload = payloadOf(pr, result)

    if (result.kind === 'green') {
      const skipped = result.passing.filter((c) => c.conclusion === 'SKIPPED').length
      const draft = pr.isDraft ? ' draft' : ''
      const behind = result.mergeState === 'BEHIND' ? ' behind rc (strict is off; not a conflict).' : ''
      const merged = result.merged ? ' already merged.' : ''
      emit(
        0,
        'pr-watch: green',
        `#${pr.number}${draft} ${pr.url} — ${result.passing.length} checks passed (${skipped} skipped), mergeable=${result.mergeable} at ${String(pr.headRefOid).slice(0, 12)}.${behind}${merged}`,
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
    if (once) {
      emit(
        2,
        'pr-watch: pending',
        `${branchNote(pr)}#${pr.number} is not done — ${pendingSummary(result)}.\n${pr.url}\n${result.pending.map(checkLine).join('\n')}\nRe-run without --once to wait, or run this again. Do not claim done.`,
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

main().catch((err) => couldNotRun(err.message))
