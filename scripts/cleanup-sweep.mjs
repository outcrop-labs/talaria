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
//     ../talaria-<name>) that is clean, has no unpushed commits, and is
//     either untouched for POLICY.staleMs or has a MERGED pull request
//   - the docker stack of such a worktree — containers, volumes, network.
//     This one runs in --gate too: it frees RAM and CPU the moment the work
//     is finished, removes nothing from disk, and `talaria dev` undoes it
//   - a side worktree's build dir untouched for POLICY.staleTargetMs
//   - a devbox the same way (../devboxes/<name>, not the shared tools layer)
//   - /tmp/talaria-* (or $TMPDIR) older than POLICY.tmpStaleMs
//   - a compose project named talaria-wt-* or devbox-* whose directory is gone
//
// WHAT IT WILL NOT:
//   - the primary checkout, the checkout this process is running in
//   - anything with a .talaria-keep file in its root
//   - a dirty tree, or commits no remote has
//   - a worktree that is not the wt/<name> pair `talaria worktree` creates
//     (a human's long-lived checkout is not this script's)
//   - node_modules, the cargo registry, the bun cache, ../devboxes/shared
//   - the primary checkout's target/ — a warm cache. ALWAYS reported with
//     its size, flagged when oversized, never deleted on a timer
//   - a docker image, volume or exited container outside a swept project.
//     The reclaimable total is reported; pruning it is a human's call
//
// "MERGED" MEANS THE PULL REQUEST MERGED, and nothing else. Ancestry is not
// the test: a worktree cut an hour ago from main also has a HEAD that is an
// ancestor of rc, and reading that as finished tears down a live stack
// somebody else is using. Every merge check fails open.
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
  // A build dir in a SIDE worktree, untouched this long, is spent. The
  // primary's is a warm cache and is never removed on a timer — only named.
  staleTargetMs: 14 * 24 * 60 * 60 * 1000,
  // Flagged bulk earns a loud line, never an exit 2. Blocking on something
  // the sweep refuses to delete is a gate with no way through, and a gate
  // with no way through gets silenced.
  nagBytes: 20 * 1024 * 1024 * 1024,
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
    // The primary's build dir is a warm cache: named when oversized, never
    // removed on a timer. A side worktree's is spent once the tree has sat.
    if (artifact.primary || artifact.current || artifact.kept) {
      return artifact.bytes >= policy.oversizedTargetBytes ? 'flag' : 'keep'
    }
    if (artifact.ageMs >= policy.staleTargetMs) return 'remove'
    return artifact.bytes >= policy.oversizedTargetBytes ? 'flag' : 'keep'
  }
  if (artifact.kind === 'orphan-compose') return 'remove'
  // A stack row exists only for a worktree whose work is finished (its branch
  // reached the base) or which has gone stale. Stopping it frees RAM and CPU
  // and is reversible by `talaria dev`; the checkout itself is never touched.
  if (artifact.kind === 'stack') return artifact.current ? 'keep' : 'remove'
  if (artifact.primary || artifact.current || artifact.kept) return 'keep'
  // A merged branch is finished work: the 7-day idle clock is the wrong
  // instrument for it. Still only ever flagged when dirty or unpushed.
  if (artifact.merged && artifact.kind === 'worktree') {
    return artifact.dirty || artifact.unpushed ? 'flag' : 'remove'
  }
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
  // `stack` joins the gate set: it stops containers and removes nothing from
  // disk, so the worst case is a stack the owner restarts with `talaria dev`.
  // A checkout is never deleted without an explicit --apply.
  if (mode === 'gate') {
    return removable.filter((a) => a.kind === 'tmp' || a.kind === 'orphan-compose' || a.kind === 'stack')
  }
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
  // A stack removes containers, never a directory — so it carries no path,
  // and the compose project name is the whole of what may be acted on.
  if (artifact.kind === 'stack') {
    return /^[a-z0-9][a-z0-9-]*$/.test(artifact.id) && !artifact.path
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
  if (artifact.kind === 'target') {
    // Only a build dir inside a SIDE worktree, and only at one of the two
    // paths the repo actually builds into. The primary's is never removable
    // here however stale it looks — `cargo clean` stays a human's call.
    const owner = TARGET_RELS.map((rel) => trimSuffix(path, rel)).find(Boolean)
    if (!owner) return false
    if (roots.primary && owner === resolve(roots.primary)) return false
    if (roots.current && owner === resolve(roots.current)) return false
    return basename(owner).startsWith('talaria-') && dirname(owner) === resolve(roots.siblingParent)
  }
  return false
}

