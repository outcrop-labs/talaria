// In-app updates: the server pulls its own repo, rebuilds, and restarts.
//
// WHY THIS SHAPE. Production is a bare `bun server-entry.js` with no
// supervisor (setup.sh installs, nothing watches), so "update the app" used
// to mean SSH in and pull. The updater moves that whole ritual behind an
// admin button: fetch + ff-only pull, `bun install`, build into a SIBLING
// directory and swap, then a detached helper boots the new server the moment
// the old one lets go of the port.
//
// MANUAL BY DEFAULT. Nothing updates itself until an admin turns the
// auto-update toggle on; the scheduled job checks the switch every run and
// sits still when it is off.
//
// THE RESTART SEQUENCE, end to end:
//   1. applyUpdate() records lastRun.state='running' BEFORE touching
//      anything, so a crash at any later step leaves an honest trail.
//   2. pull + install + build into ui/dist-next/{client,server}. The running
//      server keeps serving its old dist the whole time: building straight
//      into dist/ would serve a mid-build index.html that references asset
//      files that don't exist yet.
//   3. swap: dist -> dist-prev, dist-next -> dist. Two renames on the same
//      filesystem; the gap between them is milliseconds against the minutes
//      of exposure a live build would have.
//   4. spawn scripts/update-restart.mjs detached, then SIGTERM ourselves.
//      server-entry's SIGTERM path drains the scheduler jobs gracefully;
//      reusing it means an update restarts exactly the way a well-behaved
//      operator would restart it.
//   5. the helper waits for the port to free, runs `bun server-entry.js`
//      with our inherited environment, waits for it to answer, and exits.
//      The new server's first updater read reconciles 'running' into 'done'
//      (its HEAD now matches lastRun.to).
//
// If step 5 never lands, nothing marks the run done: the panel shows an
// update that started and never finished, with logs/talaria.log as the
// trail. That is deliberate. A stuck updater must be VISIBLE, not folded
// into a success state it did not reach.
//
// DEV IS NEVER UPDATED. Dev runs under vite, which reloads on file change;
// git-pulling from under a dev session would be chaos for no gain. The
// panel says so instead of hiding the button.
//
// TALARIA_UPDATER=off is the kill switch (same convention as
// TALARIA_SCHEDULER=off) for deployments that supervise the process
// themselves and own restarts their own way.
import { execFile, spawn } from 'node:child_process'
import { existsSync, mkdirSync, openSync, renameSync, rmSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { getSetting, setSetting } from './audit'
import { registerJob } from './scheduler'

type ExecResult = { stdout: string; stderr: string }

/** Run one command, capturing output. Args never touch a shell. */
function exec(cmd: string, args: string[], opts: { cwd?: string; timeoutMs?: number } = {}): Promise<ExecResult> {
  return new Promise((resolve, reject) => {
    execFile(cmd, args, { cwd: opts.cwd, timeout: opts.timeoutMs ?? 30_000, maxBuffer: 8 * 1024 * 1024 }, (err, stdout, stderr) => {
      if (err) reject(err instanceof Error ? err : new Error(String(err)))
      else resolve({ stdout: String(stdout), stderr: String(stderr) })
    })
  })
}

const errText = (e: unknown): string => (e instanceof Error ? e.message : String(e))

// ── Where am I? ──────────────────────────────────────────────────────────────

let cachedRoot: string | null | undefined

/** The repo root this server runs from, found by walking up from this file
 *  to the first .git. Null when the app is not running from a checkout
 *  (bundled standalone, or something exotic), which turns the updater off. */
export function repoRoot(): string | null {
  if (cachedRoot === undefined) {
    let dir = dirname(fileURLToPath(import.meta.url))
    for (let i = 0; i < 10; i++) {
      if (existsSync(join(dir, '.git'))) break
      const parent = dirname(dir)
      if (parent === dir) {
        dir = '/'
        break
      }
      dir = parent
    }
    cachedRoot = existsSync(join(dir, '.git')) ? dir : null
  }
  return cachedRoot
}

export type UpdaterMode = 'server' | 'dev' | 'off'

/** Whether in-app updates can run here, and if not, why not. */
export function updaterMode(): UpdaterMode {
  if (process.env.TALARIA_UPDATER === 'off') return 'off'
  // server-entry.js stamps this before importing the app graph; vite dev
  // never does. It is the one honest signal for "a server install".
  if (process.env.TALARIA_RUNTIME !== 'prod-server') return 'dev'
  return repoRoot() ? 'server' : 'off'
}

// ── Version state ────────────────────────────────────────────────────────────

export interface RevInfo {
  /** Full commit hash. */
  rev: string
  /** First 7 chars, what a human reads. */
  short: string
  /** Commit subject line. */
  subject: string
  /** Commit date, ISO. */
  at: string | null
}

async function revInfo(root: string, ref: string): Promise<RevInfo> {
  const out = await exec('git', ['-C', root, 'show', '-s', '--format=%H%n%s%n%cI', ref])
  const [rev = '', subject = '', at] = out.stdout.split('\n')
  return { rev: rev.trim(), short: rev.trim().slice(0, 7), subject: subject.trim(), at: at?.trim() || null }
}

/** The running commit, without touching the network. Null when git is
 *  unavailable or this is not a checkout. */
export async function currentRev(): Promise<RevInfo | null> {
  const root = repoRoot()
  if (!root) return null
  try {
    return await revInfo(root, 'HEAD')
  } catch {
    return null
  }
}

async function currentBranch(root: string): Promise<string> {
  const out = await exec('git', ['-C', root, 'rev-parse', '--abbrev-ref', 'HEAD'])
  const branch = out.stdout.trim()
  if (!branch || branch === 'HEAD') throw new Error('This checkout is on a detached HEAD; the updater needs a branch.')
  return branch
}

// ── Persisted state ──────────────────────────────────────────────────────────

export interface UpdateRun {
  at: string
  from: string
  to: string
  by: 'manual' | 'auto'
  state: 'running' | 'done' | 'failed'
  error?: string | null
}

export interface UpdaterState {
  autoUpdate: boolean
  lastCheck: { at: string; behind: number; current: RevInfo; latest: RevInfo } | null
  lastRun: UpdateRun | null
  history: UpdateRun[]
}

const DEFAULT_STATE: UpdaterState = { autoUpdate: false, lastCheck: null, lastRun: null, history: [] }

export async function updaterState(): Promise<UpdaterState> {
  const stored = await getSetting<Partial<UpdaterState>>('updater', {})
  return { ...DEFAULT_STATE, ...stored }
}

async function patchState(patch: (s: UpdaterState) => UpdaterState): Promise<UpdaterState> {
  const next = patch(await updaterState())
  await setSetting('updater', next)
  return next
}

// ── Checking ─────────────────────────────────────────────────────────────────

export interface CheckResult {
  behind: number
  current: RevInfo
  latest: RevInfo
  branch: string
}

/** Fetch origin and compare. Throws with a human message when git, the
 *  network, or the checkout shape says no. */
export async function checkForUpdate(): Promise<CheckResult> {
  const root = repoRoot()
  if (!root) throw new Error('This install is not running from a git checkout.')
  const branch = await currentBranch(root)
  try {
    await exec('git', ['-C', root, 'fetch', 'origin', branch], { timeoutMs: 60_000 })
  } catch (e) {
    throw new Error(`Could not reach the remote: ${errText(e)}`)
  }
  const current = await revInfo(root, 'HEAD')
  const latest = await revInfo(root, `origin/${branch}`)
  const countOut = await exec('git', ['-C', root, 'rev-list', '--count', `HEAD..origin/${branch}`])
  const behind = parseInt(countOut.stdout.trim(), 10) || 0
  await patchState((s) => ({ ...s, lastCheck: { at: new Date().toISOString(), behind, current, latest } }))
  return { behind, current, latest, branch }
}

// ── Applying ─────────────────────────────────────────────────────────────────

let applying = false

/** One update attempt. Resolves fast: either {started: false, error} (gate
 *  refused it) or {started: true}, after which the process exits on its own
 *  SIGTERM path and a detached helper brings the new server up. */
export async function applyUpdate(by: 'manual' | 'auto'): Promise<{ started: boolean; error?: string }> {
  if (applying) return { started: false, error: 'An update is already running.' }
  const mode = updaterMode()
  if (mode !== 'server') {
    return {
      started: false,
      error: mode === 'dev' ? 'Dev picks up code changes on its own; updates are for server installs.' : 'Updates are switched off on this install.',
    }
  }
  const root = repoRoot()!
  applying = true
  const fail = async (error: string) => {
    await patchState((s) => {
      if (!s.lastRun || s.lastRun.state !== 'running') return s
      const failed: UpdateRun = { ...s.lastRun, state: 'failed', error }
      return { ...s, lastRun: failed, history: [failed, ...s.history].slice(0, 10) }
    })
    applying = false
    return { started: false, error }
  }

  try {
    // A dirty tree means someone is working on the server, or a past update
    // half-applied; a pull over it would mix their edits with ours. Refuse.
    const status = await exec('git', ['-C', root, 'status', '--porcelain'])
    const dirty = status.stdout.split('\n').filter((l) => l.trim()).length
    if (dirty > 0) return fail(`The checkout on the server has ${dirty} uncommitted change(s). Clean it up first.`)

    const branch = await currentBranch(root)
    await exec('git', ['-C', root, 'fetch', 'origin', branch], { timeoutMs: 60_000 })
    const to = (await exec('git', ['-C', root, 'rev-parse', `origin/${branch}`])).stdout.trim()
    const from = (await exec('git', ['-C', root, 'rev-parse', 'HEAD'])).stdout.trim()
    if (to === from) return fail('Already up to date.')

    const run: UpdateRun = { at: new Date().toISOString(), from, to, by, state: 'running' }
    await patchState((s) => ({ ...s, lastRun: run, history: [run, ...s.history].slice(0, 10) }))

    await exec('git', ['-C', root, 'pull', '--ff-only'], { timeoutMs: 120_000 })

    const uiDir = join(root, 'ui')
    await exec('bun', ['install'], { cwd: uiDir, timeoutMs: 300_000 })
    // The MCP package runs its TypeScript entry directly under bun; it owns
    // its own node_modules and its deps can change between releases too.
    if (existsSync(join(root, 'mcp', 'package.json'))) {
      await exec('bun', ['install'], { cwd: join(root, 'mcp'), timeoutMs: 300_000 })
    }

    // Build into dist-next and swap (see the header): the running server
    // serves dist/ by request, and never sees a half-built one.
    await exec('bunx', ['vite', 'build', '--outDir', 'dist-next/client'], { cwd: uiDir, timeoutMs: 600_000 })
    await exec('bunx', ['vite', 'build', '--config', 'vite.server.config.ts', '--outDir', 'dist-next/server'], { cwd: uiDir, timeoutMs: 600_000 })
    swapDist(uiDir)

    spawnRestartHelper(root)
    // The HTTP response for this action has already been sent by the caller;
    // give it a beat to flush through any proxy, then take the graceful exit.
    const self = process.pid
    setTimeout(() => process.kill(self, 'SIGTERM'), 1_500).unref()
    return { started: true }
  } catch (e) {
    return fail(`Update failed: ${errText(e)}`)
  }
}

/** dist -> dist-prev, dist-next -> dist. dist-prev stays behind after the
 *  swap as the one-generation rollback a human can restore by hand; it is
 *  removed after the first successful post-update read. */
function swapDist(uiDir: string): void {
  rmSync(join(uiDir, 'dist-prev'), { recursive: true, force: true })
  const dist = join(uiDir, 'dist')
  const next = join(uiDir, 'dist-next')
  // rename over an existing directory fails; move the old one aside first,
  // which is also exactly the artifact we want to keep for a manual rollback.
  if (existsSync(dist)) renameSync(dist, join(uiDir, 'dist-prev'))
  renameSync(next, dist)
}

/** Spawn the detached restart helper. It must outlive this process, so it
 *  gets its own process group, a log file to speak into, and nothing else
 *  shared with us but the environment (which carries ui/.env values the
 *  dying server resolved at boot, plus PORT). */
function spawnRestartHelper(root: string): void {
  mkdirSync(join(root, 'logs'), { recursive: true })
  const logFd = openSync(join(root, 'logs', 'updater.log'), 'a')
  const child = spawn('bun', [join(root, 'scripts', 'update-restart.mjs')], {
    cwd: root,
    detached: true,
    stdio: ['ignore', logFd, logFd],
    env: {
      ...process.env,
      TALARIA_UPDATE_UI_DIR: join(root, 'ui'),
      TALARIA_UPDATE_PORT: process.env.PORT || '3000',
    },
  })
  child.unref()
}

// ── Reconciliation ───────────────────────────────────────────────────────────

/** Turn a 'running' lastRun into its final state, judged from where the
 *  server actually is now. Called on the first updater read after a restart
 *  (and by the scheduled job): the new server is the only party that knows
 *  the update landed. */
export async function reconcileUpdate(): Promise<void> {
  const s = await updaterState()
  const run = s.lastRun
  if (!run || run.state !== 'running') return
  const current = await currentRev()
  if (current && current.rev === run.to) {
    await patchState((prev) => {
      if (!prev.lastRun || prev.lastRun.state !== 'running') return prev
      const done: UpdateRun = { ...prev.lastRun, state: 'done', error: null }
      return {
        ...prev,
        lastRun: done,
        history: [done, ...prev.history.filter((r) => r.at !== done.at)].slice(0, 10),
      }
    })
    const root = repoRoot()
    if (root) rmSync(join(root, 'ui', 'dist-prev'), { recursive: true, force: true })
  } else if (Date.now() - Date.parse(run.at) > 30 * 60_000) {
    // Half an hour with no new server is not "still building"; it never came
    // back. Say so rather than leaving a spinner that will never resolve.
    await patchState((prev) => {
      if (!prev.lastRun || prev.lastRun.state !== 'running') return prev
      const failed: UpdateRun = {
        ...prev.lastRun,
        state: 'failed',
        error: 'The server came back on the old version, or never came back. Check logs/talaria.log on the server.',
      }
      return { ...prev, lastRun: failed, history: [failed, ...prev.history.filter((r) => r.at !== failed.at)].slice(0, 10) }
    })
  }
}

/** Flip the auto-update switch. Manual by default; this is the only way on. */
export async function setAutoUpdate(enabled: boolean): Promise<void> {
  await patchState((s) => ({ ...s, autoUpdate: enabled }))
}

// ── The scheduled check ──────────────────────────────────────────────────────
//
// NOT in REQUIRED_JOBS, for the same reason mcp-library-refresh is not: its
// failure mode is "auto-update quietly does not run", which the panel shows
// directly (a stale lastCheck next to a switch that is on), not work that
// silently never happens.
registerJob({
  name: 'update-check',
  everyMs: 6 * 60 * 60 * 1000,
  firstRunDelayMs: 10 * 60_000,
  maxRunMs: 20 * 60_000,
  run: async () => {
    await reconcileUpdate()
    const s = await updaterState()
    if (!s.autoUpdate) return 'auto-update is off'
    if (updaterMode() !== 'server') return 'not a server install'
    const check = await checkForUpdate()
    if (check.behind <= 0) return 'up to date'
    const applied = await applyUpdate('auto')
    return applied.started ? 'installing update' : `skipped: ${applied.error ?? 'unknown reason'}`
  },
})
