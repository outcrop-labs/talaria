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
//   - (--apply only) a network a `talaria-*` or `devbox-*` compose project
//     left behind: nothing attached, running or stopped; no container left in
//     its project; no live worktree or box owning it; and older than
//     POLICY.orphanNetworkMinAgeMs. Each one holds a subnet from the daemon's
//     ~30 address pools, and with parallel stacks they ran out (follow-ups A8,
//     R20). Also a pool probe network the CLI failed to remove (label
//     talaria.pool-probe)
//   - (--apply only) an app database's container (`talaria-appdb-*`) whose
//     cluster is mounted from a sibling `talaria-<name>` checkout that is no
//     longer a checkout, or from a box whose box.env is gone (N9). The
//     container only: its data directory stays
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
//   - the shared external `talaria` network, or any network compose did not
//     make for a `talaria-*` or `devbox-*` project (supabase_*, a person's
//     own), or a box project's network named outside `devbox-*`
//   - an app database mounted from anywhere but a sibling `talaria-*`
//     checkout or a box (an image install's, a TALARIA_APP_DATA_DIR
//     elsewhere), or one whose checkout or box still exists
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
  // A network younger than this is never an orphan: `compose up` creates the
  // project network first and its containers a moment later, and a sweep
  // that ran in between would pull the network out from under it.
  orphanNetworkMinAgeMs: 10 * 60 * 1000,
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
/** The worktree a compose project belongs to, or null for a project that is
 *  not a worktree's. `talaria worktree <name>` names its stack
 *  `talaria-wt-<name>`; the agents that worktree runs live in a SECOND
 *  project, `talaria-wt-<name>-fleet` (the api's TALARIA_FLEET_PROJECT), so
 *  `-fleet` is the worktree's own suffix before it is part of a name. Read
 *  bare, that project parsed as the worktree `<name>-fleet`, no such checkout
 *  ever exists, and the gate removed a live agent container at every turn
 *  boundary. */
export function worktreeProjectOwner(projectName) {
  const wt = /^talaria-wt-([a-z0-9][a-z0-9-]*)$/.exec(projectName)
  if (!wt) return null
  return wt[1].endsWith('-fleet') ? wt[1].slice(0, -'-fleet'.length) : wt[1]
}

/** The boxes a `devbox-*` compose project may belong to, or null for a
 *  project that is not a box's. `talaria box new <name>` runs `devbox-<name>`,
 *  and the agents that box renders live in `devbox-<name>-fleet` — but a box
 *  may itself be named `<x>-fleet`, so both readings are candidates, and the
 *  project is a live box's while EITHER checkout exists. `shared` is the
 *  tools layer, never a box; an empty list means nothing can vouch for it. */
export function boxProjectOwners(projectName) {
  const box = /^devbox-([a-z0-9][a-z0-9-]*)$/.exec(projectName)
  if (!box) return null
  const raw = box[1]
  const stripped = raw.endsWith('-fleet') ? raw.slice(0, -'-fleet'.length) : null
  return [raw, stripped].filter((n) => n && n !== 'shared')
}

/** `k=v,k=v` — how `docker network ls --format '{{json .}}'` spells Labels. */
export function parseLabels(text) {
  const out = {}
  for (const pair of String(text ?? '').split(',')) {
    const i = pair.indexOf('=')
    if (i > 0) out[pair.slice(0, i)] = pair.slice(i + 1)
  }
  return out
}

/** docker's CreatedAt (`2026-10-08 20:59:58.979 -0600 MDT`) in epoch ms, or
 *  0 when it does not parse — and 0 is "too young to remove", never "old". */
export function parseDockerTime(text) {
  const m = /^(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2}:\d{2})(\.\d+)? ([+-]\d{2})(\d{2})/.exec(String(text ?? ''))
  if (!m) return 0
  const ms = Date.parse(`${m[1]}T${m[2]}${(m[3] ?? '').slice(0, 4)}${m[4]}:${m[5]}`)
  return Number.isFinite(ms) ? ms : 0
}

