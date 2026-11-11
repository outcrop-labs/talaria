// testing-watch's verdict: when a merge is live on the testing instance, and
// when waiting will not make it so. A wrong "live" sends an agent to exercise
// code the instance is not running; a wrong red stops it while the follower is
// only waiting out its gap. Pure cases, no network. Run by `bun run check`.
import { spawnSync } from 'node:child_process'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { nextDelay, parseArgs, tokenFrom, verdict, versionSha } from './testing-watch.mjs'

const HERE = dirname(fileURLToPath(import.meta.url))
const MIN = 60_000
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

const OK = { ok: true, status: 'ok', version: 'testing-0123456789ab' }
const run = (status, conclusion, contains, url = `https://github.com/o/r/actions/runs/${Math.random()}`) => ({ status, conclusion, contains, url })

// ── parseArgs ────────────────────────────────────────────────────────────────

check('parseArgs: a pull request number, bare or with #', () => {
  eq(parseArgs(['723']), { target: '723', kind: 'pr', timeoutMin: 45, json: false })
  eq(parseArgs(['#723', '--json']).kind, 'pr')
  eq(parseArgs(['#723', '--json']).json, true)
})

check('parseArgs: a commit, short or full, lowercased', () => {
  eq(parseArgs(['6F037DFE']).target, '6f037dfe')
  eq(parseArgs(['6f037dfe36a3d806215e5c0e8e2ca0f5edd9ef8b']).kind, 'sha')
  // seven digits is a short sha, not a pull request
  eq(parseArgs(['1234567']).kind, 'sha')
})

check('parseArgs: --timeout takes minutes', () => {
  eq(parseArgs(['12', '--timeout', '30']).timeoutMin, 30)
  eq(typeof parseArgs(['12', '--timeout']).error, 'string')
  eq(typeof parseArgs(['12', '--timeout', 'soon']).error, 'string')
  eq(typeof parseArgs(['12', '--timeout', '0']).error, 'string')
})

check('parseArgs: refuses no argument, two, an unknown flag, and junk', () => {
  for (const argv of [[], ['1', '2'], ['12', '--wat'], ['main'], ['xyz1234']]) {
    eq(typeof parseArgs(argv).error, 'string')
  }
})

// ── versionSha ───────────────────────────────────────────────────────────────

check('versionSha: testing-<sha12> and nothing else', () => {
  eq(versionSha('testing-0123456789ab'), '0123456789ab')
  eq(versionSha('0123456789ab'), null) // rc-deploy's bare sha is not a testing build
  eq(versionSha('testing-0123456789a'), null)
  eq(versionSha('testing-0123456789abc'), null)
  eq(versionSha('0.13.1'), null)
  eq(versionSha(null), null)
  eq(versionSha(undefined), null)
})

// ── nextDelay ────────────────────────────────────────────────────────────────

check('nextDelay: 20 s, doubling, capped at 60', () => {
  eq([0, 20, 40, 60, 60].map(nextDelay), [20, 40, 60, 60, 60])
})

// ── tokenFrom ────────────────────────────────────────────────────────────────

check('tokenFrom: GH_TOKEN, then GITHUB_TOKEN, then gh, then nothing', () => {
  const gh = () => 'from-gh\n'
  eq(tokenFrom({ GH_TOKEN: 'a', GITHUB_TOKEN: 'b' }, gh), 'a')
  eq(tokenFrom({ GH_TOKEN: ' ', GITHUB_TOKEN: 'b' }, gh), 'b')
  eq(tokenFrom({}, gh), 'from-gh')
  eq(tokenFrom({}, () => ''), '')
  eq(tokenFrom({}), '')
})

// ── verdict ──────────────────────────────────────────────────────────────────

check('verdict: live when healthy and the running commit contains the target', () => {
  eq(verdict({ health: OK, runningContains: true }), { kind: 'live', version: 'testing-0123456789ab' })
  // a red deploy of a LATER commit does not unmake a live one
  eq(verdict({ health: OK, runningContains: true, runs: [run('completed', 'failure', true)] }).kind, 'live')
})

