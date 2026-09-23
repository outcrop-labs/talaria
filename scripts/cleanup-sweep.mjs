#!/usr/bin/env node
// cleanup-sweep — the disk convention, as a script the stop gate actually runs.
//
// WHY. Agents leave worktrees, devboxes, scratch checkouts and throwaway
// downloads behind. Each one is small. Together they fill the disk, and the
// first notice is a failed build. A skill nobody re-reads does not fix that.
// This file is the check: the stop gate runs it, and exit 2 is the signal.
//
// WHAT IT WILL REMOVE (--apply, and the safe subset on --gate):
//   - a `talaria worktree` checkout (branch wt/<name>, directory
//     ../talaria-<name>) untouched for POLICY.staleMs, not running, clean,
//     and with no unpushed commits
//   - a devbox the same way (../devboxes/<name>, not the shared tools layer)
//   - /tmp/talaria-* (or $TMPDIR) older than POLICY.tmpStaleMs
//   - a compose project named talaria-wt-* or devbox-* whose directory is gone
//
// WHAT IT WILL NOT:
//   - the primary checkout, the checkout this process is running in
//   - anything with a .talaria-keep file in its root
//   - a running stack, a dirty tree, or commits no remote has
//   - a worktree that is not the wt/<name> pair `talaria worktree` creates
//     (a human's long-lived checkout is not this script's)
//   - node_modules, the cargo registry, the bun cache, ../devboxes/shared
//   - the primary checkout's target/ — a warm cache. Flagged when oversized
//     and the disk is already under pressure; never deleted
//
// CONTRACT (scripts/hooks/README.md):
//   exit 0 — nothing to surface. --gate is silent.
//   exit 2 — disk use or removable stale bytes over the line; the reason
//            is on stderr, including the command that clears the removable set.
//   exit 1 — the scan could not run. Not a pass, not a block.
//
// MODES
//   (default)  report. Prints even when under the line, so `talaria cleanup`
//              is useful before the disk is full.
//   --apply    remove the safe set, then report what remains.
//   --gate     what the stop gate runs: remove only orphans and old tmp
//              (unambiguous garbage), then exit 2 if pressure or the
//              remaining removable set is over the line. Silent on exit 0.
//   --pressure one stdout line when use is over the line, else silent.
//              Always exit 0 — `talaria dev` must not refuse to start.
//   --json     the plan, on stdout.

import { execFileSync } from 'node:child_process'
import {
  existsSync,
  mkdirSync,
  readdirSync,
  realpathSync,
  rmSync,
  statfsSync,
  statSync,
  appendFileSync,
} from 'node:fs'
import { basename, dirname, join, resolve, sep } from 'node:path'
import { homedir, tmpdir } from 'node:os'
import { fileURLToPath, pathToFileURL } from 'node:url'

/** The numbers. The skill quotes them; this object is the authority. */
export const POLICY = {
  staleMs: 7 * 24 * 60 * 60 * 1000,
  tmpStaleMs: 24 * 60 * 60 * 1000,
  pressurePct: 85,
  staleBudgetBytes: 2 * 1024 * 1024 * 1024,
  oversizedTargetBytes: 10 * 1024 * 1024 * 1024,
}

const SKIP_WALK = new Set(['node_modules', 'target', '.git', 'dist', 'fleet', '.output', '.vinxi'])


export function formatBytes(n) {
  if (!Number.isFinite(n) || n < 0) return '0 B'
  if (n < 1024) return `${Math.round(n)} B`
  const units = ['KiB', 'MiB', 'GiB', 'TiB']
  let v = n
  let i = -1
  do {
    v /= 1024
    i++
  } while (v >= 1024 && i < units.length - 1)
  return `${v >= 10 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`
}

/** `wt/<name>` living at `talaria-<name>` — the pair `talaria worktree` creates.
 *  Anything else is a human checkout and not ours to sweep. */
export function worktreeName(branch, dirBasename) {
  const m = /^refs\/heads\/wt\/([a-z0-9][a-z0-9-]*)$/.exec(branch ?? '')
  if (!m) return null
  if (dirBasename !== `talaria-${m[1]}`) return null
  return m[1]
}

/**
 * keep | flag | remove.
 * flag = a human has to decide (dirty, unpushed, or an oversized warm cache).
 * remove = the safe set. The executor refuses a path the classifier did not.
 */