/**
 * Networks a `talaria-*` compose project left behind, as sweep rows. Pure:
 * the inventory hands it what docker said.
 *
 *   networks    [{ id, name, labels, createdMs }] — compose-made networks
 *               (and pool probes)
 *   containers  [{ project, networks: [name] }] — EVERY container, running
 *               or stopped: a stopped one still names its network, and
 *               removing that network would leave it unable to start
 *   liveWorktree(owner) — does that worktree's checkout exist
 *   liveBox(name)       — does that box's checkout (its box.env) exist.
 *               Defaults to yes, so a caller that cannot say never
 *               removes a box's network
 *
 * A network is an orphan only when nothing at all is attached, its project
 * has no container left (so no stack, running or stopped, owns it), no live
 * worktree or box owns the project, and it is older than the min age. The
 * shared `talaria` network is never one, whatever its labels say; neither is
 * any network outside a `talaria-*` or `devbox-*` project, nor a box
 * project's network whose own name is outside `devbox-*`.
 */
export function orphanNetworks({ networks, containers, liveWorktree, liveBox = () => true, now = Date.now(), policy = POLICY }) {
  const rows = []
  for (const net of networks) {
    if (!net?.name || net.name === 'talaria') continue
    const probe = net.labels?.['talaria.pool-probe'] === '1'
    const project = net.labels?.['com.docker.compose.project'] ?? ''
    const boxes = probe ? null : boxProjectOwners(project)
    if (!probe && !project.startsWith('talaria-') && !boxes) continue
    if (boxes) {
      // A box's network goes only by the box's own prefix, and only once no
      // box it could belong to has a checkout left (`talaria box rm` takes
      // the directory; a stopped box keeps it, and its containers too).
      if (!net.name.startsWith('devbox-')) continue
      if (boxes.length === 0 || boxes.some((n) => liveBox(n))) continue
    } else if (!probe) {
      const owner = worktreeProjectOwner(project)
      if (owner && liveWorktree(owner)) continue
    }
    const used = containers.some(
      (c) => (c.networks ?? []).includes(net.name) || (project !== '' && c.project === project),
    )
    if (used) continue
    if (!(net.createdMs > 0) || now - net.createdMs < policy.orphanNetworkMinAgeMs) continue
    rows.push({
      kind: 'orphan-network',
      id: net.name,
      networkId: net.id,
      path: '',
      bytes: 0,
      ageMs: now - net.createdMs,
      running: false,
      dirty: false,
      unpushed: false,
      kept: false,
      current: false,
      primary: false,
    })
  }
  return rows
}

/** An app database's container name: `talaria-appdb-<instance>-<slug>`
 *  (ui/src/server/app-db.ts appDbContainer). */
export const APP_DB_NAME_RE = /^talaria-appdb-[a-z0-9][a-z0-9_.-]*$/

/** Where an app database keeps its cluster inside its container: the bind
 *  mount whose host side says which checkout or box the database is. */
export const APP_DB_DATA = '/var/lib/postgresql/data'

/**
 * Which checkout or box an app database belongs to, read from the host
 * directory its cluster is mounted from — `<owner>/app-data/<slug>/pg` for a
 * checkout, `<box>/state/app-data/<slug>/pg` for a box — or null when the
 * mount is anywhere else (an image install's volume, a TALARIA_APP_DATA_DIR
 * outside both roots). Null is never swept: what the sweep cannot place is
 * not its to remove. Paths are compared as given; the caller spells both
 * sides physically.
 *
 *   { kind: 'box', name, dir }       under `<devboxHome>/<name>/` (not shared/)
 *   { kind: 'checkout', dir }        under `<siblingParent>/talaria-<x>/`
 */
export function appDbOwner(source, { siblingParent, devboxHome }) {
  const under = (root, p) => {
    if (!root || !p) return null
    const r = root.endsWith(sep) ? root : root + sep
    if (!p.startsWith(r)) return null
    const first = p.slice(r.length).split(sep)[0]
    return first || null
  }
  const box = under(devboxHome, source)
  if (box) return box === 'shared' || box.startsWith('.') ? null : { kind: 'box', name: box, dir: join(devboxHome, box) }
  const checkout = under(siblingParent, source)
  if (checkout && /^talaria-[a-z0-9][a-z0-9_.-]*$/.test(checkout)) return { kind: 'checkout', dir: join(siblingParent, checkout) }
  return null
}

