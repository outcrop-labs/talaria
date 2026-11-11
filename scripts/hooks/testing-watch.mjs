#!/usr/bin/env node
// testing-watch — is my merge live on the testing instance yet?
//
// WHAT IT ANSWERS. After a pull request merges into `testing`, three things
//   happen without anyone: testing-deploy.yml builds and smoke-tests the
//   commit and moves `ghcr.io/outcrop-labs/talaria:testing` to it, the VM's
//   follow script pulls it (after its minimum gap), and the instance restarts
//   on it. This polls the instance's /api/healthz until the version it reports
//   — `testing-<sha12>` — is a commit that CONTAINS the merge, and says so.
//   Only then is "exercise it on testing" a thing an agent can do
//   (docs/TESTING-INSTANCE.md; ship-a-change says when to run it).
//
// USAGE
//   node scripts/hooks/testing-watch.mjs <pr-number|sha> [--timeout <min>] [--json]
//     <pr-number>  a merged pull request (`123` or `#123`): its merge commit
//     <sha>        any commit on testing, 7 to 40 hex characters
//     --timeout    minutes to wait (default 45)
//     --json       the verdict as one JSON object on stdout, whatever it is
//   Run it in the harness's background mode: it waits minutes, not seconds.
//
// THE CONTRACT (scripts/hooks/README.md — the same one every gate here speaks):
//   exit 0   live: one proof line on stdout,
//            `testing-watch: live — <url> runs testing-<sha12>, which contains <target12>`.
//   exit 2   not live, and waiting will not change that; the FIRST stderr line
//            is the token:
//              testing-watch: unmerged     the pull request is not merged
//              testing-watch: red-deploy   a testing-deploy run that carries
//                                          the commit failed, and nothing
//                                          newer is building
//              testing-watch: down         /api/healthz has failed for more
//                                          than 5 minutes running
//              testing-watch: timeout      none of the above, and --timeout ran out
//   other    could not run — no token, no URL, a bad argument. Never a pass.
//
// WHERE THE INSTANCE IS. TALARIA_TESTING_URL, else the repository variable
//   TESTING_URL (what testing-deploy.yml's verify job reads). Cloudflare
//   Access headers ride along when CF_ACCESS_CLIENT_ID and
//   CF_ACCESS_CLIENT_SECRET are both set.
//
// THE TOKEN. GH_TOKEN, else GITHUB_TOKEN, else `gh auth token`: a
//   fine-grained token for this repository with Actions read, Contents read,
//   Pull requests read and Variables read (the last only when
//   TALARIA_TESTING_URL is not set). The same rule as remote-gate.mjs; the
//   copy here keeps this file runnable on a branch that has not got it yet.
//
// "CONTAINS", NOT "EQUALS". Merges into testing come minutes apart and the
//   follower applies at most once per gap, so the instance often skips
//   straight past a commit to a later one. A later commit that has the merge
//   in its history IS the merge, live. The check is local git — `git fetch
//   origin testing`, then `git merge-base --is-ancestor` — so run it from a
//   clone of the repository.
//
// AS A MODULE. parseArgs, versionSha, verdict, nextDelay and tokenFrom are
//   pure and exported for testing-watch.test.mjs; the polling runs only when
//   this file is invoked directly (pr-watch.mjs's realpath rule).

import { spawnSync } from 'node:child_process'
import { realpathSync } from 'node:fs'
import { resolve } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { parseRepo } from './pr-watch.mjs'

const MINUTE = 60_000
const DOWN_AFTER_MS = 5 * MINUTE
const FAILED = new Set(['failure', 'timed_out', 'startup_failure'])

