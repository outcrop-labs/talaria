#!/usr/bin/env node
// remote-gate — `bun run gate:remote`: compile and test on GitHub's runners,
// before a pull request exists.
//
// WHY. A local `cargo test`, clippy or svelte-check pins this machine and locks
// `api/target` for every other agent on it. GitHub's runners are free for this
// public repository, and `ci-offload.yml` runs ci.yml — the same jobs a pull
// request gets — on any push to `ci/<branch>`. This script is that push, and
// the read of its result: an agent offloads the compile and acts on the exit.
// Once the branch has a pull request, that PR's own CI is the proof and
// `pr-watch.mjs` is the watcher; this script refuses to run beside it.
//
// WHAT IT DOES, in order:
//   1. Refuses a tree with tracked changes (`dirty`: CI can only see commits;
//      an untracked file is a warning, never a block), a branch that is
//      `main`, `rc` or `testing` (`base`), and a branch with an open pull
//      request (`has-pr`).
//   2. Runs `bun run check` (`check`) and rustfmt — `cargo fmt --check`, never
//      a compile — on the Rust packages the branch touches (`fmt`). No cargo
//      here (a devbox), or a cargo that cannot run rustfmt: fmt is left to CI.
//   3. Force-pushes HEAD to `ci/<branch>` (light: Linux jobs, the surfaces the
//      branch moved) or `ci/full/<branch>` (`--full`: every surface, macOS,
//      Windows, iOS and e2e included). A `hotfix/`, `release/`, `sync/`,
//      `promote/` or `backflow/` branch is always full: that work goes straight
//      into rc (surfaces.mjs, FULL_LANES). A branch already under `ci/` is
//      pushed under its own name.
//   4. Finds the NEW `CI offload` run for that commit, and waits. It goes red
//      the moment any job fails, with that job's log tail, while the others
//      still run: a ui failure at minute 2 is not held for the api job's 20.
//   5. Green: waits for the same push's `offload / judge (review)` (the model
//      review, judge-review.yml; at most 12 minutes, inside --timeout),
//      prints what it listed, then deletes its ref and prints one proof line.
//      The review never changes the exit code; `--no-review` skips the wait.
//
// THE CONTRACT (scripts/hooks/README.md). stdin is not read.
//   exit 0 — green. One proof line on stdout (with --json, the JSON instead):
//            remote-gate: green — CI offload <url> · <ref> @ <sha12> · <light|full> · ran: … · skipped: … · review: …
//   exit 2 — block. The verdict on stderr opens with a line that is exactly
//            the token, one of:
//              remote-gate: dirty | base | has-pr | check | fmt | no-run |
//                           red | cancelled | timeout | pending
//            and the lines after it are the reason and the next command.
//            Anything on stderr before it is progress — `remote-gate: <word> —
//            …`, always with an em dash, never a token word — `bun run
//            check`'s own output, or `bun run`'s echo of the command (`$ node
//            scripts/hooks/remote-gate.mjs`). Not a pass.
//   else   — could not run: usage, not the repo root, no GitHub token, a push
//            git refused, GitHub unreachable. Never a pass.
//
// NOT A STOP HOOK. It runs for minutes — the wait is `--timeout`, 60 by
// default — and a harness kills a foreground command long before that (Claude
// Code at 10). Run it in the harness's background mode (Claude Code:
// `run_in_background`) and act on the exit. Killed, nothing is lost: `--watch
// <run-id>` resumes a run, and running it again on the same commit attaches to
// the run that commit already has instead of pushing again.
//
// WHICH RUN IS OURS. Before the push, the ids of the runs the ref already has
// are kept; after it, only a run of this commit whose id is not among them
// counts. Without that, a re-run on the same commit (after a flaky red, or
// after the ref was deleted and pushed again) read the OLD run's verdict back
// before the new run registered. A push that would move nothing — the ref is
// already at HEAD — starts no run, so that case attaches to the newest run the
// ref has for HEAD: in progress, it is watched; finished, its verdict is the
// answer, with `--rerun` named for a red.
//
// THE REF'S LIFE. Deleted on green (`--keep-ref` keeps it), and only while it
// still points at the commit that went green (a lease). Kept on red, cancelled
// and timeout, so `--rerun` and `--watch` have a run to come back to. Nothing
// else is left behind for long: ci-offload.yml's daily prune deletes every
// `ci/**` ref whose tip is more than 24 hours old, and the cleanup sweep
// deletes a merged worktree's `ci/wt/<name>` and `ci/full/wt/<name>`.
//
// GITHUB OVER REST, NOT gh. Every read is `fetch` with a token, so a devbox
// with no gh can run it: `talaria box new <name> --env GH_TOKEN=…`. The token
// is GH_TOKEN, else GITHUB_TOKEN, else `gh auth token` when gh exists — gh's
// own order. The repository is origin's. A job's log is a redirect to signed
// storage on another host: it is followed by hand, without the token.
//
// USAGE
//   bun run gate:remote [--full] [--detach] [--watch <run-id>] [--rerun]
//       [--json] [--timeout <min>] [--keep-ref] [--no-check] [--no-review]
//
//   --full       push to ci/full/<branch>: every surface and platform.
//   --detach     push, find the run, print its id, exit 2 `pending` (with
//                --rerun: start the re-run, and exit the same way).
//   --watch <id> skip everything before the wait and watch that run.
//   --rerun      re-run the failed jobs of this commit's latest finished
//                offload run (or of `--watch <id>`'s), and watch the attempt.
//                Refused once the ref holds another commit: a re-run joins
//                the ref's concurrency group and would cancel that one's run.
//   --json       stdout is {token, run_url, head_sha, ref, mode, attempt,
//                ran, skipped, failing, pending} for exit 0 and exit 2.
//   --timeout    minutes to wait for the run (default 60).
//   --keep-ref   do not delete the ref on green.
//   --no-check   skip `bun run check` and the fmt preflight.
//   --no-review  do not wait for the model review of the push.
//
// The tests are remote-gate.test.mjs (run by `bun run check`): pure, no network.