/**
 * App databases whose checkout or box is gone, as sweep rows (follow-up N9).
 * Pure: the inventory hands it what docker said.
 *
 *   dbs           [{ id, name, project, running, createdMs, source }] —
 *                 every `talaria-appdb-*` container, running or stopped,
 *                 with the host side of its APP_DB_DATA mount
 *   roots         { siblingParent, devboxHome } in their physical spelling
 *   liveCheckout(dir) — is that directory still a checkout (a listed git
 *                 worktree, or a .git in it)
 *   liveBox(name)     — does that box's box.env exist
 *
 * A box removed without `talaria box rm`, or a worktree removed while its
 * app ran, leaves its app databases running (`restart: unless-stopped`):
 * each one keeps a postgres in memory and holds the network it joined, so
 * that network never reads as an orphan either. Only a database the sweep
 * can place (appDbOwner) and whose owner is gone is a row. Defaults are the
 * safe reading: a caller that cannot say keeps everything.
 */
export function orphanAppDbs({ dbs, roots, liveCheckout = () => true, liveBox = () => true, now = Date.now() }) {
  const rows = []
  for (const db of dbs) {
    if (!APP_DB_NAME_RE.test(db?.name ?? '') || !db.source) continue
    const owner = appDbOwner(db.source, roots)
    if (!owner) continue
    if (owner.kind === 'box' ? liveBox(owner.name) : liveCheckout(owner.dir)) continue
    rows.push({
      kind: 'orphan-appdb',
      id: db.name,
      containerId: db.id,
      project: db.project ?? '',
      source: db.source,
      why: `${owner.kind === 'box' ? `box ${owner.name}` : owner.dir} is gone; its data stays at ${db.source}`,
      path: '',
      bytes: 0,
      ageMs: db.createdMs > 0 ? now - db.createdMs : 0,
      running: Boolean(db.running),
      dirty: false,
      unpushed: false,
      kept: false,
      current: false,
      primary: false,
    })
  }
  return rows
}

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
  if (artifact.kind === 'orphan-compose' || artifact.kind === 'orphan-network' || artifact.kind === 'orphan-appdb') return 'remove'
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
  // A network: a `talaria-` or `devbox-` name (so never the shared
  // `talaria` network), and a docker id to remove it by — never a path.
  if (artifact.kind === 'orphan-network') {
    return (
      /^(talaria|devbox)-[a-z0-9][a-z0-9_.-]*$/.test(artifact.id) &&
      /^[0-9a-f]{12,64}$/.test(artifact.networkId ?? '') &&
      !artifact.path
    )
  }
  // An app database: its container, by an app database's name and a docker
  // id. Never a path — its data directory stays (postgres's uid owns it).
  if (artifact.kind === 'orphan-appdb') {
    return APP_DB_NAME_RE.test(artifact.id) && /^[0-9a-f]{12,64}$/.test(artifact.containerId ?? '') && !artifact.path
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
    for (const a of toRemove) lines.push(`  ${a.kind} ${a.id}  ${formatBytes(a.bytes)}  ${ageDays(a.ageMs)}  ${a.path || a.why || ''}`)
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

/** An artifact's age as the report prints it: days, then hours, then — for
 *  the orphan networks, which qualify at ten minutes — minutes. */
export function ageDays(ms) {
  if (!Number.isFinite(ms) || ms < 0) return '?'
  const h = ms / (60 * 60 * 1000)
  if (h >= 24) return `${(h / 24).toFixed(0)}d`
  if (h >= 1) return `${Math.round(h)}h`
  return `${Math.max(1, Math.round(ms / 60_000))}m`
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
export function inventory(root, now = Date.now(), { scanNetworks = true } = {}) {
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
      const owner = worktreeProjectOwner(name)
      if (owner) {
        if (!existsSync(join(sibling, `talaria-${owner}`)) && !seenWorktree.has(owner)) {
          artifacts.push(orphan(name))
        }
        continue
      }
      // A project ending in -fleet is that box's fleet network, or a box whose
      // own name ends in -fleet. Orphan only when neither checkout exists.
      const candidates = boxProjectOwners(name)
      if (!candidates || candidates.length === 0) continue
      if (candidates.every((n) => !existsSync(join(home, n, 'box.env')))) artifacts.push(orphan(name))
    }
  }

  // Not from the stop gate: orphan networks are --apply's alone and weigh
  // nothing on disk, so the gate would only pay three more docker reads —
  // and a daemon busy with other stacks can hold a network read for a minute.
  if (projects && scanNetworks) {
    const sibling = dirname(primary)
    const scan = networkFacts()
    if (scan) {
      artifacts.push(
        ...orphanNetworks({
          ...scan,
          liveWorktree: (owner) => seenWorktree.has(owner) || existsSync(join(sibling, `talaria-${owner}`)),
          liveBox: (name) => existsSync(join(home, name, 'box.env')),
          now,
        }),
      )
    } else {
      notes.push('docker network scan failed — orphan networks not scanned')
    }
  }

  // App databases a gone checkout or box left running (N9): --apply's alone,
  // like the networks, so the gate does not pay for the docker reads.
  if (projects && scanNetworks) {
    const dbs = appDbFacts()
    if (dbs) {
      const listedPaths = new Set(listed.map((w) => real(w.path)))
      artifacts.push(
        ...orphanAppDbs({
          dbs,
          roots: { siblingParent: physical(dirname(primary)), devboxHome: physical(home) },
          liveCheckout: (dir) => listedPaths.has(dir) || existsSync(join(dir, '.git')),
          liveBox: (name) => existsSync(join(home, name, 'box.env')),
          now,
        }),
      )
    } else {
      notes.push('docker app database scan failed — orphan app databases not scanned')
    }
  }

  return { artifacts, notes, roots: { primary, current, siblingParent: dirname(primary), devboxHome: home, tmp: process.env.TALARIA_CLEANUP_TMP || tmpdir() } }
}

