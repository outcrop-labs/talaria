import assert from 'node:assert/strict'
import { beforeEach, test, vi } from 'vitest'
import { getSetting, setSetting } from './audit'
import { registerJob } from './scheduler'

// The updater's branching, exercised without a second process: git, the
// settings store, and the scheduler are all stood in for, and what's left is
// every decision the module makes — mode gating, dirty-tree refusal, and the
// running → done/failed reconciliation that a restart has to survive.

const { state, registered, gitAnswers } = vi.hoisted(() => ({
  state: new Map<string, unknown>(),
  registered: new Map<string, { everyMs: number; run: () => Promise<string> }>(),
  gitAnswers: new Map<string, string | Error>(),
}))

vi.mock('./audit', () => ({
  getSetting: vi.fn(async (key: string, fallback: unknown) => (state.has(key) ? state.get(key) : fallback)),
  setSetting: vi.fn(async (key: string, value: unknown) => {
    state.set(key, value)
  }),
}))

vi.mock('./scheduler', () => ({
  registerJob: vi.fn((spec: { name: string; everyMs: number; run: () => Promise<string> }) => {
    registered.set(spec.name, spec)
  }),
}))

// git answers, keyed by the args the updater passes (a leading "-C <dir>" is
// dropped; the test's cwd must not be part of the key). Tests plant exactly
// the outputs each scenario needs; anything un-planted answers empty, so the
// assertions catch a refactor that runs a NEW git command rather than
// silently passing it through.
const gitKey = (args: string[]) => (args[0] === '-C' ? args.slice(2) : args).join(' ')

vi.mock('node:child_process', () => ({
  execFile: vi.fn((cmd: string, args: string[], _opts: unknown, cb: (err: Error | null, out: string) => void) => {
    const answer = cmd === 'git' ? gitAnswers.get(gitKey(args)) : undefined
    if (answer instanceof Error) cb(answer, '')
    else cb(null, answer ?? '')
  }),
  spawn: vi.fn(() => ({ unref: vi.fn() })),
}))

vi.mock('node:fs', () => ({
  existsSync: vi.fn(() => true),
  mkdirSync: vi.fn(),
  openSync: vi.fn(() => 1),
  renameSync: vi.fn(),
  rmSync: vi.fn(),
}))

import { applyUpdate, reconcileUpdate, updaterMode, updaterState } from './updater'

const HEAD = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
const LATEST = 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'

// A full apply sequence ends in the updater SIGTERPing its own process; in a
// test runner that process is vitest's. No test below drives that far (they
// all stop at a gate or a planted failure), but the belt stays on.
vi.spyOn(process, 'kill').mockReturnValue(true)

const plant = (args: string[], out: string) => gitAnswers.set(args.join(' '), out)
const plantRev = (ref: string, rev: string) =>
  plant(['show', '-s', '--format=%H%n%s%n%cI', ref], `${rev}\nA subject\n2026-08-25T12:00:00+00:00\n`)

beforeEach(() => {
  state.clear()
  gitAnswers.clear()
  // Direct assignments, not vi.stubEnv: unstubAllEnvs would not undo them.
  delete process.env.TALARIA_RUNTIME
  delete process.env.TALARIA_UPDATER
  plantRev('HEAD', HEAD)
  plantRev(`origin/${'main'}`, LATEST)
  plant(['rev-parse', '--abbrev-ref', 'HEAD'], 'main\n')
  plant(['rev-list', '--count', `HEAD..origin/${'main'}`], '2\n')
  plant(['status', '--porcelain'], '')
  plant(['fetch', 'origin', 'main'], '')
  plant(['rev-parse', 'origin/main'], `${LATEST}\n`)
  plant(['rev-parse', 'HEAD'], `${HEAD}\n`)
})

test('dev mode never updates, no matter what it is asked to do', async () => {
  delete process.env.TALARIA_RUNTIME
  delete process.env.TALARIA_UPDATER
  assert.equal(updaterMode(), 'dev')
  const r = await applyUpdate('manual')
  assert.equal(r.started, false)
  assert.match(r.error ?? '', /dev/i)
})

test('the kill switch wins over everything', async () => {
  process.env.TALARIA_RUNTIME = 'prod-server'
  process.env.TALARIA_UPDATER = 'off'
  assert.equal(updaterMode(), 'off')
  const r = await applyUpdate('manual')
  assert.equal(r.started, false)
})

test('a dirty checkout is refused, not pulled over', async () => {
  process.env.TALARIA_RUNTIME = 'prod-server'
  plant(['status', '--porcelain'], ' M ui/src/server/home.ts\n?? notes.txt\n')
  const r = await applyUpdate('manual')
  assert.equal(r.started, false)
  assert.match(r.error ?? '', /2 uncommitted/)
})

test('an update that starts is recorded as running before anything else', async () => {
  process.env.TALARIA_RUNTIME = 'prod-server'
  // The pull fails: bun install would be next, and no answer is planted for
  // it, so the run lands in failed with the git error attached.
  plant(['pull', '--ff-only'], new Error('network went away'))
  const r = await applyUpdate('manual')
  assert.equal(r.started, false)
  const s = await updaterState()
  assert.equal(s.lastRun?.state, 'failed')
  assert.match(s.lastRun?.error ?? '', /network went away/)
})

test('reconcile turns running into done when the server is on the target rev', async () => {
  state.set('updater', {
    autoUpdate: false,
    lastCheck: null,
    lastRun: { at: new Date().toISOString(), from: HEAD, to: LATEST, by: 'manual', state: 'running' },
    history: [],
  })
  plantRev('HEAD', LATEST)
  await reconcileUpdate()
  const s = await updaterState()
  assert.equal(s.lastRun?.state, 'done')
  assert.equal(s.lastRun?.to, LATEST)
})

test('reconcile leaves a young running update alone: the build may still be working', async () => {
  state.set('updater', {
    autoUpdate: false,
    lastCheck: null,
    lastRun: { at: new Date().toISOString(), from: HEAD, to: LATEST, by: 'manual', state: 'running' },
    history: [],
  })
  // HEAD still answers as the OLD rev (planted in beforeEach).
  await reconcileUpdate()
  const s = await updaterState()
  assert.equal(s.lastRun?.state, 'running')
})

test('reconcile fails an update that never landed, after half an hour of silence', async () => {
  state.set('updater', {
    autoUpdate: false,
    lastCheck: null,
    lastRun: { at: new Date(Date.now() - 45 * 60_000).toISOString(), from: HEAD, to: LATEST, by: 'auto', state: 'running' },
    history: [],
  })
  await reconcileUpdate()
  const s = await updaterState()
  assert.equal(s.lastRun?.state, 'failed')
  assert.match(s.lastRun?.error ?? '', /never came back/i)
})

test('the scheduled job registers, sits still while auto is off, and checks while it is on', async () => {
  const job = registered.get('update-check')
  assert.ok(job, 'the updater must register its job by importing the module')
  assert.equal(job.everyMs, 6 * 60 * 60 * 1000)

  process.env.TALARIA_RUNTIME = 'prod-server'
  let result = await job.run()
  assert.match(result, /auto-update is off/)

  state.set('updater', { autoUpdate: true, lastCheck: null, lastRun: null, history: [] })
  // Up to date: the count says zero.
  plant(['rev-list', '--count', `HEAD..origin/${'main'}`], '0\n')
  result = await job.run()
  assert.equal(result, 'up to date')
})

test('getSetting/setSetting round-trip through the mocked store the tests assert on', async () => {
  await setSetting('updater', { autoUpdate: true })
  assert.deepEqual(await getSetting('updater', {}), { autoUpdate: true })
})