export function classify(artifact, policy = POLICY) {
  if (artifact.kind === 'target') {
    return artifact.bytes >= policy.oversizedTargetBytes ? 'flag' : 'keep'
  }
  if (artifact.kind === 'orphan-compose') return 'remove'
  if (artifact.primary || artifact.current || artifact.kept) return 'keep'
  if (artifact.running) return 'keep'
  const limit = artifact.kind === 'tmp' ? policy.tmpStaleMs : policy.staleMs
  if (artifact.ageMs < limit) return 'keep'
  if (artifact.kind === 'tmp') return 'remove'
  if (artifact.dirty || artifact.unpushed) return 'flag'
  return 'remove'
}

/** What this mode is allowed to delete. --gate only takes garbage that has
 *  no checkout left (orphans) or was scratch by name (old tmp). Worktrees
 *  and boxes wait for --apply, so a stop hook never surprises a parked tree. */
export function selectRemoval(classified, mode) {
  const removable = classified.filter((a) => a.action === 'remove')
  if (mode === 'apply') return removable
  if (mode === 'gate') return removable.filter((a) => a.kind === 'tmp' || a.kind === 'orphan-compose')
  return []
}

export function verdict(remaining, pressure, policy = POLICY) {
  const removable = remaining.filter((a) => a.action === 'remove')
  const removableBytes = removable.reduce((n, a) => n + (a.bytes || 0), 0)
  const pressureHigh = pressure.usedPct >= policy.pressurePct
  const budgetHigh = removableBytes >= policy.staleBudgetBytes
  return {
    exit: pressureHigh || budgetHigh ? 2 : 0,
    pressureHigh,
    budgetHigh,
    removableBytes,
  }
}

/** Last line of defense. classify can be wrong; the path still has to match
 *  the convention before anything is deleted. */
export function removalAllowed(artifact, roots) {
  if (artifact.kind === 'orphan-compose') {
    return /^(talaria-wt-|devbox-)[a-z0-9][a-z0-9-]*$/.test(artifact.id) && !artifact.path
  }
  if (!artifact.path) return false
  const path = resolve(artifact.path)
  if (roots.primary && path === resolve(roots.primary)) return false
  if (roots.current && path === resolve(roots.current)) return false
  if (artifact.kind === 'tmp') {
    const tmp = resolve(roots.tmp)
    return path.startsWith(tmp + sep) && basename(path).startsWith('talaria-')
  }
  if (artifact.kind === 'worktree') {
    return basename(path).startsWith('talaria-') && dirname(path) === resolve(roots.siblingParent)
  }
  if (artifact.kind === 'devbox') {
    const home = resolve(roots.devboxHome)
    return path.startsWith(home + sep) && basename(path) !== 'shared'
  }
  return false
}

export function formatReport({ pressure, remaining, removed, notes, policy = POLICY }) {
  const v = verdict(remaining, pressure, policy)
  const lines = []
  const pct = pressure.usedPct.toFixed(0)
  const free = formatBytes(pressure.avail)
  lines.push(
    `disk ${pct}% used (${free} free)` +
      (v.pressureHigh ? ` — over the ${policy.pressurePct}% line` : ` — under the ${policy.pressurePct}% line`),
  )
  if (removed?.length) {
    lines.push('removed:')
    for (const a of removed) lines.push(`  ${a.kind} ${a.id}  ${formatBytes(a.bytes)}  ${a.path || a.id}`)
  }
  const toRemove = remaining.filter((a) => a.action === 'remove')
  const flagged = remaining.filter((a) => a.action === 'flag')
  if (toRemove.length) {
    lines.push(
      `removable ${formatBytes(v.removableBytes)}` +
        (v.budgetHigh ? ` — over the ${formatBytes(policy.staleBudgetBytes)} line` : ''),
    )
    lines.push('remove with `bun talaria cleanup --apply`:')
    for (const a of toRemove) lines.push(`  ${a.kind} ${a.id}  ${formatBytes(a.bytes)}  ${ageDays(a.ageMs)}  ${a.path || ''}`)
  }
  if (flagged.length) {
    lines.push('flagged, not removed (dirty, unpushed, or a warm cache — a human decides):')
    for (const a of flagged) {
      const why = a.kind === 'target' ? 'oversized cache' : a.dirty ? 'dirty' : a.unpushed ? 'unpushed' : 'kept back'
      lines.push(`  ${a.kind} ${a.id}  ${formatBytes(a.bytes)}  ${why}  ${a.path || ''}`)
    }
  }
  if (!toRemove.length && !flagged.length && !removed?.length) lines.push('nothing stale.')
  if (v.pressureHigh && flagged.some((a) => a.kind === 'target')) {
    lines.push('an oversized target/ was kept on purpose. `cargo clean --manifest-path api/Cargo.toml` if that is the pressure.')
  }
  if (v.exit === 2) {
    lines.push('clear the removable set with `bun talaria cleanup --apply`, then claim done.')
    lines.push('a tree you still need: touch `.talaria-keep` in its root and it will be left alone.')
  }
  for (const n of notes ?? []) lines.push(n)
  return lines.join('\n') + '\n'
}