/** argv (without node and the script) → {target, kind, timeoutMin, json} or {error}. */
export function parseArgs(argv) {
  const out = { target: '', kind: '', timeoutMin: 45, json: false }
  const positional = []
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a === '--json') out.json = true
    else if (a === '--timeout') {
      const raw = argv[++i]
      const n = Number(raw)
      if (raw === undefined || !Number.isFinite(n) || n <= 0) return { error: `--timeout wants a number of minutes, got ${raw ?? 'nothing'}` }
      out.timeoutMin = n
    } else if (a.startsWith('-')) return { error: `unknown flag ${a}` }
    else positional.push(a)
  }
  if (positional.length !== 1) return { error: 'one argument: a pull request number or a commit sha' }
  const arg = positional[0].replace(/^#/, '')
  // A short number is a pull request; anything 7+ hex long is a commit (a
  // seven-digit pull request number is a long way off).
  if (/^\d{1,6}$/.test(arg)) {
    out.kind = 'pr'
    out.target = arg
  } else if (/^[0-9a-f]{7,40}$/i.test(arg)) {
    out.kind = 'sha'
    out.target = arg.toLowerCase()
  } else {
    return { error: `not a pull request number or a commit sha: ${positional[0]}` }
  }
  return out
}

/** The commit a healthz version names: `testing-<sha12>` → the sha12, else null. */
export function versionSha(version) {
  const m = /^testing-([0-9a-f]{12})$/.exec(String(version ?? ''))
  return m ? m[1] : null
}

/** The wait before the next poll: 20 s, doubling, at most 60 s. */
export function nextDelay(prevSec) {
  if (!prevSec) return 20
  return Math.min(prevSec * 2, 60)
}

/** GH_TOKEN, else GITHUB_TOKEN, else what `gh auth token` prints, else ''. */
export function tokenFrom(env, ghTokenFn = () => '') {
  for (const key of ['GH_TOKEN', 'GITHUB_TOKEN']) {
    const v = (env[key] ?? '').trim()
    if (v) return v
  }
  return String(ghTokenFn() ?? '').trim()
}

/**
 * One poll's facts → the verdict. Pure.
 *   health          {ok, version, status} from /api/healthz, or null when it
 *                   did not answer (or answered non-2xx).
 *   runningContains the running commit has the target in its history; null
 *                   when the version names no commit this clone can resolve.
 *   runs            testing-deploy runs, newest first:
 *                   {status, conclusion, contains, url} — `contains`: the
 *                   run's head has the target in its history.
 *   downForMs       how long healthz has failed in a row (0 when it answered).
 *   elapsedMs, timeoutMs
 * → {kind: 'live' | 'red-deploy' | 'down' | 'timeout' | 'wait', ...}
 */
export function verdict({ health, runningContains, runs = [], downForMs = 0, elapsedMs = 0, timeoutMs = 45 * MINUTE }) {
  const healthy = Boolean(health && health.ok && health.status === 'ok')
  if (healthy && runningContains === true) return { kind: 'live', version: health.version }

  // The deploy: the newest run that carries the target and has finished
  // (a cancelled run is no verdict — the queue replaced it). Red only when
  // nothing newer that carries it is still queued or building.
  const carrying = runs.filter((r) => r.contains)
  const at = carrying.findIndex((r) => r.status === 'completed' && r.conclusion !== 'cancelled')
  const finished = at === -1 ? null : carrying[at]
  const newerBuilding = carrying.slice(0, at === -1 ? carrying.length : at).some((r) => r.status !== 'completed')
  if (finished && FAILED.has(finished.conclusion) && !newerBuilding) return { kind: 'red-deploy', url: finished.url }

  if (downForMs > DOWN_AFTER_MS) return { kind: 'down' }
  if (elapsedMs >= timeoutMs) return { kind: 'timeout' }
  return { kind: 'wait', deploy: newerBuilding ? 'building' : finished ? finished.conclusion : 'no run yet' }
}

// ── the CLI ──────────────────────────────────────────────────────────────────

function couldNotRun(why) {
  console.error(`testing-watch: could not run — not a pass: ${why}`)
  process.exit(1)
}

function git(args) {
  const r = spawnSync('git', args, { encoding: 'utf8', timeout: 120_000 })
  return { ok: r.status === 0, out: (r.stdout || '').trim(), err: (r.stderr || '').trim() }
}