/** `p` (absolute) in its physical spelling, even when it no longer exists:
 *  the nearest ancestor that does is realpath'd and the rest re-appended. A
 *  gone checkout's mount must compare against a realpath'd root (cli
 *  stack.ts `physical`, the same rule). */
export function physical(p) {
  const rest = []
  let d = resolve(p)
  for (let i = 0; i < 64; i++) {
    try {
      return join(realpathSync(d), ...rest.reverse())
    } catch {
      const parent = dirname(d)
      if (parent === d) return resolve(p)
      rest.push(basename(d))
      d = parent
    }
  }
  return resolve(p)
}

/** Every `talaria-appdb-*` container with what orphanAppDbs reads, or null
 *  when docker did not answer (fail closed: no rows). Two reads: the names,
 *  then one inspect of all of them. */
function appDbFacts() {
  const ps = dockerOut(['ps', '-a', '--filter', 'name=talaria-appdb-', '--format', '{{.ID}}\t{{.Names}}'])
  if (ps === null) return null
  const ids = ps
    .split('\n')
    .map((l) => l.split('\t'))
    .filter(([id, name]) => id && APP_DB_NAME_RE.test((name ?? '').trim()))
    .map(([id]) => id.trim())
  if (ids.length === 0) return []
  const out = dockerOut(['inspect', ...ids])
  if (out === null) return null
  try {
    return JSON.parse(out).map((c) => {
      const mount = (c.Mounts ?? []).find((m) => m.Destination === APP_DB_DATA)
      return {
        id: String(c.Id ?? ''),
        name: String(c.Name ?? '').replace(/^\//, ''),
        project: c.Config?.Labels?.['com.docker.compose.project'] ?? '',
        running: c.State?.Running === true,
        createdMs: Date.parse(c.Created ?? '') || 0,
        source: mount?.Type === 'bind' && mount.Source ? physical(mount.Source) : '',
      }
    })
  } catch {
    return null
  }
}

/** One docker read, or null — never '' for a failure. The network scan must
 *  fail CLOSED: an empty container list read as "nothing attached" would make
 *  every network look orphaned. */
function dockerOut(args, timeoutMs = 30_000) {
  try {
    return execFileSync('docker', args, { encoding: 'utf8', timeout: timeoutMs, stdio: ['ignore', 'pipe', 'pipe'] })
  } catch {
    return null
  }
}

/** null when the docker command succeeded, else its stderr (or the error)
 *  in one line. */
function dockerErr(args, timeoutMs = 30_000) {
  try {
    execFileSync('docker', args, { encoding: 'utf8', timeout: timeoutMs, stdio: ['ignore', 'pipe', 'pipe'] })
    return null
  } catch (e) {
    return String(e?.stderr || e?.message || e).trim().split('\n')[0]
  }
}

/** True only when docker says the container does not exist — a timeout or
 *  a daemon that did not answer is not "gone". */
function containerGone(id) {
  const why = dockerErr(['inspect', '--format', '{{.Id}}', id])
  return why !== null && /no such (object|container)/i.test(why)
}

/** True only when docker says the network does not exist — a timeout or a
 *  daemon that did not answer is not "gone". */
function networkGone(id) {
  try {
    execFileSync('docker', ['network', 'inspect', '--format', '{{.Id}}', id], {
      encoding: 'utf8',
      timeout: 30_000,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    return false
  } catch (e) {
    return /not found|no such network/i.test(String(e?.stderr ?? ''))
  }
}

function jsonLines(text) {
  return text
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean)
    .map((l) => JSON.parse(l))
}

/** What orphanNetworks needs, in three docker reads — compose-made networks,
 *  pool probes, and every container's project and networks — or null when
 *  any read fails. */
function networkFacts() {
  const composed = dockerOut(['network', 'ls', '--filter', 'label=com.docker.compose.project', '--format', '{{json .}}'])
  const probes = dockerOut(['network', 'ls', '--filter', 'label=talaria.pool-probe', '--format', '{{json .}}'])
  const ps = dockerOut(['ps', '-a', '--format', '{{.Label "com.docker.compose.project"}}\t{{.Networks}}'])
  if (composed === null || probes === null || ps === null) return null
  try {
    const networks = [...jsonLines(composed), ...jsonLines(probes)].map((n) => ({
      id: String(n.ID ?? ''),
      name: String(n.Name ?? ''),
      labels: parseLabels(n.Labels),
      createdMs: parseDockerTime(n.CreatedAt),
    }))
    const containers = ps
      .split('\n')
      .filter((l) => l.trim())
      .map((l) => {
        const [project, nets] = l.split('\t')
        return { project: (project ?? '').trim(), networks: (nets ?? '').split(',').map((n) => n.trim()).filter(Boolean) }
      })
    return { networks, containers }
  } catch {
    return null
  }
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
  if (artifact.kind === 'orphan-network') {
    // Asked again at the moment of removal: a stack may have come up on it
    // since the scan. Anything attached, running or stopped, and it stays.
    const attached = dockerOut(['ps', '-aq', '--filter', `network=${artifact.networkId}`])
    if (attached === null || attached.trim() !== '') {
      throw new Error(`network ${artifact.id} is in use again (or docker did not answer) — left in place`)
    }
    // A daemon busy with other projects can hold a network operation past
    // the client's timeout and still carry it out, so a failed rm asks
    // whether the network is still there before it reports a failure.
    if (dockerOut(['network', 'rm', artifact.networkId], 120_000) === null && !networkGone(artifact.networkId)) {
      throw new Error(`docker network rm ${artifact.id} failed`)
    }
    return
  }
  if (artifact.kind === 'orphan-appdb') {
    // Asked again at the moment of removal, by id: it must still be that
    // database, on that data directory. Anything else and it stays.
    const out = dockerOut(['inspect', artifact.containerId])
    let now
    try {
      now = out === null ? null : JSON.parse(out)[0]
    } catch {
      now = null
    }
    const mount = (now?.Mounts ?? []).find((m) => m.Destination === APP_DB_DATA)
    if (!now || String(now.Name ?? '').replace(/^\//, '') !== artifact.id || !mount?.Source || physical(mount.Source) !== artifact.source) {
      throw new Error(`app database ${artifact.id} changed since the scan (or docker did not answer) — left in place`)
    }
    // A daemon busy with other stacks can fail a removal half way (the
    // container is left `Dead`) or answer after the client gave up, so a
    // failed rm asks whether the container is still there, and tries once
    // more before it reports docker's own words.
    let why = ''
    for (let attempt = 0; attempt < 2; attempt++) {
      why = dockerErr(['rm', '-f', artifact.containerId], 120_000)
      if (why === null || containerGone(artifact.containerId)) break
    }
    if (why !== null && !containerGone(artifact.containerId)) throw new Error(`docker rm ${artifact.id} failed: ${why}`)
    // Its own compose project (app-db.ts runs one per database, named for
    // it) may still hold the `<name>_default` network an older app DB was
    // made with. An external network it joined (a box's) is not labelled
    // with this project and stays for the network sweep.
    if (artifact.project === artifact.id) dockerByLabel(artifact.project)
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
    inventoried = inventory(root, Date.now(), { scanNetworks: mode !== 'gate' })
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
  // exitCode, never exit(): a write to a pipe is asynchronous, and exit()
  // drops whatever has not drained. A host with many stacks has a report
  // over 64 KiB, so a piped `talaria cleanup` (an agent reading it), the
  // stop gate's stderr and `--json` all ended at byte 65536, and the rows
  // appended last never arrived.
  process.exitCode = v.exit
}

if (invokedDirectly()) main()