function ageDays(ms) {
  if (!Number.isFinite(ms) || ms < 0) return '?'
  const d = ms / (24 * 60 * 60 * 1000)
  return d >= 1 ? `${d.toFixed(0)}d` : `${Math.max(1, Math.round(d * 24))}h`
}

function real(p) {
  try {
    return realpathSync(p)
  } catch {
    return resolve(p)
  }
}

function git(args, cwd) {
  return execFileSync('git', args, { cwd, encoding: 'utf8', timeout: 30_000, stdio: ['ignore', 'pipe', 'pipe'] })
}

function tryGit(args, cwd) {
  try {
    return git(args, cwd).trim()
  } catch {
    return null
  }
}

function newestMs(dir, depthLimit) {
  let max = 0
  const walk = (d, depth) => {
    let st
    try {
      st = statSync(d)
    } catch {
      return
    }
    if (st.mtimeMs > max) max = st.mtimeMs
    if (!st.isDirectory()) return
    if (depthLimit !== undefined && depth >= depthLimit) return
    let entries
    try {
      entries = readdirSync(d, { withFileTypes: true })
    } catch {
      return
    }
    for (const e of entries) {
      if (SKIP_WALK.has(e.name)) continue
      walk(join(d, e.name), depth + 1)
    }
  }
  walk(dir, 0)
  return max
}

function dirBytes(path) {
  try {
    const out = execFileSync('du', ['-sb', '--', path], {
      encoding: 'utf8',
      timeout: 60_000,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    return Number(out.trim().split(/\s+/)[0]) || 0
  } catch {
    return 0
  }
}

export function readPressure(path) {
  const s = statfsSync(path)
  const total = Number(s.blocks) * Number(s.bsize)
  const avail = Number(s.bavail) * Number(s.bsize)
  const used = total - avail
  return { total, avail, used, usedPct: total === 0 ? 0 : (used / total) * 100 }
}

function parseComposeLs(text) {
  const trimmed = text.trim()
  if (!trimmed) return []
  if (trimmed.startsWith('[')) {
    const parsed = JSON.parse(trimmed)
    return Array.isArray(parsed) ? parsed : []
  }
  return trimmed
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line))
}