function ghToken() {
  const r = spawnSync('gh', ['auth', 'token'], {
    encoding: 'utf8',
    timeout: 10_000,
    env: { ...process.env, GH_PROMPT_DISABLED: '1' },
  })
  return r.status === 0 ? r.stdout : ''
}

const isAncestor = (a, b) => git(['merge-base', '--is-ancestor', a, b]).ok

async function main(opts) {
  const token = tokenFrom(process.env, ghToken)
  if (!token) {
    couldNotRun(
      'set GH_TOKEN — fine-grained, outcrop-labs/talaria: Actions read, Contents read, Pull requests read, Variables read, Metadata read — or `gh auth login`; in a devbox pass it with `talaria box new <name> --env GH_TOKEN=…`',
    )
  }
  const remote = git(['remote', 'get-url', 'origin'])
  const repo = remote.ok ? parseRepo(remote.out) : null
  if (!repo) couldNotRun('run it from a clone of the repository: `git remote get-url origin` names no GitHub repository')
  const apiBase = repo.host === 'github.com' ? 'https://api.github.com' : `https://${repo.host}/api/v3`

  const api = async (path) => {
    const res = await fetch(`${apiBase}/repos/${repo.slug}/${path}`, {
      headers: { Authorization: `Bearer ${token}`, Accept: 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28' },
      signal: AbortSignal.timeout(20_000),
    })
    if (!res.ok) {
      const err = new Error(`GET ${path}: HTTP ${res.status}`)
      err.status = res.status
      throw err
    }
    return res.json()
  }

  let url = (process.env.TALARIA_TESTING_URL ?? '').trim()
  if (!url) {
    try {
      url = String((await api('actions/variables/TESTING_URL')).value ?? '').trim()
    } catch (e) {
      if (e.status !== 404) couldNotRun(`reading the TESTING_URL variable: ${e.message} (or set TALARIA_TESTING_URL)`)
    }
  }
  if (!url) couldNotRun('no testing instance: set TALARIA_TESTING_URL, or the repository variable TESTING_URL (docs/TESTING-INSTANCE.md)')
  url = url.replace(/\/+$/, '')

  const out = (result, code) => {
    if (opts.json) console.log(JSON.stringify(result))
    process.exit(code)
  }
  const blocked = (token_, lines, extra = {}) => {
    console.error([`testing-watch: ${token_}`, ...lines].join('\n'))
    out({ token: token_, url, ...extra }, 2)
  }

  // The target commit.
  let target = opts.target
  let label = target
  if (opts.kind === 'pr') {
    let pr
    try {
      pr = await api(`pulls/${opts.target}`)
    } catch (e) {
      couldNotRun(`reading pull request #${opts.target}: ${e.message}`)
    }
    if (!pr.merged) {
      blocked('unmerged', [`#${opts.target} is ${pr.state === 'open' ? 'open' : 'closed without merging'}: nothing of it can be live.`, pr.html_url])
    }
    target = pr.merge_commit_sha
    label = `#${opts.target}`
    if (pr.base?.ref !== 'testing') {
      console.error(`testing-watch: note — #${opts.target} merged into ${pr.base?.ref}, not testing; it reaches the instance only once testing carries it (a back-flow)`)
    }
  }
  if (!git(['fetch', '--quiet', 'origin', 'testing']).ok) couldNotRun('`git fetch origin testing` failed')
  const full = git(['rev-parse', '--verify', '--quiet', `${target}^{commit}`])
  if (!full.ok) couldNotRun(`${target} is not a commit this clone has, even after fetching testing`)
  target = full.out
  const t12 = target.slice(0, 12)

  const cf = {}
  if (process.env.CF_ACCESS_CLIENT_ID && process.env.CF_ACCESS_CLIENT_SECRET) {
    cf['CF-Access-Client-Id'] = process.env.CF_ACCESS_CLIENT_ID
    cf['CF-Access-Client-Secret'] = process.env.CF_ACCESS_CLIENT_SECRET
  }

  const started = Date.now()
  const timeoutMs = opts.timeoutMin * MINUTE
  let downSince = 0
  let delay = 0
  let runsError = ''
  for (;;) {
    git(['fetch', '--quiet', 'origin', 'testing'])

    let health = null
    try {
      const res = await fetch(`${url}/api/healthz`, { headers: cf, signal: AbortSignal.timeout(10_000) })
      const body = await res.json().catch(() => ({}))
      health = { ok: res.ok, status: body.status ?? null, version: body.version ?? null }
    } catch {
      health = null
    }
    const healthy = Boolean(health && health.ok && health.status === 'ok')
    if (healthy) downSince = 0
    else if (!downSince) downSince = Date.now()

    const sha12 = versionSha(health?.version)
    let runningContains = null
    if (sha12) {
      const running = git(['rev-parse', '--verify', '--quiet', `${sha12}^{commit}`])
      runningContains = running.ok ? isAncestor(target, running.out) : null
    }

    let runs = []
    try {
      const page = await api('actions/workflows/testing-deploy.yml/runs?branch=testing&per_page=10')
      runs = (page.workflow_runs ?? []).map((r) => ({
        status: r.status,
        conclusion: r.conclusion,
        url: r.html_url,
        contains: isAncestor(target, r.head_sha),
      }))
    } catch (e) {
      // Once per distinct error: a missing workflow (before it reaches main)
      // or a token without Actions read says the same thing every poll.
      if (e.message !== runsError) console.error(`testing-watch: could not read testing-deploy runs (${e.message}); watching the instance only`)
      runsError = e.message
    }

    const v = verdict({
      health,
      runningContains,
      runs,
      downForMs: downSince ? Date.now() - downSince : 0,
      elapsedMs: Date.now() - started,
      timeoutMs,
    })
    const extra = { version: health?.version ?? null, target }
    if (v.kind === 'live') {
      const line = `testing-watch: live — ${url} runs ${health.version}, which contains ${t12} (${label})`
      if (opts.json) out({ token: 'live', url, ...extra }, 0)
      console.log(line)
      process.exit(0)
    }
    if (v.kind === 'red-deploy') {
      blocked('red-deploy', [`a testing-deploy run carrying ${t12} (${label}) failed, and nothing newer is building:`, v.url,
        'If the failure is the commit, fix it on testing by a pull request; if it is infrastructure (a runner, the registry), re-run that run.'], { run_url: v.url, ...extra })
    }
    if (v.kind === 'down') {
      blocked('down', [`${url}/api/healthz has not answered ok for more than 5 minutes.`,
        'On the VM: `journalctl -u talaria-follow -n 100` and `docker ps`; docs/TESTING-INSTANCE.md, Troubleshooting.'], extra)
    }
    if (v.kind === 'timeout') {
      blocked('timeout', [`${url} still runs ${health?.version ?? '(no answer)'} after ${opts.timeoutMin} min, which does not contain ${t12} (${label}).`,
        'The follower waits out its minimum gap and honours a hold file (~/.local/state/talaria-follow/hold); testing-deploy may still be building.'], extra)
    }
    delay = nextDelay(delay)
    const left = Math.max(0, Math.round((timeoutMs - (Date.now() - started)) / 1000))
    const wait = Math.max(1, Math.min(delay, left))
    console.error(`testing-watch: waiting — ${url} runs ${health?.version ?? '(no answer)'}; testing-deploy for ${t12}: ${v.deploy}; next poll in ${wait}s, ${left}s left`)
    await sleep(wait * 1000)
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

if (invokedDirectly()) {
  const opts = parseArgs(process.argv.slice(2))
  if (opts.error) {
    console.error('usage: node scripts/hooks/testing-watch.mjs <pr-number|sha> [--timeout <min>] [--json]')
    couldNotRun(opts.error)
  }
  main(opts).catch((err) => couldNotRun(err.message))
}