import { spawnSync } from 'node:child_process'
import { existsSync, realpathSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { needsFull, offloadBranch } from '../surfaces.mjs'
import { OFFLOAD_PREFIX, parseRepo } from './pr-watch.mjs'
import { OFFLOAD_REVIEW_CHECK, readReview, safeTerminal } from '../judge-review-wire.mjs'

export { needsFull }

/** The tokens of exit 2. The verdict's first stderr line is `remote-gate: <token>`. */
export const TOKENS = ['dirty', 'base', 'has-pr', 'check', 'fmt', 'no-run', 'red', 'cancelled', 'timeout', 'pending']

/** Branches nobody offloads: they move by pull request (docs/BRANCHES.md). */
export const BASES = ['main', 'rc', 'testing']

/** The workflow a push to ci/** runs. Matched by path, which a rename of its
 *  `name:` does not move. */
export const OFFLOAD_WORKFLOW = '.github/workflows/ci-offload.yml'
const OFFLOAD_NAME = 'CI offload'

/** The model review a ci/** push also starts (judge-review.yml). */
export const REVIEW_WORKFLOW = '.github/workflows/judge-review.yml'
/** The longest the green path waits for that review, inside --timeout. */
export const REVIEW_WAIT_MS = 12 * 60_000

/** ci-offload.yml calls ci.yml as job `offload`, so every CI job is named
 *  `offload / <job>`. A job outside that call (the daily prune) is not this
 *  run's work when it is skipped. */
const CALL_PREFIX = 'offload / '

const BOOLEAN_FLAGS = {
  '--full': 'full',
  '--detach': 'detach',
  '--rerun': 'rerun',
  '--json': 'json',
  '--keep-ref': 'keepRef',
  '--no-check': 'noCheck',
  '--no-review': 'noReview',
  '--help': 'help',
  '-h': 'help',
}

/**
 * The command line, or `{ error }`. Unknown arguments are an error: a typo'd
 * `--ful` silently running light would read as a full pass.
 * @param {string[]} argv
 */
export function parseArgs(argv) {
  const opts = {
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
  }
  for (let i = 0; i < argv.length; i++) {
    const raw = String(argv[i])
    const eq = raw.startsWith('--') ? raw.indexOf('=') : -1
    const name = eq > 0 ? raw.slice(0, eq) : raw
    const inline = eq > 0 ? raw.slice(eq + 1) : undefined
    if (name === '--watch' || name === '--timeout') {
      const value = inline ?? argv[++i]
      if (value === undefined || value === '' || String(value).startsWith('-')) return { error: `${name} needs a value` }
      if (name === '--watch') {
        if (!/^\d+$/.test(value)) return { error: `--watch takes a run id (digits), got ${value}` }
        opts.watch = Number(value)
      } else {
        const min = Number(value)
        if (!Number.isFinite(min) || min <= 0) return { error: `--timeout takes minutes > 0, got ${value}` }
        opts.timeoutMin = min
      }
      continue
    }
    const key = BOOLEAN_FLAGS[name]
    if (!key) return { error: `unknown argument ${raw}` }
    if (inline !== undefined) return { error: `${name} takes no value` }
    opts[key] = true
  }
  if (opts.detach && opts.watch !== null) return { error: '--detach and --watch do not combine: --watch is how a detached run is resumed' }
  return opts
}

/** The offload ref for a branch: `ci/<branch>`, or `ci/full/<branch>`. A
 *  branch already under `ci/` is pushed under its own name. */
export function refFor(branch, full) {
  const b = String(branch)
  if (b.startsWith(OFFLOAD_PREFIX)) return b
  return `${full ? 'ci/full/' : OFFLOAD_PREFIX}${b}`
}

/** The mode ci-offload.yml and surfaces.mjs give a push to this ref. */
export function modeOfRef(ref) {
  const r = String(ref)
  if (r.startsWith('ci/full/')) return 'full'
  return needsFull(offloadBranch(`refs/heads/${r}`) ?? '') ? 'full' : 'light'
}

/** The branch an offload ref carries (`ci/full/x` and `ci/x` are `x`). */
export function branchOfRef(ref) {
  return offloadBranch(`refs/heads/${ref}`) ?? String(ref)
}

function isOffloadRun(run) {
  const path = String(run?.path ?? '').replace(/@.*$/, '')
  return path ? path === OFFLOAD_WORKFLOW : run?.name === OFFLOAD_NAME
}

/**
 * The newest `CI offload` run of `sha` whose id is not in `snapshotIds` (the
 * runs the ref had before this push), or null. Run ids only grow, so the
 * largest is the newest.
 * @param {Array<Record<string, any>>} runs  REST `workflow_runs` rows
 * @param {string} sha
 * @param {Iterable<number|string>} [snapshotIds]
 */
export function pickNewRun(runs, sha, snapshotIds = []) {
  const seen = new Set([...snapshotIds].map(String))
  const fresh = (Array.isArray(runs) ? runs : [])
    .filter((r) => r && r.head_sha === sha && !seen.has(String(r.id)) && isOffloadRun(r))
    .sort((a, b) => Number(b.id) - Number(a.id))
  return fresh[0] ?? null
}

/**
 * One attempt's jobs, split the way the verdict reads them. Names drop the
 * `offload / ` prefix. A finished job that is not success, neutral or skipped
 * is failing — failure, timed_out, and cancelled (a job over its
 * `timeout-minutes` reads cancelled). A skipped job outside the offload call
 * (the prune) is not counted.
 * @param {Array<Record<string, any>>} jobs  REST `jobs` rows
 */
export function jobLines(jobs) {
  const ran = []
  const skipped = []
  const failing = []
  const pending = []
  for (const job of Array.isArray(jobs) ? jobs : []) {
    const full = String(job?.name ?? '')
    const inCall = full.startsWith(CALL_PREFIX)
    const name = inCall ? full.slice(CALL_PREFIX.length) : full
    if (job?.status !== 'completed') pending.push(name)
    else if (job.conclusion === 'skipped') {
      if (inCall) skipped.push(name)
    } else if (job.conclusion === 'success' || job.conclusion === 'neutral') ran.push(name)
    else failing.push({ id: job.id, name, conclusion: job.conclusion || 'unknown', url: job.html_url || '' })
  }
  return { ran, skipped, failing, pending }
}

/**
 * The verdict on one poll: `green`, `red`, `cancelled` or `wait`. Red comes
 * as soon as any job has failed, while the rest still run (`early`). A run
 * cancelled with no job failed — a newer push superseded it — is `cancelled`,
 * and so is a startup failure. `onlyCancelled` marks a red whose failing jobs
 * are all cancelled: on a run still in progress that is either a job over its
 * time limit or a supersede in flight, which the caller tells apart by asking
 * whether the ref moved.
 */
export function verdictOf(run, jobs) {
  const split = jobLines(jobs)
  const onlyCancelled = split.failing.length > 0 && split.failing.every((j) => j.conclusion === 'cancelled')
  if (run?.status === 'completed') {
    if (run.conclusion === 'success' && split.failing.length === 0) return { kind: 'green', ...split }
    if (run.conclusion === 'startup_failure') return { kind: 'cancelled', ...split }
    if (run.conclusion === 'cancelled' && (split.failing.length === 0 || onlyCancelled)) return { kind: 'cancelled', ...split }
    return { kind: 'red', early: false, onlyCancelled, ...split }
  }
  if (split.failing.length) return { kind: 'red', early: true, onlyCancelled, ...split }
  return { kind: 'wait', ...split }
}

/** The exit-0 line: the one state an agent may cite. */
export function proofLine({ run, ref, sha, mode, verdict, review }) {
  const attempt = Number(run?.run_attempt) > 1 ? ` · attempt ${run.run_attempt}` : ''
  const list = (names) => (names.length ? names.join(', ') : 'none')
  return `remote-gate: green — ${run?.name || OFFLOAD_NAME} ${run?.html_url || ''} · ${ref} @ ${String(sha).slice(0, 12)} · ${mode}${attempt} · ran: ${list(verdict.ran)} · skipped: ${list(verdict.skipped)}${review ? ` · ${reviewClause(review)}` : ''}`
}

/**
 * The newest judge-review.yml push run of `sha` whose id is not in
 * `snapshotIds` (the ref's review runs before this push), or null.
 * @param {Array<Record<string, any>>} runs  REST `workflow_runs` rows
 */
export function pickReviewRun(runs, sha, snapshotIds = []) {
  const seen = new Set([...snapshotIds].map(String))
  const fresh = (Array.isArray(runs) ? runs : [])
    .filter((r) => r && r.head_sha === sha && r.event === 'push' && !seen.has(String(r.id)) && String(r.path ?? '').replace(/@.*$/, '') === REVIEW_WORKFLOW)
    .sort((a, b) => Number(b.id) - Number(a.id))
  return fresh[0] ?? null
}

/** What the review job's annotations say: {status, reason, listed, more, cost}. */
export function reviewFromAnnotations(annotations) {
  const r = readReview(annotations)
  if (!r.verdict) return { status: 'unread', reason: 'no verdict annotation', listed: [], more: 0, cost: null }
  return {
    status: r.verdict.status,
    reason: r.verdict.reason && r.verdict.reason !== 'none' ? r.verdict.reason : null,
    listed: r.findings.map((f) => ({ id: f.id, severity: f.severity, category: f.category, path: f.path, line: f.line, title: String(f.message).split(' — ')[0] })),
    more: Number(r.verdict.more || 0),
    cost: r.verdict.cost && r.verdict.cost !== 'na' ? r.verdict.cost : null,
  }
}

/** The proof line's review clause. */
export function reviewClause(review) {
  if (!review || review.status === 'none') return 'review: none'
  if (review.status === 'unread') return `review: not read (${review.reason || 'unknown'})`
  if (review.status !== 'reviewed') return `review: not reviewed (${review.reason || review.status})`
  return `review: ${review.listed.length} to read (${review.more} more)${review.cost ? ` $${review.cost}` : ''}`
}

/** One stderr line per listed finding: id, place, title — never the failure or fix. */
export function reviewFindingLines(review) {
  return (review?.listed ?? []).map((f) => `remote-gate: review — ${f.id} ${f.severity} ${f.category} ${f.path}:${f.line} — ${safeTerminal(f.title, 100).replace(/\n/g, ' ')}`)
}

const STAMP = /^\uFEFF?\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z ?/
// eslint-disable-next-line no-control-regex
const ANSI = /\u001b\[[0-9;]*[A-Za-z]/g

/** A test runner's line for a test that PASSED — bun's `(pass) …`, vitest's
 *  `✓ …`. A red's tail is for the failure: bun prints every passing test, so
 *  80 lines of them hid the one that failed (the cli job's first red here). */
const PASSED = /^\s*(?:\(pass\) |✓ )/

/**
 * A job log, cut to what a reader needs: no timestamps, colour codes, blank
 * lines, passing-test lines or the groups they leave empty; ended at the failing step (`##[error]Process completed
 * with exit code`) or, with none, before the post-job cleanup; then the last
 * `maxLines` lines, and at most `maxChars` characters, whole lines first.
 */
export function trimLog(text, maxLines = 80, maxChars = 12000) {
  let lines = String(text ?? '')
    .replace(/\r\n?/g, '\n')
    .split('\n')
    .map((l) => l.replace(STAMP, '').replace(ANSI, ''))
    .filter((l) => l.trim() && !PASSED.test(l))
  // A group left empty once its passing tests are gone (bun prints one per
  // test file) says nothing.
  lines = lines.filter((l, i, all) => !(l.startsWith('##[group]') && all[i + 1] === '##[endgroup]') && !(l === '##[endgroup]' && all[i - 1]?.startsWith('##[group]')))
  const failed = lines.findIndex((l) => l.startsWith('##[error]Process completed with exit code'))
  if (failed >= 0) lines = lines.slice(0, failed + 1)
  else {
    const post = lines.findIndex((l) => l.trim() === 'Post job cleanup.')
    if (post > 0) lines = lines.slice(0, post)
  }
  while (lines.length && !lines[lines.length - 1].trim()) lines.pop()
  const tail = lines.slice(-Math.max(1, maxLines))
  while (tail.length > 1 && tail.join('\n').length > maxChars) tail.shift()
  const out = tail.join('\n')
  return out.length > maxChars ? out.slice(-maxChars) : out
}

/** Seconds to the next poll: 20 first, doubling, capped at 60, never past
 *  the time left and never under 1. */
export function nextDelay(prev, leftSec = Infinity) {
  const base = prev == null ? 20 : Math.min(Number(prev) * 2, 60)
  return Math.max(1, Math.min(base, Math.floor(leftSec)))
}

/**
 * The GitHub token and where it came from: GH_TOKEN, else GITHUB_TOKEN, else
 * `ghTokenFn()` (`gh auth token`) — gh's own order. null when there is none.
 * @param {Record<string, string|undefined>} env
 * @param {() => string|null|undefined} ghTokenFn
 */
export function tokenFrom(env, ghTokenFn = () => null) {
  for (const key of ['GH_TOKEN', 'GITHUB_TOKEN']) {
    const value = String(env?.[key] ?? '').trim()
    if (value) return { token: value, source: key }
  }
  const fromGh = String(ghTokenFn() ?? '').trim()
  return fromGh ? { token: fromGh, source: '`gh auth token`' } : null
}

// ── the run ─────────────────────────────────────────────────────────────────

const ROOT = process.cwd()
const CHILD_ENV = { ...process.env, GH_PROMPT_DISABLED: '1', GH_NO_UPDATE_NOTIFIER: '1', GIT_TERMINAL_PROMPT: '0' }
const TOKEN_HELP =
  'set GH_TOKEN — fine-grained, outcrop-labs/talaria: Actions read/write, Contents read, Pull requests read, Metadata read — or `gh auth login`; in a devbox pass it with `talaria box new <name> --env GH_TOKEN=…`'
const NEXT_RED =
  'Fix, commit, run gate:remote again (a new push cancels this run). Reproduce one failing test locally only if the log is not enough (`cargo test -p <crate> <test>`, `bunx vitest run <file>`). Infra (`The job was not acquired by Runner`, registry, a secret): `bun run gate:remote --rerun` once, then report the job link — not self-fixable.'

/** Thrown to end the run with an exit code once its output is written. */
class Exit {
  constructor(code) {
    this.code = code
  }
}

let jsonOut = false
let payload = {}

function couldNotRun(why) {
  process.stderr.write(`remote-gate: could not run — not a pass: ${why}\n`)
  throw new Exit(1)
}

function progress(word, text) {
  process.stderr.write(`remote-gate: ${word} — ${text}\n`)
}

function writeJson(token) {
  if (!jsonOut) return
  process.stdout.write(
    `${JSON.stringify({
      token,
      run_url: null,
      head_sha: null,
      ref: null,
      mode: null,
      attempt: null,
      ran: [],
      skipped: [],
      failing: [],
      pending: [],
      review: null,
      ...payload,
    })}\n`,
  )
}

function block(token, lines) {
  writeJson(token)
  process.stderr.write(`${[`remote-gate: ${token}`, ...lines.filter((l) => l !== null && l !== undefined)].join('\n')}\n`)
  throw new Exit(2)
}

function run(cmd, args, { timeout = 60_000, cwd = ROOT, env = CHILD_ENV } = {}) {
  const r = spawnSync(cmd, args, { cwd, env, encoding: 'utf8', timeout, maxBuffer: 64 * 1024 * 1024 })
  return { ok: !r.error && r.status === 0, status: r.status, error: r.error, out: (r.stdout || '').trim(), err: (r.stderr || '').trim() }
}

function git(args, opts) {
  return run('git', args, opts)
}

function firstLine(text) {
  return String(text || '').split('\n').find((l) => l.trim()) || ''
}

function why(r) {
  if (r.error) return r.error.code === 'ETIMEDOUT' ? 'timed out' : r.error.message
  return firstLine(r.err) || firstLine(r.out) || `exit ${r.status}`
}

// ── GitHub ──────────────────────────────────────────────────────────────────

class ApiError extends Error {
  constructor(status, message) {
    super(message)
    this.status = status
  }
}

function apiBase(host) {
  return host === 'github.com' ? 'https://api.github.com' : `https://${host}/api/v3`
}

function headers(gh, extra = {}) {
  return {
    authorization: `Bearer ${gh.token}`,
    accept: 'application/vnd.github+json',
    'x-github-api-version': '2022-11-28',
    'user-agent': 'talaria-remote-gate',
    ...extra,
  }
}

async function api(gh, method, path, body) {
  let res
  try {
    res = await fetch(`${gh.base}${path}`, {
      method,
      headers: headers(gh, body ? { 'content-type': 'application/json' } : {}),
      body: body ? JSON.stringify(body) : undefined,
      signal: AbortSignal.timeout(20_000),
    })
  } catch (e) {
    throw new ApiError(0, `${method} ${path}: ${e?.name === 'TimeoutError' ? 'no answer in 20s' : e?.message || e}`)
  }
  const text = await res.text().catch(() => '')
  if (!res.ok) {
    let message = text.slice(0, 200)
    try {
      message = JSON.parse(text).message || message
    } catch {
      // not JSON: the raw start of the body says enough
    }
    const scope = res.status === 401 || res.status === 403 ? ` — the token from ${gh.source} was refused; ${TOKEN_HELP}` : ''
    throw new ApiError(res.status, `${method} ${path}: GitHub answered ${res.status} ${message}${scope}`)
  }
  if (!text) return null
  try {
    return JSON.parse(text)
  } catch {
    return null
  }
}

/** A GET that a poll may retry: a failed read is not a verdict. */
async function read(gh, path, state) {
  for (let attempt = 1; ; attempt++) {
    try {
      return await api(gh, 'GET', path)
    } catch (e) {
      const permanent = e instanceof ApiError && e.status >= 400 && e.status < 500 && e.status !== 429
      if (permanent || attempt >= state.tries) throw e
      progress('retry', `${e.message} — read ${attempt} of ${state.tries} failed, again in 10s`)
      await sleep(10_000)
    }
  }
}

const repoPath = (gh) => `/repos/${gh.slug}`

async function reviewRuns(gh, { branch, sha, perPage = 30 } = {}) {
  const q = new URLSearchParams({ event: 'push', per_page: String(perPage) })
  if (branch) q.set('branch', branch)
  if (sha) q.set('head_sha', sha)
  const body = await read(gh, `${repoPath(gh)}/actions/runs?${q}`, { tries: 3 })
  return (body?.workflow_runs ?? []).filter((r) => String(r?.path ?? '').replace(/@.*$/, '') === REVIEW_WORKFLOW)
}

/** The push's model review: wait for its run (≤ REVIEW_WAIT_MS, inside the
 *  deadline), then read the `offload / judge (review)` job's annotations.
 *  Never throws: a failure is `unread` or `none`. */
async function awaitReview(gh, ref, sha, snapshot, deadlineMs) {
  const until = Math.min(Date.now() + REVIEW_WAIT_MS, deadlineMs)
  let delay = null
  let runRow = null
  try {
    for (;;) {
      runRow = runRow ? await read(gh, `${repoPath(gh)}/actions/runs/${runRow.id}`, { tries: 3 }) : pickReviewRun(await reviewRuns(gh, { branch: ref, sha }), sha, snapshot)
      if (runRow?.status === 'completed') break
      if (Date.now() >= until) return runRow ? { status: 'unread', reason: `the review run ${runRow.html_url} had not finished`, listed: [], more: 0, cost: null } : { status: 'none', listed: [], more: 0 }
      delay = nextDelay(delay, (until - Date.now()) / 1000)
      progress('review', runRow ? `${runRow.html_url} — waiting for the model review` : 'waiting for the judge review run of this push')
      await sleep(delay * 1000)
    }
    const jobs = (await read(gh, `${repoPath(gh)}/actions/runs/${runRow.id}/jobs?per_page=50`, { tries: 3 }))?.jobs ?? []
    const job = jobs.find((j) => j.name === OFFLOAD_REVIEW_CHECK)
    if (!job || job.conclusion === 'skipped') return { status: 'none', listed: [], more: 0 }
    const annotations = await read(gh, `${repoPath(gh)}/check-runs/${job.id}/annotations?per_page=100`, { tries: 3 })
    return { ...reviewFromAnnotations(annotations), url: runRow.html_url }
  } catch (e) {
    return { status: 'unread', reason: safeTerminal(e?.message || String(e), 200), listed: [], more: 0, cost: null }
  }
}

async function offloadRuns(gh, { branch, sha, perPage = 100 } = {}) {
  const q = new URLSearchParams({ event: 'push', per_page: String(perPage) })
  if (branch) q.set('branch', branch)
  if (sha) q.set('head_sha', sha)
  const body = await read(gh, `${repoPath(gh)}/actions/runs?${q}`, { tries: 3 })
  return (body?.workflow_runs ?? []).filter(isOffloadRun)
}

/** A job's log, trimmed. The endpoint redirects to signed storage on another
 *  host: followed by hand, so the token never leaves for that host. */
async function jobLog(gh, jobId) {
  try {
    const res = await fetch(`${gh.base}${repoPath(gh)}/actions/jobs/${jobId}/logs`, {
      headers: headers(gh),
      redirect: 'manual',
      signal: AbortSignal.timeout(20_000),
    })
    let text
    if (res.status >= 300 && res.status < 400) {
      const location = res.headers.get('location')
      if (!location) throw new Error(`GitHub answered ${res.status} with no location`)
      const blob = await fetch(location, { signal: AbortSignal.timeout(20_000) })
      if (!blob.ok) throw new Error(`log storage answered ${blob.status}`)
      text = await blob.text()
    } else if (res.ok) {
      text = await res.text()
    } else {
      throw new Error(`GitHub answered ${res.status}`)
    }
    return trimLog(text, 80, 12000) || '(the log is empty)'
  } catch (e) {
    return `(log fetch failed: ${e?.name === 'TimeoutError' ? 'no answer in 20s' : e?.message || e})`
  }
}

// ── local preflight ─────────────────────────────────────────────────────────

function preflightTree() {
  const unstaged = git(['diff', '--quiet', 'HEAD'])
  const staged = git(['diff', '--cached', '--quiet'])
  if (unstaged.error || staged.error) couldNotRun(`git diff: ${why(unstaged.error ? unstaged : staged)}`)
  if (unstaged.status !== 0 || staged.status !== 0) {
    const listing = git(['status', '--porcelain', '--untracked-files=no']).out.split('\n').filter(Boolean)
    const more = listing.length > 20 ? [`  … and ${listing.length - 20} more`] : []
    block('dirty', [
      'tracked changes are not committed — commit or stash; CI can only see commits:',
      ...listing.slice(0, 20).map((l) => `  ${l}`),
      ...more,
    ])
  }
  const untracked = git(['ls-files', '--others', '--exclude-standard']).out.split('\n').filter(Boolean)
  if (untracked.length) {
    const names = untracked.slice(0, 5).join(', ') + (untracked.length > 5 ? `, … (${untracked.length - 5} more)` : '')
    progress('untracked', `${untracked.length} untracked file(s), which CI cannot see: ${names}`)
  }
}

function runCheck() {
  progress('preflight', 'bun run check')
  const r = spawnSync('bun', ['run', 'check'], { cwd: ROOT, env: CHILD_ENV, stdio: ['ignore', 2, 2], timeout: 600_000 })
  if (r.error) couldNotRun(`bun run check: ${r.error.code === 'ETIMEDOUT' ? 'timed out' : r.error.message}`)
  if (r.status !== 0) {
    block('check', [
      `\`bun run check\` failed (exit ${r.status ?? r.signal}) — its output is above.`,
      'Fix it and commit; then run gate:remote again. Nothing was pushed.',
    ])
  }
}

/** surfaces.mjs's plan of this branch, as an offload of it would see it, or
 *  `{ error }`. Local git only — the preview and the fmt preflight. */
function localPlan(full) {
  const r = run(process.execPath, [join(ROOT, 'scripts/surfaces.mjs'), 'plan', '--json', ...(full ? ['--full'] : [])], { timeout: 120_000 })
  if (!r.ok) return { error: r.status === 2 ? 'neither origin/testing nor origin/rc is in this clone' : why(r) }
  try {
    return JSON.parse(r.out)
  } catch {
    return { error: 'the plan was not JSON' }
  }
}

function hasCargo() {
  return run('cargo', ['--version'], { timeout: 30_000 }).ok
}

/** rustfmt on the packages the branch touches: `cargo fmt --check`, which
 *  formats nothing and compiles nothing. Only a reported diff blocks; a cargo
 *  that cannot run rustfmt here leaves fmt to CI. */
function fmtPreflight(plan) {
  if (plan.error) {
    progress('preflight', `no local plan (${plan.error}) — fmt left to CI`)
    return
  }
  const local = plan.local || {}
  const jobs = []
  if (local.apiGraph?.length) {
    jobs.push({ dir: 'api', args: ['fmt', '--all', '--check'], fix: '(cd api && cargo fmt --all)' })
  } else if (local.apiPackages?.length) {
    const p = local.apiPackages.flatMap((name) => ['-p', name])
    jobs.push({ dir: 'api', args: ['fmt', ...p, '--check'], fix: `(cd api && cargo fmt ${p.join(' ')})` })
  }
  if (local.desktopCargo) jobs.push({ dir: 'desktop/src-tauri', args: ['fmt', '--check'], fix: '(cd desktop/src-tauri && cargo fmt)' })
  if (!jobs.length) return
  if (!hasCargo()) {
    progress('preflight', 'no cargo here — fmt left to CI')
    return
  }
  for (const job of jobs) {
    const label = `cargo ${job.args.join(' ')} in ${job.dir}`
    // cwd, not --manifest-path: rustup honors the directory's rust-toolchain.toml.
    // No auto-install: a missing toolchain is not worth a download here.
    const r = run('cargo', job.args, { cwd: join(ROOT, job.dir), timeout: 300_000, env: { ...CHILD_ENV, RUSTUP_AUTO_INSTALL: '0' } })
    if (r.ok) {
      progress('preflight', `${label}: clean`)
      continue
    }
    if (!/^Diff in /m.test(r.out)) {
      progress('preflight', `${label} could not decide here (${why(r)}) — fmt left to CI`)
      continue
    }
    const diff = r.out.split('\n')
    block('fmt', [
      `${label} found unformatted code:`,
      ...diff.slice(0, 60),
      ...(diff.length > 60 ? [`… ${diff.length - 60} more line(s)`] : []),
      `Fix: ${job.fix}, commit, and run gate:remote again. Nothing was pushed.`,
    ])
  }
}

function planPreview(plan, ref, sha, mode) {
  const head = `${ref} @ ${sha.slice(0, 12)} · ${mode}`
  if (plan.error) {
    progress('plan', `${head} — no local preview (${plan.error}); CI plans the run itself`)
    return
  }
  const names = Object.keys(plan.surfaces || {})
  // e2e is a full-mode job in ci.yml, like the macOS, Windows and iOS legs
  // (surfaces.mjs's own preview says the same).
  const runs = (n) => plan.surfaces[n] && (plan.all || n !== 'e2e')
  const on = names.filter(runs)
  const off = names.filter((n) => !runs(n))
  const base = plan.base ? `${plan.files?.length ?? 0} file(s) since ${String(plan.base.sha).slice(0, 12)} (merge-base with ${plan.base.ref})` : ''
  const jobs = plan.all
    ? 'CI runs every surface, macOS, Windows, iOS and e2e included'
    : `CI runs ${on.length ? on.join(', ') : 'only the invariants'}${off.length ? `; skips ${off.join(', ')}` : ''}`
  progress('plan', `${head} — ${base}; ${jobs}`)
}

// ── waiting ─────────────────────────────────────────────────────────────────

/** Where the ref points on origin now: a sha, '' when it is gone, null when
 *  origin did not answer. */
function remoteSha(ref) {
  const r = git(['ls-remote', 'origin', `refs/heads/${ref}`], { timeout: 60_000 })
  if (!r.ok) return null
  // ls-remote matches a pattern against the TAIL of a ref; only the exact one counts.
  const line = r.out.split('\n').map((l) => l.split(/\s+/)).find(([, name]) => name === `refs/heads/${ref}`)
  return line ? line[0] : ''
}

/** Poll for the run a push of `sha` to `ref` started: not in `snapshot`. */
async function awaitNewRun(gh, ref, sha, snapshot) {
  const until = Date.now() + 180_000
  for (;;) {
    const found = pickNewRun(await offloadRuns(gh, { branch: ref, sha }), sha, snapshot)
    if (found) return found
    if (Date.now() >= until) return null
    await sleep(10_000)
  }
}

function setPayload(run, ref, sha, verdict) {
  Object.assign(payload, {
    run_url: run?.html_url ?? null,
    head_sha: sha ?? null,
    ref: ref ?? null,
    mode: ref ? modeOfRef(ref) : null,
    attempt: run?.run_attempt ?? null,
  })
  if (verdict) {
    Object.assign(payload, {
      ran: verdict.ran,
      skipped: verdict.skipped,
      failing: verdict.failing.map(({ name, conclusion, url }) => ({ name, conclusion, url })),
      pending: verdict.pending,
    })
  }
}

/**
 * Poll one run until it has a verdict. `minAttempt`: a re-run's attempt,
 * which GitHub may not show for a moment after the POST. `settle`: wait for
 * the run to finish, never return early red (a re-run needs a finished run).
 */
async function watch(gh, id, { deadline, minAttempt = 1, settle = false }) {
  let delay = null
  let failures = 0
  for (;;) {
    let runRow
    let jobs
    try {
      runRow = await api(gh, 'GET', `${repoPath(gh)}/actions/runs/${id}`)
      const attempt = Number(runRow?.run_attempt) || 1
      if (attempt >= minAttempt) {
        const body = await api(gh, 'GET', `${repoPath(gh)}/actions/runs/${id}/attempts/${attempt}/jobs?per_page=100`)
        jobs = body?.jobs ?? []
      }
      failures = 0
    } catch (e) {
      const permanent = e instanceof ApiError && e.status >= 400 && e.status < 500 && e.status !== 429
      failures++
      if (permanent || failures >= 5) couldNotRun(`${e.message} — resume: bun run gate:remote --watch ${id}`)
      progress('retry', `${e.message} — read failed (${failures} in a row), polling again`)
      runRow = null
    }
    const left = (deadline - Date.now()) / 1000
    if (runRow && jobs) {
      const v = verdictOf(runRow, jobs)
      if (v.kind !== 'wait' && !(settle && runRow.status !== 'completed')) return { run: runRow, verdict: v }
      if (left <= 0) return { run: runRow, verdict: { ...v, kind: 'timeout' } }
      delay = nextDelay(delay, left)
      const done = v.ran.length + v.skipped.length + v.failing.length
      const total = done + v.pending.length
      progress('waiting', `${runRow.html_url} — ${done}/${total} job(s) done${v.pending.length ? `; running: ${v.pending.join(', ')}` : ''} — next poll in ${delay}s, ${Math.ceil(left / 60)}m left`)
    } else {
      // The last read failed: the id is still the resume.
      if (left <= 0) return { run: runRow ?? { id }, verdict: { kind: 'timeout', ran: [], skipped: [], failing: [], pending: [] } }
      delay = nextDelay(delay, left)
      if (runRow) progress('waiting', `${runRow.html_url} — attempt ${minAttempt} has not started yet — next poll in ${delay}s`)
    }
    await sleep(delay * 1000)
  }
}

async function report(gh, { run: runRow, verdict }, { ref, sha, branch, opts, already = false, reviewSnapshot = [] }) {
  const mode = modeOfRef(ref)
  setPayload(runRow, ref, sha, verdict)
  const url = runRow?.html_url || '(run url unknown)'
  const pushedAs = `head ${branch} @ ${String(sha).slice(0, 12)} (pushed as ${ref})`

  // A run still in progress whose only failures are cancelled jobs is a
  // supersede in flight when the ref has moved on; otherwise a job hit its
  // time limit, which is red.
  if (verdict.kind === 'red' && verdict.early && verdict.onlyCancelled) {
    const now = remoteSha(ref)
    if (now !== null && now !== sha) verdict = { ...verdict, kind: 'cancelled' }
  }

  if (verdict.kind === 'green') {
    // The model review of the same push, before the ref goes: its run fetches
    // this sha, and the deletion must not race it. It never changes the exit.
    let review = null
    if (!opts.noReview) {
      review = await awaitReview(gh, ref, sha, reviewSnapshot, opts.deadlineMs ?? Date.now() + REVIEW_WAIT_MS)
      for (const line of reviewFindingLines(review)) process.stderr.write(`${line}\n`)
      payload.review = review
    }
    const proof = proofLine({ run: runRow, ref, sha, mode, verdict, review })
    payload.proof = proof
    if (!opts.keepRef && String(ref).startsWith(OFFLOAD_PREFIX)) {
      // The lease: delete only while the ref still points at the commit that
      // went green. A newer push to it is someone's run, not ours to end.
      const del = git(['push', '--quiet', `--force-with-lease=refs/heads/${ref}:${sha}`, 'origin', `:refs/heads/${ref}`], { timeout: 120_000 })
      if (del.ok) progress('ref', `deleted ${ref}`)
      else progress('ref', `${ref} left in place (${why(del)}); the daily prune removes it`)
    }
    writeJson('green')
    if (!jsonOut) process.stdout.write(`${proof}\n`)
    throw new Exit(0)
  }

  const again = already ? ['already ran for this sha — `--rerun` re-runs its failed jobs'] : []

  if (verdict.kind === 'cancelled') {
    const now = remoteSha(ref)
    let newer = []
    if (now && now !== sha) {
      const runs = await offloadRuns(gh, { branch: ref, sha: now }).catch(() => [])
      const latest = pickNewRun(runs, now, [])
      newer = [`${ref} moved to ${now.slice(0, 12)}${latest ? `: its run is ${latest.html_url}` : ''}`]
    }
    block('cancelled', [
      url,
      pushedAs,
      `the run ended ${runRow?.conclusion || 'cancelled'} and no job failed${verdict.failing.length ? ` (${verdict.failing.length} cancelled)` : ''} — superseded by a newer push, cancelled by hand, or it never started.`,
      ...newer,
      ...again,
      'Run gate:remote again for this commit, or `bun run gate:remote --rerun` to re-run this run. The ref is kept.',
    ])
  }

  if (verdict.kind === 'timeout') {
    block('timeout', [
      url,
      pushedAs,
      `no verdict after ${opts.timeoutMin} min${verdict.pending?.length ? `; still running: ${verdict.pending.join(', ')}` : ''}.`,
      `resume: bun run gate:remote --watch ${runRow?.id ?? '<run-id>'}`,
      'Not a pass. The ref is kept.',
    ])
  }

  // red
  const order = { failure: 0, timed_out: 1 }
  const byWeight = [...verdict.failing].sort((a, b) => (order[a.conclusion] ?? 2) - (order[b.conclusion] ?? 2))
  const logged = byWeight.slice(0, 3)
  const tails = []
  for (const job of logged) {
    progress('logs', `fetching the log of ${job.name}`)
    tails.push(`── ${job.name} (${job.conclusion}) — log tail ──`, await jobLog(gh, job.id))
  }
  const failingLines = verdict.failing.length
    ? verdict.failing.map((j) => `  ${j.name} — ${j.conclusion} — ${j.url}`)
    : [`  the run concluded ${runRow?.conclusion} and no job failed — open the run.`]
  const more = verdict.failing.length > logged.length ? [`${verdict.failing.length - logged.length} more failing job(s) not logged`] : []
  const running = verdict.pending.length ? [`still running: ${verdict.pending.join(', ')}`] : []
  block('red', [url, pushedAs, ...failingLines, ...tails, ...more, ...running, ...again, NEXT_RED, 'The ref is kept.'])
}

// ── main ────────────────────────────────────────────────────────────────────

function help() {
  return `remote-gate — compile and test on GitHub before a pull request exists.

  bun run gate:remote [--full] [--detach] [--watch <run-id>] [--rerun]
      [--json] [--timeout <min>] [--keep-ref] [--no-check] [--no-review]

Pushes HEAD to ci/<branch> (--full: ci/full/<branch>), watches the CI offload
run, and goes red on the first failing job with its log. Run it in the
background and act on the exit:
  0  green — one proof line on stdout; the ref is deleted
  2  remote-gate: ${TOKENS.join(' | ')} — first line of the verdict on stderr
  1  could not run — not a pass

Needs a GitHub token: GH_TOKEN, GITHUB_TOKEN, or gh's login. A branch with an
open pull request is refused: its CI is the proof (scripts/hooks/pr-watch.mjs).
`
}

async function main(opts) {
  jsonOut = opts.json
  if (!existsSync(join(ROOT, 'scripts/hooks/remote-gate.mjs'))) couldNotRun('run from the repo root')
  const deadline = () => Date.now() + opts.timeoutMin * 60_000
  opts.deadlineMs = deadline()
  const resume = opts.watch !== null || opts.rerun

  // (b) Tracked state. A resume pushes nothing, so it does not care.
  if (!resume) preflightTree()

  // (c) The commit and the branch.
  const head = git(['rev-parse', 'HEAD'])
  if (!head.ok) couldNotRun(`git rev-parse HEAD: ${why(head)}`)
  const sha = head.out
  const symbolic = git(['symbolic-ref', '--short', '-q', 'HEAD'])
  const named = symbolic.ok ? symbolic.out : ''
  const branch = named || `detached-${sha.slice(0, 12)}`
  if (!resume && BASES.includes(branch)) {
    block('base', [`\`${branch}\` moves by pull request, never by offload — cut a working branch: git switch -c wt/<name>`])
  }

  // GitHub: origin's repository, and a token.
  const origin = git(['remote', 'get-url', 'origin'])
  const repo = origin.ok ? parseRepo(origin.out) : null
  if (!repo) couldNotRun(`origin is not a GitHub repository (${origin.ok ? origin.out : why(origin)})`)
  const ghToken = () => {
    const r = run('gh', ['auth', 'token', '--hostname', repo.host], { timeout: 15_000 })
    return r.ok ? r.out : null
  }
  const auth = tokenFrom(process.env, ghToken)
  if (!auth) couldNotRun(`no GitHub token: ${TOKEN_HELP}`)
  const gh = { base: apiBase(repo.host), slug: repo.slug, token: auth.token, source: auth.source }

  // A run named by id must be an offload's: green deletes its ref, and a
  // pull request's run is pr-watch's to read.
  const offloadRunById = async (id) => {
    const row = await api(gh, 'GET', `${repoPath(gh)}/actions/runs/${id}`).catch((e) => couldNotRun(e.message))
    if (!isOffloadRun(row) || !String(row?.head_branch ?? '').startsWith(OFFLOAD_PREFIX)) {
      couldNotRun(`run ${id} is not a CI offload run (${row?.name ?? '?'} on ${row?.head_branch ?? '?'}) — a pull request's runs are pr-watch's to read`)
    }
    return row
  }

  // (m) --watch: that run, nothing before it.
  if (opts.watch !== null && !opts.rerun) {
    const runRow = await offloadRunById(opts.watch)
    const ref = runRow.head_branch
    setPayload(runRow, ref, runRow.head_sha)
    progress('run', `watching ${runRow.html_url} (${ref} @ ${String(runRow.head_sha).slice(0, 12)}, attempt ${runRow.run_attempt})`)
    const result = await watch(gh, runRow.id, { deadline: deadline() })
    return report(gh, result, { ref, sha: runRow.head_sha, branch: branchOfRef(ref), opts })
  }

  // (l) --rerun: the failed jobs of a finished run, then its new attempt.
  if (opts.rerun) {
    let target
    if (opts.watch !== null) {
      target = await offloadRunById(opts.watch)
    } else {
      const refs = new Set([refFor(branch, false), refFor(branch, true)])
      const runs = (await offloadRuns(gh, { sha }).catch((e) => couldNotRun(e.message))).filter((r) => refs.has(r.head_branch))
      target = pickNewRun(runs, sha, [])
      if (!target) {
        block('no-run', [`no CI offload run of ${sha.slice(0, 12)} on ${[...refs].join(' or ')} — run \`bun run gate:remote\` first; --rerun re-runs a run that exists.`])
      }
    }
    const ref = target.head_branch
    const end = deadline()
    if (target.status !== 'completed') {
      progress('rerun', `${target.html_url} is still running; a re-run needs it finished — waiting`)
      const settled = await watch(gh, target.id, { deadline: end, settle: true })
      if (settled.verdict.kind === 'timeout') return report(gh, settled, { ref, sha: target.head_sha, branch: branchOfRef(ref), opts })
      target = settled.run
    }
    if (target.conclusion === 'success') {
      progress('rerun', `${target.html_url} is green — nothing to re-run`)
      const result = await watch(gh, target.id, { deadline: end })
      return report(gh, result, { ref, sha: target.head_sha, branch: branchOfRef(ref), opts })
    }
    // A re-run joins the ref's concurrency group (ci-offload.yml): re-running
    // an older commit's run would cancel the run of the commit the ref holds now.
    const now = remoteSha(ref)
    if (now === null) couldNotRun(`git ls-remote origin refs/heads/${ref} did not answer`)
    if (now && now !== target.head_sha) {
      couldNotRun(`${ref} has moved on to ${now.slice(0, 12)}: re-running ${target.html_url} (${String(target.head_sha).slice(0, 12)}) would cancel that commit's run — run gate:remote on the commit you want checked`)
    }
    await api(gh, 'POST', `${repoPath(gh)}/actions/runs/${target.id}/rerun-failed-jobs`).catch((e) => couldNotRun(`re-run refused: ${e.message}`))
    const attempt = (Number(target.run_attempt) || 1) + 1
    progress('rerun', `re-running the failed jobs of ${target.html_url} as attempt ${attempt}`)
    // --detach: the re-run is started; its id is the resume, as after a push.
    // It leaves only once GitHub shows the new attempt: until then the run
    // still reads as the finished one, and a `--watch` that came straight
    // after read that attempt's red back as the re-run's.
    if (opts.detach) {
      const until = Date.now() + 120_000
      for (;;) {
        const row = await read(gh, `${repoPath(gh)}/actions/runs/${target.id}`, { tries: 3 }).catch((e) => couldNotRun(e.message))
        if (Number(row?.run_attempt) >= attempt) break
        if (Date.now() >= until) couldNotRun(`GitHub accepted the re-run of ${target.html_url} but shows no attempt ${attempt} after 120s — give it a minute, then: bun run gate:remote --watch ${target.id}`)
        await sleep(3_000)
      }
      setPayload({ ...target, run_attempt: attempt }, ref, target.head_sha)
      block('pending', [target.html_url, `run ${target.id} attempt ${attempt} on ${ref} @ ${String(target.head_sha).slice(0, 12)}`, `resume: bun run gate:remote --watch ${target.id}`])
    }
    const result = await watch(gh, target.id, { deadline: end, minAttempt: attempt })
    return report(gh, result, { ref, sha: target.head_sha, branch: branchOfRef(ref), opts })
  }

  // (d) A pull request's CI is the proof, not this.
  if (named) {
    const owner = repo.slug.split('/')[0]
    const q = new URLSearchParams({ head: `${owner}:${named}`, state: 'open', per_page: '5' })
    const prs = await api(gh, 'GET', `${repoPath(gh)}/pulls?${q}`).catch((e) => couldNotRun(e.message))
    if (Array.isArray(prs) && prs.length) {
      const pr = prs[0]
      block('has-pr', [
        `#${pr.number} is open: its own CI is the proof — \`node scripts/hooks/pr-watch.mjs\` (in the background). gate:remote is for work not yet in a PR.`,
        pr.html_url,
      ])
    }
  }

  // (f) The ref and its mode.
  const full = opts.full || needsFull(branch)
  const ref = refFor(branch, full)
  if (opts.full && branch.startsWith(OFFLOAD_PREFIX) && !ref.startsWith('ci/full/')) {
    progress('plan', `${branch} is already a ci/ ref; --full cannot rename it — it runs as ${modeOfRef(ref)}`)
  }
  if (!git(['check-ref-format', '--branch', ref]).ok) couldNotRun(`${ref} is not a valid branch name`)
  const mode = modeOfRef(ref)
  setPayload(null, ref, sha)

  // (e) Local preflight: the seconds-long check, then rustfmt.
  const plan = localPlan(mode === 'full')
  if (!opts.noCheck) {
    runCheck()
    fmtPreflight(plan)
  }
  planPreview(plan, ref, sha, mode)

  // (g) Attach, or push.
  const at = remoteSha(ref)
  if (at === null) couldNotRun(`git ls-remote origin refs/heads/${ref} did not answer`)
  let runRow
  let already = false
  // The ref's review runs before this push: only a newer one is this push's.
  const reviewSnapshot = at === sha ? [] : (await reviewRuns(gh, { branch: ref }).catch(() => [])).map((r) => r.id)
  if (at === sha) {
    progress('push', `${ref} is already at ${sha.slice(0, 12)} — no push; attaching to its run`)
    const existing = pickNewRun(await offloadRuns(gh, { branch: ref, sha }).catch((e) => couldNotRun(e.message)), sha, [])
    runRow = existing ?? (await awaitNewRun(gh, ref, sha, []).catch((e) => couldNotRun(e.message)))
    already = Boolean(existing && existing.status === 'completed')
  } else {
    const snapshot = (await offloadRuns(gh, { branch: ref }).catch((e) => couldNotRun(e.message))).map((r) => r.id)
    // The pre-push hook (flow-guard) passes ci/*. Never --no-verify.
    const push = git(['push', '--force', '--quiet', 'origin', `${sha}:refs/heads/${ref}`], { timeout: 300_000 })
    if (!push.ok) couldNotRun(`git push to ${ref} failed: ${push.err || why(push)}`)
    progress('push', `${sha.slice(0, 12)} → ${ref} (${mode})`)
    // (h) The run that push started.
    runRow = await awaitNewRun(gh, ref, sha, snapshot).catch((e) => couldNotRun(e.message))
  }
  if (!runRow) {
    block('no-run', [
      `no CI offload run of ${sha.slice(0, 12)} on ${ref} within 180s — this commit has no ci-offload.yml with a \`ci/**\` push trigger — merge origin/testing or origin/rc into the branch.`,
      `The ref ${ref} is left for the daily prune; \`git push origin :refs/heads/${ref}\` removes it now.`,
    ])
  }
  setPayload(runRow, ref, sha)
  progress('run', `${runRow.name || OFFLOAD_NAME} ${runRow.html_url} (run ${runRow.id}, attempt ${runRow.run_attempt})`)

  // (i) Detach: the id is the resume.
  if (opts.detach) {
    block('pending', [runRow.html_url, `run ${runRow.id} on ${ref} @ ${sha.slice(0, 12)}`, `resume: bun run gate:remote --watch ${runRow.id}`])
  }

  // (j)–(k) Wait, and the verdict.
  const result = already
    ? { run: runRow, verdict: verdictOf(runRow, (await read(gh, `${repoPath(gh)}/actions/runs/${runRow.id}/attempts/${runRow.run_attempt}/jobs?per_page=100`, { tries: 3 }).catch((e) => couldNotRun(e.message)))?.jobs ?? []) }
    : await watch(gh, runRow.id, { deadline: deadline() })
  return report(gh, result, { ref, sha, branch, opts, already, reviewSnapshot })
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

// The pure helpers above are imported by remote-gate.test.mjs; only a direct
// run pushes. Exit codes are set, never forced: a forced exit can drop the end
// of a piped stderr (cleanup-sweep.mjs, main).
if (invokedDirectly()) {
  const opts = parseArgs(process.argv.slice(2))
  if (opts.error) {
    process.stderr.write(`remote-gate: could not run — not a pass: ${opts.error}\n\n${help()}`)
    process.exitCode = 1
  } else if (opts.help) {
    process.stdout.write(help())
  } else {
    main(opts).then(
      () => {
        process.exitCode = 1 // every path ends in an Exit; falling through is a bug, not a pass
      },
      (e) => {
        if (e instanceof Exit) process.exitCode = e.code
        else {
          process.stderr.write(`remote-gate: could not run — not a pass: ${e?.stack || e}\n`)
          process.exitCode = 1
        }
      },
    )
  }
}