function composeProjects() {
  try {
    const out = execFileSync('docker', ['compose', 'ls', '-a', '--format', 'json'], {
      encoding: 'utf8',
      timeout: 8_000,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    return parseComposeLs(out)
  } catch {
    return null
  }
}

function projectRunning(projects, name) {
  if (!projects) return true
  const row = projects.find((p) => p.Name === name)
  if (!row) return false
  return /running/i.test(row.Status ?? '')
}

function parseWorktrees(root) {
  const text = git(['worktree', 'list', '--porcelain'], root)
  const out = []
  let cur = null
  for (const line of text.split('\n')) {
    if (line.startsWith('worktree ')) {
      if (cur) out.push(cur)
      cur = { path: line.slice('worktree '.length), branch: '' }
    } else if (cur && line.startsWith('branch ')) {
      cur.branch = line.slice('branch '.length)
    }
  }
  if (cur) out.push(cur)
  return out
}

function commitMs(dir) {
  const s = tryGit(['log', '-1', '--format=%ct'], dir)
  return s ? Number(s) * 1000 : 0
}

function isDirty(dir) {
  const s = tryGit(['status', '--porcelain'], dir)
  return s === null ? true : s !== ''
}

function isUnpushed(dir) {
  const upstream = tryGit(['rev-list', '--count', '@{upstream}..HEAD'], dir)
  if (upstream !== null) return Number(upstream) > 0
  const remotes = tryGit(['rev-list', '--count', '--not', '--remotes', 'HEAD'], dir)
  if (remotes !== null) return Number(remotes) > 0
  return true
}

function devboxHome(root) {
  return process.env.TALARIA_DEVBOX_HOME || join(dirname(root), 'devboxes')
}

/**
 * Inventory. docker down is treated as "in use" so a missing daemon cannot
 * make a live worktree look abandoned. Sizes are filled in by the caller for
 * the rows that can actually be removed or flagged.
 */
export function inventory(root, now = Date.now()) {
  const notes = []
  const current = real(root)
  const listed = parseWorktrees(root)
  const primary = listed.length ? real(listed[0].path) : current
  const projects = composeProjects()
  if (projects === null) notes.push('docker unavailable — stacks treated as in use; orphan projects not scanned')
  const artifacts = []
  const seenWorktree = new Set()

  for (const wt of listed) {
    const path = real(wt.path)
    const name = worktreeName(wt.branch, basename(path))
    if (!name) continue
    seenWorktree.add(name)
    const ageMs = now - Math.max(newestMs(path), commitMs(path), statMs(join(path, 'api/target')), statMs(join(path, 'desktop/src-tauri/target')))
    const running = projectRunning(projects, `talaria-wt-${name}`)
    const stale = ageMs >= POLICY.staleMs && !running && path !== current && path !== primary && !existsSync(join(path, '.talaria-keep'))
    artifacts.push({
      kind: 'worktree',
      id: name,
      path,
      bytes: 0,
      ageMs,
      running,
      dirty: stale ? isDirty(path) : false,
      unpushed: stale ? isUnpushed(path) : false,
      kept: existsSync(join(path, '.talaria-keep')),
      current: path === current,
      primary: path === primary,
    })
  }

  const home = devboxHome(root)
  if (existsSync(home)) {
    for (const e of readdirSync(home, { withFileTypes: true })) {
      if (!e.isDirectory() || e.name === 'shared' || e.name.startsWith('.')) continue
      const path = real(join(home, e.name))
      if (!existsSync(join(path, 'box.env'))) continue
      const running = projectRunning(projects, `devbox-${e.name}`)
      const ageMs = now - Math.max(newestMs(path), statMs(join(path, 'box.env')))
      const stale = ageMs >= POLICY.staleMs && !running && !existsSync(join(path, '.talaria-keep'))
      const clone = join(path, 'talaria')
      artifacts.push({
        kind: 'devbox',
        id: e.name,
        path,
        bytes: 0,
        ageMs,
        running,
        dirty: stale && existsSync(clone) ? isDirty(clone) : false,
        unpushed: stale && existsSync(clone) ? isUnpushed(clone) : false,
        kept: existsSync(join(path, '.talaria-keep')),
        current: false,
        primary: false,
      })
    }
  }

  const tmp = process.env.TALARIA_CLEANUP_TMP || tmpdir()
  if (existsSync(tmp)) {
    for (const e of readdirSync(tmp, { withFileTypes: true })) {
      if (!e.name.startsWith('talaria-')) continue
      const path = join(tmp, e.name)
      const ageMs = now - newestMs(path, 2)
      artifacts.push({
        kind: 'tmp',
        id: e.name,
        path,
        bytes: 0,
        ageMs,
        running: false,
        dirty: false,
        unpushed: false,
        kept: false,
        current: false,
        primary: false,
      })
    }
  }

  if (projects) {
    const sibling = dirname(primary)
    for (const row of projects) {
      const name = row.Name ?? ''
      const wt = /^talaria-wt-([a-z0-9][a-z0-9-]*)$/.exec(name)
      if (wt && !existsSync(join(sibling, `talaria-${wt[1]}`)) && !seenWorktree.has(wt[1])) {
        artifacts.push(orphan(name))
        continue
      }
      const box = /^devbox-([a-z0-9][a-z0-9-]*)$/.exec(name)
      if (!box) continue
      // A project ending in -fleet is that box's fleet network, or a box whose
      // own name ends in -fleet. Orphan only when neither checkout exists.
      const raw = box[1]
      const stripped = raw.endsWith('-fleet') ? raw.slice(0, -'-fleet'.length) : null
      const candidates = [raw, stripped].filter((n) => n && n !== 'shared')
      if (candidates.length === 0) continue
      if (candidates.every((n) => !existsSync(join(home, n, 'box.env')))) artifacts.push(orphan(name))
    }
  }

  return { artifacts, notes, roots: { primary, current, siblingParent: dirname(primary), devboxHome: home, tmp: process.env.TALARIA_CLEANUP_TMP || tmpdir() } }
}

function statMs(path) {
  try {
    return statSync(path).mtimeMs
  } catch {
    return 0
  }
}

function orphan(name) {
  return {
    kind: 'orphan-compose',
    id: name,
    path: '',
    bytes: 0,
    ageMs: POLICY.staleMs,
    running: false,
    dirty: false,
    unpushed: false,
    kept: false,
    current: false,
    primary: false,
  }
}


function logRemoval(line) {
  try {
    const dir = join(homedir(), '.local', 'state', 'talaria')
    mkdirSync(dir, { recursive: true })
    appendFileSync(join(dir, 'cleanup.log'), `${new Date().toISOString()} ${line}\n`)
  } catch {
    // a log that cannot be written must not hide the removal
  }
}

function dockerByLabel(project) {
  const filter = `label=com.docker.compose.project=${project}`
  const ids = tryExec('docker', ['ps', '-aq', '--filter', filter])
  if (ids) tryExec('docker', ['rm', '-f', ...ids.split('\n').filter(Boolean)])
  const vols = tryExec('docker', ['volume', 'ls', '-q', '--filter', filter])
  if (vols) tryExec('docker', ['volume', 'rm', ...vols.split('\n').filter(Boolean)])
  const nets = tryExec('docker', ['network', 'ls', '-q', '--filter', filter])
  if (nets) tryExec('docker', ['network', 'rm', ...nets.split('\n').filter(Boolean)])
}

function tryExec(cmd, args) {
  try {
    return execFileSync(cmd, args, { encoding: 'utf8', timeout: 60_000, stdio: ['ignore', 'pipe', 'pipe'] }).trim()
  } catch {
    return ''
  }
}

export function removeArtifact(artifact, roots) {
  if (!removalAllowed(artifact, roots)) {
    throw new Error(`refusing to remove ${artifact.kind} ${artifact.id}: path is outside the sweep`)
  }
  if (artifact.kind === 'orphan-compose') {
    dockerByLabel(artifact.id)
    return
  }
  if (artifact.kind === 'tmp') {
    rmSync(artifact.path, { recursive: true, force: true })
    return
  }
  if (artifact.kind === 'worktree') {
    const project = `talaria-wt-${artifact.id}`
    tryExec('docker', [
      'compose',
      '-p',
      project,
      '-f',
      join(roots.primary, 'docker/sidecars.compose.yml'),
      '-f',
      join(roots.primary, 'docker/dev-compose.yml'),
      'down',
      '-v',
    ])
    tryExec('docker', ['rm', '-f', `talaria-pg-${artifact.id}`, `talaria-redis-${artifact.id}`])
    dockerByLabel(project)
    tryExec('git', ['-C', roots.primary, 'worktree', 'remove', '--force', artifact.path])
    if (existsSync(artifact.path)) rmSync(artifact.path, { recursive: true, force: true })
    tryExec('git', ['-C', roots.primary, 'worktree', 'prune'])
    tryExec('git', ['-C', roots.primary, 'branch', '-D', `wt/${artifact.id}`])
    return
  }
  if (artifact.kind === 'devbox') {
    const project = `devbox-${artifact.id}`
    const args = [
      'compose',
      '-p',
      project,
      '-f',
      join(roots.primary, 'docker/sidecars.compose.yml'),
      '-f',
      join(roots.primary, 'docker/devbox.compose.yml'),
    ]
    const envFile = join(artifact.path, 'compose.env')
    if (existsSync(envFile)) args.push('--env-file', envFile)
    const override = join(artifact.path, 'compose.override.yml')
    if (existsSync(override)) args.push('-f', override)
    args.push('down', '-v', '--remove-orphans')
    tryExec('docker', args)
    dockerByLabel(project)
    dockerByLabel(`${project}-fleet`)
    rmSync(artifact.path, { recursive: true, force: true })
  }
}


function measure(list, pressure, primary) {
  const out = list.map((a) => ({ ...a }))
  for (const a of out) {
    a.action = classify(a)
    if ((a.action === 'remove' || a.action === 'flag') && a.path) a.bytes = dirBytes(a.path)
    a.action = classify(a)
  }
  if (pressure.usedPct >= POLICY.pressurePct) {
    for (const rel of ['api/target', 'desktop/src-tauri/target']) {
      const path = join(primary, rel)
      if (!existsSync(path)) continue
      const row = {
        kind: 'target',
        id: rel,
        path,
        bytes: dirBytes(path),
        ageMs: 0,
        running: false,
        dirty: false,
        unpushed: false,
        kept: true,
        current: true,
        primary: true,
      }
      row.action = classify(row)
      out.push(row)
    }
  }
  return out
}

function help() {
  return `talaria cleanup — flag or sweep stale dev artifacts

  node scripts/cleanup-sweep.mjs            report
  node scripts/cleanup-sweep.mjs --apply    remove the safe set
  node scripts/cleanup-sweep.mjs --gate     stop-gate mode (orphans + old tmp, then block if still over the line)
  node scripts/cleanup-sweep.mjs --pressure one line when disk use is over ${POLICY.pressurePct}%

Thresholds live in POLICY in this file. A directory with .talaria-keep is left alone.
`
}

function invokedDirectly() {
  const arg = process.argv[1]
  if (!arg) return false
  try {
    return realpathSync(arg) === realpathSync(fileURLToPath(import.meta.url))
  } catch {
    return pathToFileURL(resolve(arg)).href === import.meta.url
  }
}

function main() {
  const argv = process.argv.slice(2)
  if (argv.includes('--help') || argv.includes('-h')) {
    process.stdout.write(help())
    process.exit(0)
  }
  const mode = argv.includes('--apply') ? 'apply' : argv.includes('--gate') ? 'gate' : argv.includes('--pressure') ? 'pressure' : 'report'
  const json = argv.includes('--json')
  const root = real(resolve(fileURLToPath(new URL('.', import.meta.url)), '..'))

  let pressure
  try {
    pressure = readPressure(root)
  } catch (e) {
    console.error(`cleanup sweep could not read disk use — not a pass: ${e instanceof Error ? e.message : e}`)
    process.exit(1)
  }

  if (mode === 'pressure') {
    if (pressure.usedPct >= POLICY.pressurePct) {
      process.stdout.write(
        `disk ${pressure.usedPct.toFixed(0)}% used (${formatBytes(pressure.avail)} free) — over the ${POLICY.pressurePct}% line. bun talaria cleanup\n`,
      )
    }
    process.exit(0)
  }

  let inventoried
  try {
    inventoried = inventory(root)
  } catch (e) {
    console.error(`cleanup sweep could not scan — not a pass: ${e instanceof Error ? e.message : e}`)
    process.exit(1)
  }

  let classified = measure(inventoried.artifacts, pressure, inventoried.roots.primary)
  const removing = selectRemoval(classified, mode)
  const removed = []
  const failed = []
  for (const a of removing) {
    try {
      removeArtifact(a, inventoried.roots)
      removed.push(a)
      logRemoval(`removed ${a.kind} ${a.id} ${a.path || ''}`.trim())
    } catch (e) {
      failed.push(`${a.kind} ${a.id}: ${e instanceof Error ? e.message : e}`)
    }
  }
  if (removed.length) {
    const gone = new Set(removed)
    classified = classified.filter((a) => !gone.has(a))
    try {
      pressure = readPressure(root)
    } catch {
      // the pre-removal reading still answers "are we over the line"
    }
  }
  const v = verdict(classified, pressure)
  const notes = [...inventoried.notes, ...failed.map((f) => `could not remove ${f}`)]
  const report = formatReport({ pressure, remaining: classified, removed, notes })

  if (json) {
    process.stdout.write(JSON.stringify({ exit: v.exit, pressure, removed: removed.map((a) => a.id), remaining: classified }, null, 2) + '\n')
  } else if (mode === 'gate') {
    if (v.exit === 2) process.stderr.write(report)
  } else {
    process.stdout.write(report)
  }
  process.exit(v.exit)
}

if (invokedDirectly()) main()