/** The two build dirs this repo produces, relative to a checkout root. */
export const TARGET_RELS = ['api/target', 'desktop/src-tauri/target']

/** `/w/talaria-x/api/target` minus `api/target` → `/w/talaria-x`, else null. */
function trimSuffix(path, rel) {
  const tail = sep + rel.split('/').join(sep)
  return path.endsWith(tail) ? path.slice(0, -tail.length) : null
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
  // Build caches are reported at every disk reading, kept or not. They are
  // the largest thing this repo produces and the slowest to notice.
  const caches = remaining.filter((a) => a.kind === 'target')
  const cacheBytes = caches.reduce((n, a) => n + (a.bytes || 0), 0)
  if (caches.length) {
    lines.push(`build caches ${formatBytes(cacheBytes)}:`)
    for (const a of caches) {
      const note = a.action === 'remove' ? 'stale — removable' : a.primary ? 'warm cache' : 'in use'
      lines.push(`  ${a.id}  ${formatBytes(a.bytes)}  ${ageDays(a.ageMs)}  ${note}`)
    }
  }
  if (!toRemove.length && !flagged.length && !removed?.length && !caches.length) lines.push('nothing stale.')
  if (cacheBytes >= policy.nagBytes) {
    lines.push(
      `that is over ${formatBytes(policy.nagBytes)} of build cache. \`cargo clean --manifest-path api/Cargo.toml\` reclaims it; ` +
        'the next build pays for it, and sccache keeps that cheap.',
    )
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

/**
 * What docker is holding that it would give back. Reported, never acted on:
 * `docker system df` counts images and volumes across every project on the
 * machine, most of which are not this repo's to prune — and an exited
 * container is as likely to be a one-shot init sidecar as it is garbage.
 * Naming the number is the job; `docker system prune` stays a human's call.
 */
function dockerReclaimable() {
  const out = tryExec('docker', ['system', 'df', '--format', 'json'])
  if (!out) return null
  const rows = []
  for (const line of out.split('\n').filter(Boolean)) {
    try {
      const r = JSON.parse(line)
      if (Array.isArray(r)) rows.push(...r)
      else rows.push(r)
    } catch {
      return null
    }
  }
  const parts = rows
    .filter((r) => r.Reclaimable && !/^0B/.test(String(r.Reclaimable)))
    .map((r) => `${String(r.Type).toLowerCase()} ${r.Reclaimable}`)
  return parts.length ? parts.join(', ') : null
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
 * Is this branch's work finished — as in, its pull request merged?
 *
 * Only the pull request can answer that. Ancestry cannot: a worktree cut an
 * hour ago from `main` also has a HEAD that is an ancestor of `rc`, and
 * reading that as "merged" tears down a colleague's running stack. The
 * distinction is not "has this branch got commits in the base" but "did this
 * branch's work land", and that fact lives on the PR.
 *
 * A local prefilter keeps the network call rare: a branch still ahead of the
 * base is obviously unfinished, so it never reaches `gh`. Everything fails
 * OPEN — no `gh`, no auth, no network, no PR all read as "not merged", so a
 * tree is never reclaimed on a guess.
 */
function isMerged(dir, branch) {
  const ahead = tryGit(['rev-list', '--count', 'origin/rc..HEAD'], dir)
  if (ahead === null || Number(ahead) > 0) return false
  try {
    const out = execFileSync('gh', ['pr', 'view', branch, '--json', 'state', '-q', '.state'], {
      cwd: dir,
      encoding: 'utf8',
      timeout: 5_000,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    return out.trim() === 'MERGED'
  } catch {
    return false
  }
}

/** A build directory. Always inventoried, whatever the disk reading — an
 *  artifact nobody can see is the one that fills the disk. `classify` still
 *  decides, and the primary's is never removed on a timer. */
function targetRow(path, rel, now, flags) {
  const owner = trimSuffix(resolve(path), rel)
  return {
    kind: 'target',
    id: flags.primary || !owner ? rel : `${basename(owner)}/${rel}`,
    path,
    bytes: 0,
    ageMs: now - statMs(path),
    merged: false,
    running: false,
    dirty: false,
    unpushed: false,
    kept: flags.kept,
    current: flags.current,
    primary: flags.primary,
  }
}

/** A running stack whose worktree is finished or stale. Stopping it frees
 *  RAM and CPU; the checkout is a separate row and a separate decision. */
function stackRow(name, ageMs, merged, current) {
  return {
    kind: 'stack',
    id: name,
    path: '',
    bytes: 0,
    ageMs,
    merged,
    running: true,
    dirty: false,
    unpushed: false,
    kept: false,
    current,
    primary: false,
  }
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
  const reclaim = dockerReclaimable()
  if (reclaim) notes.push(`docker is holding reclaimable space: ${reclaim}. \`docker system prune\` is a human's call.`)
  const artifacts = []
  const seenWorktree = new Set()

  for (const wt of listed) {
    const path = real(wt.path)
    const name = worktreeName(wt.branch, basename(path))
    if (!name) continue
    seenWorktree.add(name)
    const ageMs = now - Math.max(newestMs(path), commitMs(path), statMs(join(path, 'api/target')), statMs(join(path, 'desktop/src-tauri/target')))
    const running = projectRunning(projects, `talaria-wt-${name}`)
    const isCurrent = path === current
    const isPrimary = path === primary
    const kept = existsSync(join(path, '.talaria-keep'))
    // Merged is a reclaim signal on its own, so it is worth a lookup for any
    // side tree — but never for the one we are standing in.
    const merged = !isCurrent && !isPrimary && !kept ? isMerged(path, `wt/${name}`) : false
    const stale = ageMs >= POLICY.staleMs && !running && !isCurrent && !isPrimary && !kept
    // Finished or stale, the git state decides remove vs flag, so read it.
    const inspect = stale || (merged && !isCurrent && !isPrimary && !kept)
    artifacts.push({
      kind: 'worktree',
      id: name,
      path,
      bytes: 0,
      ageMs,
      merged,
      running,
      dirty: inspect ? isDirty(path) : false,
      unpushed: inspect ? isUnpushed(path) : false,
      kept,
      current: isCurrent,
      primary: isPrimary,
    })
    if (running && (merged || ageMs >= POLICY.staleMs) && !kept) {
      artifacts.push(stackRow(name, ageMs, merged, isCurrent))
    }
    for (const rel of TARGET_RELS) {
      const tp = join(path, rel)
      if (!existsSync(tp)) continue
      artifacts.push(targetRow(tp, rel, now, { current: isCurrent, primary: isPrimary, kept }))
    }
  }

  // The primary is not a `wt/<name>` pair, so the loop above skips it — but
  // its build dir is the biggest thing on this disk and has to be counted.
  for (const rel of TARGET_RELS) {
    const tp = join(primary, rel)
    if (!existsSync(tp)) continue
    artifacts.push(targetRow(tp, rel, now, { current: primary === current, primary: true, kept: false }))
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

/** Stop a worktree stack and take its volumes and network with it. Touches
 *  no directory, so it is safe in the gate: `talaria dev` brings it back. */
function downWorktreeStack(name, roots) {
  const project = `talaria-wt-${name}`
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
  tryExec('docker', ['rm', '-f', `talaria-pg-${name}`, `talaria-redis-${name}`])
  dockerByLabel(project)
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
  if (artifact.kind === 'stack') {
    downWorktreeStack(artifact.id, roots)
    return
  }
  if (artifact.kind === 'target') {
    rmSync(artifact.path, { recursive: true, force: true })
    return
  }
  if (artifact.kind === 'worktree') {
    downWorktreeStack(artifact.id, roots)
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


/**
 * Size and classify. A build dir is sized ALWAYS, never behind a disk
 * reading: `classify` needs its bytes to judge it, and a 35 GiB cache that
 * only becomes visible at 85% of a 638 GiB disk is a cache nobody ever sees.
 * Everything else is sized only when it is already a candidate — `du` on a
 * tree we are going to keep is wasted work.
 */
function measure(list) {
  const out = list.map((a) => ({ ...a }))
  for (const a of out) {
    a.action = classify(a)
    const sized = a.kind === 'target' || a.action === 'remove' || a.action === 'flag'
    if (sized && a.path) a.bytes = dirBytes(a.path)
    a.action = classify(a)
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

  let classified = measure(inventoried.artifacts)
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