check('verdict: not live while the instance is unhealthy, even on the right commit', () => {
  eq(verdict({ health: { ...OK, status: 'degraded' }, runningContains: true }).kind, 'wait')
  eq(verdict({ health: { ...OK, ok: false }, runningContains: true }).kind, 'wait')
})

check('verdict: waits while the instance runs an older commit and the deploy builds', () => {
  const v = verdict({ health: OK, runningContains: false, runs: [run('in_progress', null, true), run('completed', 'success', false)] })
  eq(v, { kind: 'wait', deploy: 'building' })
  eq(verdict({ health: OK, runningContains: false, runs: [] }), { kind: 'wait', deploy: 'no run yet' })
  // a version this clone cannot resolve is not "contains"
  eq(verdict({ health: OK, runningContains: null, runs: [run('completed', 'success', true)] }), { kind: 'wait', deploy: 'success' })
})

check('verdict: red-deploy when the newest finished run carrying it failed and nothing newer builds', () => {
  const red = run('completed', 'failure', true, 'https://x/runs/9')
  eq(verdict({ health: OK, runningContains: false, runs: [red, run('completed', 'success', false)] }), { kind: 'red-deploy', url: 'https://x/runs/9' })
  eq(verdict({ health: OK, runningContains: false, runs: [run('completed', 'timed_out', true, 'u')] }).kind, 'red-deploy')
  eq(verdict({ health: OK, runningContains: false, runs: [run('completed', 'startup_failure', true, 'u')] }).kind, 'red-deploy')
})

check('verdict: no red-deploy while a newer run carrying it builds, or after a newer one succeeded', () => {
  const red = run('completed', 'failure', true)
  eq(verdict({ health: OK, runningContains: false, runs: [run('queued', null, true), red] }).kind, 'wait')
  eq(verdict({ health: OK, runningContains: false, runs: [run('completed', 'success', true), red] }).kind, 'wait')
})

check('verdict: a cancelled run is no verdict, and a failed run before the merge is not the merge', () => {
  // the queue replaced it: the next finished run decides
  eq(verdict({ health: OK, runningContains: false, runs: [run('completed', 'cancelled', true), run('completed', 'success', true)] }).kind, 'wait')
  // a run whose head predates the target says nothing about it
  eq(verdict({ health: OK, runningContains: false, runs: [run('completed', 'failure', false)] }).kind, 'wait')
})

check('verdict: down after more than 5 minutes of failed healthz, not before', () => {
  eq(verdict({ health: null, runningContains: null, downForMs: 5 * MIN + 1 }).kind, 'down')
  eq(verdict({ health: null, runningContains: null, downForMs: 5 * MIN }).kind, 'wait')
  // a red deploy outranks down: it is the reason to look first
  eq(verdict({ health: null, runningContains: null, downForMs: 10 * MIN, runs: [run('completed', 'failure', true, 'u')] }).kind, 'red-deploy')
})

check('verdict: timeout once the clock runs out with nothing else to say', () => {
  eq(verdict({ health: OK, runningContains: false, elapsedMs: 45 * MIN, timeoutMs: 45 * MIN }).kind, 'timeout')
  eq(verdict({ health: OK, runningContains: false, elapsedMs: 44 * MIN, timeoutMs: 45 * MIN }).kind, 'wait')
  // live at the wire is still live
  eq(verdict({ health: OK, runningContains: true, elapsedMs: 50 * MIN, timeoutMs: 45 * MIN }).kind, 'live')
})

// ── the CLI ──────────────────────────────────────────────────────────────────

check('importing does not poll; a direct run with a bad argument could not run (exit 1, not 2)', () => {
  const r = spawnSync(process.execPath, [join(HERE, 'testing-watch.mjs'), 'not-a-target'], { encoding: 'utf8' })
  eq(r.status, 1)
  if (!/could not run/.test(r.stderr)) throw new Error(r.stderr)
})

if (failed) {
  console.error(`testing-watch: ${failed} of ${ran} failed`)
  process.exit(1)
}
console.log(`testing-watch: ${ran} checks, all clean`)
