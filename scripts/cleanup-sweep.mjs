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
//   - with a MERGED worktree's stack or checkout, its offload refs on origin
//     (`ci/wt/<name>`, `ci/full/wt/<name>`: `bun run gate:remote` keeps one
//     after a red). Never fatal; ci-offload.yml's daily prune is the backstop
//   - a side worktree's build dir untouched for POLICY.staleTargetMs
//   - a devbox the same way (../devboxes/<name>, not the shared tools layer)
//   - /tmp/talaria-* (or $TMPDIR) older than POLICY.tmpStaleMs
//   - a compose project named talaria-wt-* or devbox-* whose directory is gone.
//     A `-fleet` name has two readings (a stack's agents, or the stack of a
//     worktree or box named `<x>-fleet`), and either one with a checkout
//     keeps it
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
//   - (--apply only) an MCP package's container (`talaria-mcp-*`) and its
//     package network whose owner labels (api/crates/talaria-mcp/src/pkg.rs
//     `owner_labels`) name a sibling `talaria-<name>` checkout that is gone,
//     or a box whose box.env is gone (UB3). A network goes only once nothing
//     but such containers is attached to it
//
// WHAT IT WILL NOT:
//   - the primary checkout, the checkout this process is running in
//   - anything with a .talaria-keep file in its root, and the compose
//     projects, volumes and networks of the worktree or box it sits in. A
//     `../talaria-<name>` directory holding only a .talaria-keep holds that
//     worktree's stack after the checkout is gone. Every removal reads the
//     keep again at the moment it acts
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
//   - an MCP package container or network with no owner labels (made before
//     pkg.rs wrote them): the report names it, a human decides; nor one the
//     primary or an image install owns, whatever its name says
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
  readFileSync,
  readdirSync,
  renameSync,
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
  // A side worktree's build dir used within this long is never dropped for
  // a finished pull request or for disk pressure: an agent between two
  // builds is still using it. The cargo lock says whether one is building
  // right now; this says whether one was a moment ago.
  targetIdleMs: 60 * 60 * 1000,
  // Over the line, --apply drops at most this many idle build caches in one
  // run, and stops early once a drop frees less than pressureMinFreedBytes:
  // then the pressure is not build caches (docker, a transient write, or
  // seeded caches whose extents another tree still shares), and dropping
  // more would only cost rebuilds.
  pressureDropMax: 3,
  pressureMinFreedBytes: 512 * 1024 * 1024,
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
/** The worktrees a `talaria-wt-*` compose project may belong to, or null for
 *  a project that is not a worktree's. `talaria worktree <name>` names its
 *  stack `talaria-wt-<name>`, and the agents that worktree runs live in a
 *  SECOND project, `talaria-wt-<name>-fleet` (the api's
 *  TALARIA_FLEET_PROJECT). But `talaria worktree` takes any kebab name, and a
 *  worktree named `<x>-fleet` runs its own Postgres and Redis in
 *  `talaria-wt-<x>-fleet` too. So both readings are candidates, and the
 *  project is a live worktree's while EITHER one has a checkout, which is
 *  boxProjectOwners' rule. Each single reading broke something. Read bare,
 *  a worktree's agents parsed as the worktree `<name>-fleet`, and the gate
 *  removed a live agent container at every turn boundary. Read stripped,
 *  the infra of the live worktree `rel-fleet` parsed as the missing worktree
 *  `rel`, and the gate removed its containers and volumes (2026-11-02: that
 *  stack's database and redis were lost). */
export function worktreeProjectOwners(projectName) {
  const wt = /^talaria-wt-([a-z0-9][a-z0-9-]*)$/.exec(projectName)
  if (!wt) return null
  const raw = wt[1]
  const stripped = raw.endsWith('-fleet') ? raw.slice(0, -'-fleet'.length) : null
  return [raw, stripped].filter(Boolean)
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

/**
 * Compose projects whose worktree or box is gone, as sweep rows. Pure: the
 * inventory hands it `docker compose ls` and two predicates.
 *
 *   projects        [{ Name }] — every compose project, running or stopped
 *   liveWorktree(n) — worktree `n` has a checkout
 *   liveBox(n)      — box `n` has a checkout
 *
 * A project is an orphan only when NO worktree or box it could belong to is
 * live (worktreeProjectOwners, boxProjectOwners). A project that is neither
 * kind is not the sweep's, and neither is the box tools layer. Both
 * predicates default to "live", so a caller that cannot say removes nothing.
 * The inventory's predicates count a `.talaria-keep` as a checkout: a human
 * who touches one in `../talaria-<n>` or `<devboxes>/<n>` keeps the stack,
 * even after its checkout is gone. That is how a stack is held while its
 * evidence is still needed.
 */
export function orphanProjects({ projects, liveWorktree = () => true, liveBox = () => true }) {
  const rows = []
  for (const row of projects ?? []) {
    const name = row?.Name ?? ''
    const worktrees = worktreeProjectOwners(name)
    const owners = worktrees ?? boxProjectOwners(name)
    if (!owners || owners.length === 0) continue
    const live = worktrees ? liveWorktree : liveBox
    if (owners.some((n) => live(n))) continue
    rows.push(orphan(name))
  }
  return rows
}

/**
 * What still vouches for a compose project, read from disk at the moment of
 * removal: the path of a checkout, a box.env or a `.talaria-keep`, or null
 * when nothing does. The scan and the removal are not one instant. The gate
 * sizes every candidate between them, and a `talaria worktree` or a
 * `.talaria-keep` can land in that gap. Nothing is removed on the scan's
 * word alone. A `devbox-*` name with no box it could be (the tools layer)
 * always vouches for itself. `exists` is injectable for the test.
 */
export function projectVouchedBy(projectName, { siblingParent, devboxHome }, exists = existsSync) {
  const worktrees = worktreeProjectOwners(projectName)
  if (worktrees) {
    for (const n of worktrees) {
      // A `.talaria-keep` lives inside the directory, so a directory that
      // exists covers both: a checkout, or a placeholder holding only the keep.
      const dir = join(siblingParent, `talaria-${n}`)
      if (exists(dir)) return dir
    }
    return null
  }
  const boxes = boxProjectOwners(projectName)
  if (!boxes) return null
  if (boxes.length === 0) return 'the box tools layer'
  for (const n of boxes) {
    for (const f of ['box.env', '.talaria-keep']) {
      const p = join(devboxHome, n, f)
      if (exists(p)) return p
    }
  }
  return null
}

/** Is box `name` live? Its box.env exists, or a `.talaria-keep` holds it. */
export function boxStands(devboxHome, name, exists = existsSync) {
  return exists(join(devboxHome, name, 'box.env')) || exists(join(devboxHome, name, '.talaria-keep'))
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
 *   liveWorktree(name) — does that worktree's checkout exist. Asked for
 *               each name the project could belong to (worktreeProjectOwners)
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
      // Either reading of a `-fleet` name keeps it (worktreeProjectOwners).
      const owners = worktreeProjectOwners(project)
      if (owners && owners.some((n) => liveWorktree(n))) continue
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
 *   liveCheckout(dir) — is that checkout still there (checkoutStands)
 *   liveBox(name)     — does that box's box.env exist
 *
 * A box removed without `talaria box rm`, or a worktree removed while its
 * app ran, leaves its app databases running (`restart: unless-stopped`):
 * each one keeps a postgres in memory and holds the network it joined, so
 * that network never reads as an orphan either. Only a database the sweep
 * can place (appDbOwner) and whose owner is gone is a row. Defaults are the
 * safe reading: a caller that cannot say keeps everything.
 */
/**
 * Is the checkout an app database was placed under (appDbOwner) still there?
 * Only a directory that is gone makes its databases orphans. A sibling
 * `talaria-<x>` still on disk keeps them whether or not git lists it or a
 * `.git` sits in it (a worktree half removed, a copy made by hand): a
 * running database under a directory that exists is not the sweep's to
 * remove.
 */
export function checkoutStands(dir) {
  return existsSync(dir)
}

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

/** An MCP package's container (`talaria-mcp-<instance>-<server>`) or its
 *  network (`talaria-mcp-<instance>`): api/crates/talaria-mcp/src/pkg.rs
 *  `container_name` and `pkg_network`. */
export const MCP_PKG_NAME_RE = /^talaria-mcp-[a-z0-9][a-z0-9_.-]*$/

/** The owner labels pkg.rs `owner_labels` writes on both. */
export const MCP_LABEL = { instance: 'talaria.mcp.instance', owner: 'talaria.mcp.owner', checkout: 'talaria.mcp.checkout' }

/**
 * Whose an MCP package container or network is, from its owner labels, or
 * null when the sweep cannot place it: no labels (made before pkg.rs wrote
 * them), the primary's or an image install's (`owner=instance`), a worktree
 * label that names no checkout, or a checkout that is not a sibling
 * `talaria-<x>` of the primary (appDbOwner's rule). Null is never swept.
 * Paths compare as given; the caller spells both sides physically.
 *
 *   { kind: 'box', name }        owner=devbox; the instance is the box's name
 *   { kind: 'checkout', dir }    owner=worktree, under `<siblingParent>/talaria-<x>`
 */
export function mcpPkgOwner(labels, { siblingParent }) {
  const owner = labels?.[MCP_LABEL.owner]
  const instance = labels?.[MCP_LABEL.instance] ?? ''
  if (owner === 'devbox') {
    return /^[a-z0-9][a-z0-9_.-]*$/.test(instance) && instance !== 'shared' ? { kind: 'box', name: instance } : null
  }
  if (owner === 'worktree') {
    const dir = labels?.[MCP_LABEL.checkout] ?? ''
    if (!dir || !siblingParent || dirname(dir) !== siblingParent) return null
    return /^talaria-[a-z0-9][a-z0-9_.-]*$/.test(basename(dir)) ? { kind: 'checkout', dir } : null
  }
  return null
}

/**
 * MCP package containers and networks whose owner is gone, as sweep rows
 * (UB3). Pure: the inventory hands it what docker said.
 *
 *   containers  [{ id, name, labels, running, createdMs }] — every
 *               `talaria-mcp-*` container, running or stopped
 *   networks    [{ id, name, labels, createdMs, attached: [container id] }]
 *               — every `talaria-mcp-*` network, with EVERY container that
 *               names it, running or stopped (a stopped one could not start
 *               again without it)
 *   roots       { siblingParent } in its physical spelling
 *   liveCheckout(dir), liveBox(name) — default: alive, so a caller that
 *               cannot say keeps everything
 *
 * A worktree removed while a package ran, or a box deleted without `box rm`,
 * leaves its packages running (`docker run -d`, no compose project to take
 * them down) and their network holding a subnet. A container is a row when
 * its owner is placed and gone. A network is a row when its owner is placed
 * and gone, it is older than the networks' minimum age, and nothing is
 * attached to it but containers this same pass removes. Container rows come
 * first, so `--apply` empties a network before it removes it. `unmarked`
 * names what carries no owner label at all, for the report.
 */
export function orphanMcpPackages({ containers, networks, roots, liveCheckout = () => true, liveBox = () => true, now = Date.now(), policy = POLICY }) {
  const rows = []
  const unmarked = []
  const goneOwner = (labels) => {
    const o = mcpPkgOwner(labels, roots)
    if (!o) return null
    return (o.kind === 'box' ? liveBox(o.name) : liveCheckout(o.dir)) ? null : o
  }
  const whose = (o) => (o.kind === 'box' ? `box ${o.name}` : o.dir)
  const base = { path: '', bytes: 0, dirty: false, unpushed: false, kept: false, current: false, primary: false }
  const going = new Set()
  for (const c of containers) {
    if (!MCP_PKG_NAME_RE.test(c?.name ?? '')) continue
    if (!c.labels?.[MCP_LABEL.owner]) {
      unmarked.push(`container ${c.name}`)
      continue
    }
    const o = goneOwner(c.labels)
    if (!o) continue
    going.add(c.id)
    rows.push({
      ...base,
      kind: 'orphan-mcp-container',
      id: c.name,
      containerId: c.id,
      owner: ownerKey(c.labels),
      why: `${whose(o)} is gone`,
      ageMs: c.createdMs > 0 ? now - c.createdMs : 0,
      running: Boolean(c.running),
    })
  }
  for (const n of networks) {
    if (!MCP_PKG_NAME_RE.test(n?.name ?? '')) continue
    if (!n.labels?.[MCP_LABEL.owner]) {
      unmarked.push(`network ${n.name}`)
      continue
    }
    const o = goneOwner(n.labels)
    if (!o) continue
    if ((n.attached ?? []).some((id) => !going.has(id))) continue
    if (!(n.createdMs > 0) || now - n.createdMs < policy.orphanNetworkMinAgeMs) continue
    rows.push({
      ...base,
      kind: 'orphan-mcp-network',
      id: n.name,
      networkId: n.id,
      owner: ownerKey(n.labels),
      why: `${whose(o)} is gone`,
      ageMs: now - n.createdMs,
      running: false,
    })
  }
  return { rows, unmarked }
}

/** The owner labels as one string, so the executor can tell that what it is
 *  about to remove still says what the scan read. */
export function ownerKey(labels) {
  return [MCP_LABEL.owner, MCP_LABEL.instance, MCP_LABEL.checkout].map((k) => labels?.[k] ?? '').join('|')
}

export function classify(artifact, policy = POLICY) {
  if (artifact.kind === 'target') {
    const big = artifact.bytes >= policy.oversizedTargetBytes ? 'flag' : 'keep'
    // The primary's build dir is a warm cache: named when oversized, never
    // removed on a timer. Nor is a checkout `talaria worktree` did not make
    // (`foreign`): it is reported, so the disk it holds is seen, and that is all.
    // Half removed already (dropTarget was interrupted): it goes once no
    // cargo holds it.
    if (artifact.dropping) return artifact.busy === false ? 'remove' : 'flag'
    if (artifact.primary || artifact.current || artifact.kept || artifact.foreign) return big
    // A cargo building in it: never.
    if (artifact.busy === true) return big
    // Nor one whose stack runs: an api serving from it, or about to rebuild.
    if (artifact.stackRunning) return big
    // A lock check that cannot be trusted (no /proc/locks off Linux; btrfs,
    // whose stat device is not the one /proc/locks prints) never deletes:
    // what would have gone is flagged for a human instead.
    const untrusted = artifact.busy === null
    // A side worktree's is spent once the tree has sat.
    if (artifact.ageMs >= policy.staleTargetMs) return untrusted ? 'flag' : 'remove'
    // Its pull request is finished — merged, or closed unmerged — at the
    // commit this checkout sits on. The cache goes first: the checkout keeps
    // its own rules (a closed PR's tree is not removed, a merged one waits
    // for --apply), and a rebuild is a reseed (`talaria seed-target`). Not
    // while it was used in the last POLICY.targetIdleMs or its stack runs,
    // and a tree with work no remote has is a human's call, as for the
    // checkout itself.
    if ((artifact.pr === 'merged' || artifact.pr === 'closed') && targetIdleBy(artifact, policy)) {
      return artifact.dirty || artifact.unpushed || untrusted ? 'flag' : 'remove'
    }
    return big
  }
  if (
    artifact.kind === 'orphan-compose' ||
    artifact.kind === 'orphan-network' ||
    artifact.kind === 'orphan-appdb' ||
    artifact.kind === 'orphan-mcp-container' ||
    artifact.kind === 'orphan-mcp-network'
  ) {
    return 'remove'
  }
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

/** Idle: no cargo lock held (checked, not guessed), no stack running, and
 *  not built in for POLICY.targetIdleMs. */
export function targetIdle(artifact, policy = POLICY) {
  return artifact.busy === false && targetIdleBy(artifact, policy)
}

/** The idle test without the lock half: no stack, not built in lately. */
function targetIdleBy(artifact, policy) {
  return !artifact.stackRunning && artifact.ageMs >= policy.targetIdleMs
}

/**
 * Over the line, the caches --apply may drop beyond the removable set, least
 * recently used first: an idle, clean, pushed side worktree's build dir, kept
 * until now because its pull request is open, it has none yet, or a
 * `.talaria-keep` holds the tree. The keep holds the checkout; under
 * pressure its cache is the cheapest thing on the disk to rebuild. Never the
 * primary's, the current checkout's, a checkout `talaria worktree` did not
 * make, one a cargo is building in, or one in a tree with work no remote has.
 * Pure: `classified` carries `action`. The executor removes them one at a
 * time and stops once the disk is back under the line, because a seeded
 * target shares its extents and its `du` says more than its removal frees.
 */
export function pressureDrops(classified, pressure, policy = POLICY) {
  if (!(pressure.usedPct >= policy.pressurePct)) return []
  return classified
    .filter(
      (a) =>
        a.kind === 'target' &&
        a.action !== 'remove' &&
        !a.primary &&
        !a.current &&
        !a.foreign &&
        !a.kept &&
        !a.dirty &&
        !a.unpushed &&
        a.inspected === true &&
        targetIdle(a, policy),
    )
    // A non-seeded cache first: removing a seeded one frees only what it
    // does not share (a third of its `du` here), and its rebuild writes all
    // of it back. Least recently used first within each.
    .sort((x, y) => Number(Boolean(x.seeded)) - Number(Boolean(y.seeded)) || y.ageMs - x.ageMs)
}

/** Over the line, the idle caches a `.talaria-keep` holds. The keep is
 *  absolute (a human fenced the tree off), so these are never dropped; the
 *  report names them, flagged, so the human can decide. Pure. */
export function pressureKept(classified, pressure, policy = POLICY) {
  if (!(pressure.usedPct >= policy.pressurePct)) return []
  return classified.filter(
    (a) => a.kind === 'target' && a.kept && a.action !== 'remove' && !a.primary && !a.current && !a.foreign && targetIdle(a, policy),
  )
}

/** What this mode is allowed to delete. --gate only takes garbage that has
 *  no checkout left (orphans) or was scratch by name (old tmp). Worktrees
 *  and boxes wait for --apply, so a stop hook never surprises a parked tree. */
export function selectRemoval(classified, mode) {
  const removable = classified.filter((a) => a.action === 'remove')
  // A build dir goes before its checkout: it is the largest part of it, so a
  // run cut short has still freed the most.
  if (mode === 'apply') return [...removable.filter((a) => a.kind === 'target'), ...removable.filter((a) => a.kind !== 'target')]
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
  // A removable checkout's size already holds its build dirs: count them once.
  const goingTrees = new Set(removable.filter((a) => a.kind === 'worktree').map((a) => a.path))
  const removableBytes = removable
    .filter((a) => !(a.kind === 'target' && a.owner && goingTrees.has(a.owner)))
    .reduce((n, a) => n + (a.bytes || 0), 0)
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
  // An MCP package's container or network: by its name and a docker id.
  if (artifact.kind === 'orphan-mcp-container') {
    return MCP_PKG_NAME_RE.test(artifact.id) && /^[0-9a-f]{12,64}$/.test(artifact.containerId ?? '') && !artifact.path
  }
  if (artifact.kind === 'orphan-mcp-network') {
    return MCP_PKG_NAME_RE.test(artifact.id) && /^[0-9a-f]{12,64}$/.test(artifact.networkId ?? '') && !artifact.path
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
    // A `.target-dropping-*` beside where the target was is the same dir,
    // part way through its removal.
    const asTarget = DROPPING_RE.test(basename(path)) ? join(dirname(path), 'target') : path
    const owner = TARGET_RELS.map((rel) => trimSuffix(asTarget, rel)).find(Boolean)
    if (!owner) return false
    if (roots.primary && owner === resolve(roots.primary)) return false
    if (roots.current && owner === resolve(roots.current)) return false
    return basename(owner).startsWith('talaria-') && dirname(owner) === resolve(roots.siblingParent)
  }
  return false
}

/** The two build dirs this repo produces, relative to a checkout root. */
export const TARGET_RELS = ['api/target', 'desktop/src-tauri/target']

/** The lock files cargo takes on each profile dir of a target (1.97:
 *  `.cargo-lock` shared, the other two exclusive). cli/src/targetseed.ts
 *  reads the same three. */
export const CARGO_LOCK_FILES = ['.cargo-lock', '.cargo-build-lock', '.cargo-artifact-lock']

/** A device number as /proc/locks spells it (`fc:00`). */
export function procLocksDev(dev) {
  const major = Math.floor(dev / 256) & 0xfff
  const minor = (dev & 0xff) | ((Math.floor(dev / 4096) >>> 0) & ~0xff & 0xfffff)
  return `${major.toString(16).padStart(2, '0')}:${minor.toString(16).padStart(2, '0')}`
}

/** `dev:inode` of every flock in /proc/locks, holders and waiters alike (a
 *  cargo waiting on a lock is about to build). Pure. */
export function heldFlocks(procLocks) {
  const out = new Set()
  for (const line of String(procLocks ?? '').split('\n')) {
    const m = /^\s*\d+:\s+(?:->\s+)?FLOCK\s+\S+\s+\S+\s+\d+\s+([0-9a-f]+:[0-9a-f]+):(\d+)\s/.exec(line)
    if (m) out.add(`${m[1]}:${m[2]}`)
  }
  return out
}

/**
 * Is a cargo building in this target? true, false, or null when it cannot be
 * told (no /proc/locks: not Linux) — and null is never "idle". Read-only: it
 * takes no lock, so asking never slows another session's build. Every
 * profile dir is asked (debug, release, a target triple's).
 */
export function targetBusy(targetDir, procLocks = readProcLocks(), fsType = fsTypeOf(targetDir)) {
  if (procLocks === null) return null
  // btrfs: stat() answers the subvolume's anonymous device, /proc/locks the
  // superblock's, so no key ever matches and every target would read idle.
  if (fsType === BTRFS_SUPER_MAGIC) return null
  const held = heldFlocks(procLocks)
  const profiles = []
  const walk = (d, depth) => {
    let entries
    try {
      entries = readdirSync(d, { withFileTypes: true })
    } catch {
      return
    }
    if (entries.some((e) => CARGO_LOCK_FILES.includes(e.name))) profiles.push(d)
    if (depth < 1) for (const e of entries) if (e.isDirectory() && !e.name.startsWith('.')) walk(join(d, e.name), depth + 1)
  }
  walk(targetDir, 0)
  for (const p of profiles) {
    for (const f of CARGO_LOCK_FILES) {
      try {
        // bigint: an inode past 2^53 keeps every digit
        const st = statSync(join(p, f), { bigint: true })
        if (held.has(`${procLocksDev(Number(st.dev))}:${st.ino}`)) return true
      } catch {
        // no such lock file in this profile
      }
    }
  }
  return false
}

/** statfs f_type of btrfs. */
export const BTRFS_SUPER_MAGIC = 0x9123683e

function fsTypeOf(path) {
  try {
    return Number(statfsSync(path).type)
  } catch {
    return null
  }
}

function readProcLocks() {
  try {
    return readFileSync('/proc/locks', 'utf8')
  } catch {
    return null
  }
}

/** When a build last wrote into a target: the newest of the target, its
 *  debug profile, and the two dirs every build adds to. The target's own
 *  mtime moves only when a profile dir is created, so alone it is no clock. */
export function targetLastUsedMs(targetDir) {
  let max = 0
  for (const p of [targetDir, join(targetDir, 'debug'), join(targetDir, 'debug', 'deps'), join(targetDir, 'debug', '.fingerprint')]) {
    max = Math.max(max, statMs(p))
  }
  return max
}

/**
 * Where this checkout's work stands on GitHub: 'merged' or 'closed' when a
 * pull request of its branch finished AT this checkout's HEAD (a later
 * commit is new work), 'open' when one is open, else null. Pure: `prs` is
 * `gh pr list --state all --json number,state,headRefName,headRefOid`. A
 * recut branch reusing an old name sits on another commit, so an old PR of
 * the same name never finishes it (mergedAtHead's rule).
 */
export function prStateAt(prs, branch, head) {
  if (!branch || !head || !Array.isArray(prs)) return null
  const mine = prs.filter((p) => p?.headRefName === branch)
  if (mine.some((p) => p.state === 'OPEN')) return 'open'
  if (mine.some((p) => p.state === 'MERGED' && p.headRefOid === head)) return 'merged'
  if (mine.some((p) => p.state === 'CLOSED' && p.headRefOid === head)) return 'closed'
  return null
}

/** The repo's recent pull requests in ONE `gh` call, or null (no gh, no
 *  auth, no network: every state then reads unknown, and unknown keeps). */
function recentPrs(root) {
  try {
    const out = execFileSync('gh', ['pr', 'list', '--state', 'all', '--limit', '400', '--json', 'number,state,headRefName,headRefOid'], {
      cwd: root,
      encoding: 'utf8',
      timeout: 20_000,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    const prs = JSON.parse(out)
    return Array.isArray(prs) ? prs : null
  } catch {
    return null
  }
}

/** `TALARIA_WORKTREE=<name>` in a checkout's ui/.env: the marker `talaria
 *  worktree` writes, which survives the branch rename an agent makes before
 *  it pushes (wt/<name> → feat/…). */
export function worktreeMarker(uiEnv) {
  const m = /^TALARIA_WORKTREE=([a-z0-9][a-z0-9-]*)\s*$/m.exec(String(uiEnv ?? ''))
  return m ? m[1] : null
}

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
      const why =
        a.kind === 'target'
          ? a.busy === null && !a.primary && !a.foreign && !a.kept && !a.current
            ? 'the cargo lock check cannot be trusted here (btrfs, or no /proc/locks) — never deleted'
            : a.dropping
              ? 'half removed, and a cargo holds it'
              : a.pr === 'merged' || a.pr === 'closed'
                ? `its PR is ${a.pr}, but the tree is ${a.dirty ? 'dirty' : 'unpushed'}`
                : 'oversized cache'
          : a.dirty
            ? 'dirty'
            : a.unpushed
              ? 'unpushed'
              : 'kept back'
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
      const note =
        a.action === 'remove'
          ? a.pr === 'merged' || a.pr === 'closed'
            ? `PR ${a.pr} — removable`
            : 'stale — removable'
          : a.primary
            ? 'warm cache'
            : a.foreign
              ? 'not a talaria worktree — reported only'
              : a.busy === true
                ? 'building now'
                : 'in use'
      const seeded = a.seeded ? ' (seeded by reflink: shares extents, so du says more than removing it frees)' : ''
      lines.push(`  ${a.id}  ${formatBytes(a.bytes)}  ${ageDays(a.ageMs)}  ${note}${seeded}`)
    }
  }
  const drops = pressureDrops(remaining, pressure, policy)
  if (drops.length) {
    lines.push(
      `over the line, \`bun talaria cleanup --apply\` also drops these idle build caches, unseeded and least recently used first, ` +
        `until the disk is under ${policy.pressurePct}% or a drop frees under ${formatBytes(policy.pressureMinFreedBytes)} ` +
        `(the checkouts stay; \`bun talaria seed-target\` refills one):`,
    )
    for (const a of drops.slice(0, policy.pressureDropMax)) lines.push(`  ${a.id}  ${formatBytes(a.bytes)}  last built ${ageDays(a.ageMs)} ago  ${a.pr ? `PR ${a.pr}` : 'no PR'}${a.seeded ? '  seeded' : ''}`)
    if (drops.length > policy.pressureDropMax) lines.push(`  (at most ${policy.pressureDropMax} per run; ${drops.length - policy.pressureDropMax} more after that)`)
  }
  const held = pressureKept(remaining, pressure, policy)
  if (held.length) {
    lines.push('flagged, not dropped: idle build caches a `.talaria-keep` holds (a human decides):')
    for (const a of held) lines.push(`  ${a.id}  ${formatBytes(a.bytes)}  last built ${ageDays(a.ageMs)} ago`)
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
      cur = { path: line.slice('worktree '.length), branch: '', head: '' }
    } else if (cur && line.startsWith('HEAD ')) {
      cur.head = line.slice('HEAD '.length).trim()
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
 * A local prefilter keeps the network call rare: a branch still ahead of
 * every base it could have merged into is obviously unfinished, so it never
 * reaches `gh`. The bases are `rc` and `testing` (a feature merges into
 * either; mergedPrefilter): a worktree merged into `testing` is 0 ahead of
 * origin/testing and still ahead of origin/rc until the next promotion, and
 * reading only rc kept it forever. A base whose ref does not resolve here —
 * no `testing` on this clone — is no answer and is ignored. Everything fails
 * OPEN — no `gh`, no auth, no network, no PR all read as "not merged", so a
 * tree is never reclaimed on a guess.
 *
 * The PR is the one whose head is THIS checkout's HEAD (mergedAtHead), not
 * any PR of the branch's name. `talaria worktree r6-ui` cut fresh from rc
 * reuses `wt/r6-ui`, and so does every recut. The fresh tree has 0 commits
 * ahead of rc, and `gh pr view wt/r6-ui` answered MERGED (#629, merged a week
 * earlier), so a stack minutes old was read as finished and torn down.
 */
function isMerged(dir, branch) {
  const ahead = (base) => {
    const n = tryGit(['rev-list', '--count', `${base}..HEAD`], dir)
    return n === null ? null : Number(n)
  }
  if (!mergedPrefilter(ahead('origin/rc'), ahead('origin/testing'))) return false
  const head = tryGit(['rev-parse', 'HEAD'], dir)
  if (!head) return false
  try {
    const out = execFileSync(
      'gh',
      ['pr', 'list', '--head', branch, '--state', 'all', '--limit', '100', '--json', 'number,state,headRefOid'],
      { cwd: dir, encoding: 'utf8', timeout: 5_000, stdio: ['ignore', 'pipe', 'pipe'] },
    )
    return mergedAtHead(JSON.parse(out), head)
  } catch {
    return false
  }
}

/** May this branch have merged — is it 0 commits ahead of a base? `aheadRc`
 *  and `aheadTesting` are `git rev-list --count origin/<base>..HEAD`, or null
 *  when that ref does not resolve (an unknown base is ignored, not a 0). Only
 *  a "maybe" reaches `gh`: the PR still decides (mergedAtHead). */
export function mergedPrefilter(aheadRc, aheadTesting) {
  return aheadRc === 0 || aheadTesting === 0
}

/** Did a pull request whose head is `head` merge? `prs` is `gh pr list
 *  --head <branch> --state all --json number,state,headRefOid`: every PR the
 *  branch name ever had. Only a MERGED one at this exact commit counts. An
 *  older PR of the same name is another piece of work: its head is not a
 *  commit a recut branch sits on. */
export function mergedAtHead(prs, head) {
  if (!head || !Array.isArray(prs)) return false
  return prs.some((p) => p?.state === 'MERGED' && p.headRefOid === head)
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
    owner: owner ?? '',
    busy: targetBusy(path),
    seeded: existsSync(join(path, '.talaria-seed.json')),
    foreign: Boolean(flags.foreign),
    pr: flags.pr ?? null,
    stackRunning: Boolean(flags.stackRunning),
    bytes: 0,
    // A build writing as the scan runs can stamp a time after `now`.
    ageMs: Math.max(0, now - targetLastUsedMs(path)),
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
 *  RAM and CPU; the checkout is a separate row and a separate decision.
 *  `checkout` is never acted on (a stack has no path): the removal reads it
 *  for a `.talaria-keep` touched since the scan. */
function stackRow(name, ageMs, merged, current, checkout) {
  return {
    kind: 'stack',
    id: name,
    checkout,
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
export function inventory(root, now = Date.now(), { scanNetworks = true, lifecycle = scanNetworks, pressureHigh = false } = {}) {
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
  // Build-cache lifecycle (a finished PR's cache goes first; over the line,
  // idle caches go least recently used first) needs every PR's state: one
  // `gh` call, made by `talaria cleanup` and never by the stop gate, which
  // removes no cache and must stay quick.
  const prs = lifecycle ? recentPrs(root) : null
  if (lifecycle && prs === null) notes.push('gh did not answer — build caches of finished pull requests were not looked up')
  const treeState = new Map()
  const inspectTree = (path) => {
    if (!treeState.has(path)) treeState.set(path, { dirty: isDirty(path), unpushed: isUnpushed(path) })
    return treeState.get(path)
  }
  /** A side checkout's build dirs, with what the lifecycle rules read. A
   *  checkout is `talaria worktree`'s (`ours`) by its wt/<name> pair or by
   *  the marker in its ui/.env; any other is reported and left alone. */
  const pushTargets = (path, { name, ours, branch, head, isCurrent, kept, running }) => {
    const branchName = (branch ?? '').replace(/^refs\/heads\//, '')
    const pr = ours && prs ? prStateAt(prs, branchName, head) : null
    for (const rel of TARGET_RELS) {
      const tp = join(path, rel)
      if (!existsSync(tp)) continue
      const row = targetRow(tp, rel, now, {
        current: isCurrent,
        primary: false,
        kept,
        foreign: !ours,
        pr,
        stackRunning: name ? (running ?? projectRunning(projects, `talaria-wt-${name}`)) : false,
      })
      if (lifecycle && ours && !isCurrent && (pr === 'merged' || pr === 'closed' || pressureHigh)) {
        Object.assign(row, inspectTree(path), { inspected: true })
      }
      artifacts.push(row)
    }
    // A build dir an interrupted removal left renamed (dropTarget): spent by
    // definition, so removable as soon as no cargo holds it.
    if (!ours) return
    for (const rel of TARGET_RELS) {
      const parent = dirname(join(path, rel))
      let names = []
      try {
        names = readdirSync(parent).filter((n) => DROPPING_RE.test(n))
      } catch {
        continue
      }
      for (const n of names) {
        const row = targetRow(join(parent, n), rel, now, { current: isCurrent, primary: false, kept: false, foreign: false, pr: null, stackRunning: false })
        row.id = `${basename(path)}/${dirname(rel)}/${n}`
        row.dropping = true
        artifacts.push(row)
      }
    }
  }

  for (const wt of listed) {
    const path = real(wt.path)
    const name = worktreeName(wt.branch, basename(path))
    if (!name) {
      if (path === primary) continue // its build dirs are counted below
      // Not from the stop gate: sizing these is a `du` of the largest trees
      // on the disk, and the gate removes no build dir of this kind anyway.
      if (!lifecycle) continue
      // A `talaria worktree` checkout whose branch was renamed before its
      // push (most agent worktrees), or someone's own checkout. Its build
      // dirs were invisible to the sweep either way, and they were the
      // largest ones on the disk (91 and 107 GiB on 2026-11-03).
      let marker = null
      try {
        marker = worktreeMarker(readFileSync(join(path, 'ui/.env'), 'utf8'))
      } catch {
        marker = null
      }
      const ours = marker !== null && basename(path) === `talaria-${marker}` && dirname(path) === dirname(primary)
      pushTargets(path, {
        name: ours ? marker : null,
        ours,
        branch: wt.branch,
        head: wt.head,
        isCurrent: path === current,
        kept: existsSync(join(path, '.talaria-keep')),
      })
      continue
    }
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
      artifacts.push(stackRow(name, ageMs, merged, isCurrent, path))
    }
    if (isPrimary) continue
    pushTargets(path, { name, ours: true, branch: wt.branch, head: wt.head, isCurrent, kept, running })
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

  // A worktree is live while git lists it or its sibling directory exists. A
  // directory holding only a `.talaria-keep` counts: it is how a human holds
  // a stack whose checkout is gone. A box is live while its box.env or a
  // `.talaria-keep` in its directory exists.
  const sibling = dirname(primary)
  const worktreeStands = (n) => seenWorktree.has(n) || existsSync(join(sibling, `talaria-${n}`))
  const boxLive = (n) => boxStands(home, n)

  if (projects) {
    artifacts.push(...orphanProjects({ projects, liveWorktree: worktreeStands, liveBox: boxLive }))
  }

  // Not from the stop gate: orphan networks are --apply's alone and weigh
  // nothing on disk, so the gate would only pay three more docker reads —
  // and a daemon busy with other stacks can hold a network read for a minute.
  if (projects && scanNetworks) {
    const scan = networkFacts()
    if (scan) {
      artifacts.push(
        ...orphanNetworks({
          ...scan,
          liveWorktree: worktreeStands,
          liveBox: boxLive,
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
      artifacts.push(
        ...orphanAppDbs({
          dbs,
          roots: { siblingParent: physical(dirname(primary)), devboxHome: physical(home) },
          liveCheckout: checkoutStands,
          liveBox: boxLive,
          now,
        }),
      )
    } else {
      notes.push('docker app database scan failed — orphan app databases not scanned')
    }
  }

  // MCP packages whose worktree or box is gone (UB3): --apply's alone too.
  if (projects && scanNetworks) {
    const pkgs = mcpPkgFacts()
    if (pkgs) {
      const { rows, unmarked } = orphanMcpPackages({
        ...pkgs,
        roots: { siblingParent: physical(dirname(primary)) },
        liveCheckout: checkoutStands,
        liveBox: boxLive,
        now,
      })
      artifacts.push(...rows)
      if (unmarked.length) {
        notes.push(
          `MCP package objects with no owner labels (made before the api labelled them), left alone: ${unmarked.join(', ')}. ` +
            '`docker inspect` says whose; removing one is a human\'s call.',
        )
      }
    } else {
      notes.push('docker MCP package scan failed — orphan MCP packages not scanned')
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

/** Every `talaria-mcp-*` container and network with what orphanMcpPackages
 *  reads, or null when docker did not answer (fail closed: no rows). Labels
 *  come from `inspect`, not `ls`, whose comma-joined Labels column would
 *  split a checkout path that holds a comma. `attached` is every container,
 *  running or stopped, that names the network. */
function mcpPkgFacts() {
  const ps = dockerOut(['ps', '-a', '--format', '{{.ID}}\t{{.Names}}\t{{.Networks}}'])
  const nets = dockerOut(['network', 'ls', '--filter', 'name=talaria-mcp-', '--format', '{{.ID}}\t{{.Name}}'])
  if (ps === null || nets === null) return null
  const all = ps
    .split('\n')
    .filter((l) => l.trim())
    .map((l) => {
      const [id, name, networks] = l.split('\t')
      return { id: (id ?? '').trim(), name: (name ?? '').trim(), networks: (networks ?? '').split(',').map((n) => n.trim()).filter(Boolean) }
    })
  const pkgIds = all.filter((c) => MCP_PKG_NAME_RE.test(c.name)).map((c) => c.id)
  const netRows = nets
    .split('\n')
    .map((l) => l.split('\t').map((x) => (x ?? '').trim()))
    .filter(([id, name]) => id && MCP_PKG_NAME_RE.test(name ?? ''))
  let containers = []
  let networks = []
  try {
    if (pkgIds.length) {
      const out = dockerOut(['inspect', ...pkgIds])
      if (out === null) return null
      containers = JSON.parse(out).map((c) => ({
        id: String(c.Id ?? ''),
        name: String(c.Name ?? '').replace(/^\//, ''),
        labels: c.Config?.Labels ?? {},
        running: c.State?.Running === true,
        createdMs: Date.parse(c.Created ?? '') || 0,
      }))
    }
    if (netRows.length) {
      const out = dockerOut(['network', 'inspect', ...netRows.map(([id]) => id)])
      if (out === null) return null
      networks = JSON.parse(out).map((n) => {
        const name = String(n.Name ?? '')
        return {
          id: String(n.Id ?? ''),
          name,
          labels: n.Labels ?? {},
          createdMs: Date.parse(n.Created ?? '') || 0,
          // By full id: inspect answers full ids, ps answers short ones.
          attached: all.filter((c) => c.networks.includes(name)).map((c) => containers.find((p) => p.id.startsWith(c.id))?.id ?? c.id),
        }
      })
    }
  } catch {
    return null
  }
  return { containers, networks }
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

/**
 * The offload refs `bun run gate:remote` left on origin for a branch: of the
 * `git ls-remote origin 'refs/heads/ci/*<branch>'` lines, exactly
 * `refs/heads/ci/<branch>` and `refs/heads/ci/full/<branch>`. The pattern is a
 * glob matched against the tail of a ref, `*` crossing `/`, so it also lists
 * `ci/xwt/<name>` and `ci/full/a/wt/<name>`: other branches' refs, never this
 * one's. Pure.
 * @param {string} branch  `wt/<name>`
 * @param {string[]|string} lsRemoteLines
 */
export function offloadRefsFor(branch, lsRemoteLines) {
  const lines = Array.isArray(lsRemoteLines) ? lsRemoteLines : String(lsRemoteLines ?? '').split('\n')
  const listed = new Set(lines.map((line) => String(line).trim().split(/\s+/)[1]).filter(Boolean))
  return [`refs/heads/ci/${branch}`, `refs/heads/ci/full/${branch}`].filter((ref) => listed.has(ref))
}

function firstLineOf(e) {
  const text = String(e?.stderr || e?.message || e || '')
  return text.split('\n').find((l) => l.trim())?.trim() || 'no output'
}

/**
 * Delete a merged worktree's offload refs from origin — the `ci/wt/<name>`
 * and `ci/full/wt/<name>` that `bun run gate:remote` pushes and keeps after a
 * red. Returns notes for the report. Never fatal: the worktree's own teardown
 * already happened, and ci-offload.yml's daily prune deletes any `ci/**` ref
 * whose tip is over a day old. The pre-push hook passes a `ci/` deletion.
 */
function deleteOffloadRefs(name, roots) {
  const branch = `wt/${name}`
  const opts = { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, GIT_TERMINAL_PROMPT: '0' } }
  let listing
  try {
    listing = execFileSync('git', ['-C', roots.primary, 'ls-remote', 'origin', `refs/heads/ci/*${branch}`], { ...opts, timeout: 20_000 })
  } catch (e) {
    return [`could not list the offload refs of ${branch} (the daily prune removes them): ${firstLineOf(e)}`]
  }
  const refs = offloadRefsFor(branch, listing)
  if (!refs.length) return []
  const names = refs.map((ref) => ref.slice('refs/heads/'.length)).join(', ')
  try {
    execFileSync('git', ['-C', roots.primary, 'push', '--quiet', 'origin', ...refs.map((ref) => `:${ref}`)], { ...opts, timeout: 60_000 })
  } catch (e) {
    return [`could not delete ${names} (the daily prune removes them): ${firstLineOf(e)}`]
  }
  logRemoval(`removed offload refs ${names}`)
  return [`deleted the offload refs of merged ${branch}: ${names}`]
}

/** A `.talaria-keep` in the checkout a stack, worktree or box row names. */
function keptNow(artifact) {
  const dir = artifact.checkout || artifact.path
  return Boolean(dir) && existsSync(join(dir, '.talaria-keep'))
}

/** The name a build dir is renamed to before it is deleted. */
export const DROPPING_RE = /^\.target-dropping-\d+$/

/**
 * Remove a build dir without racing a cargo. Checked, RENAMED to a sibling
 * `.target-dropping-<pid>` (atomic on one filesystem), checked again, then
 * deleted. A recursive delete of a large target takes minutes on a slow
 * disk; a cargo that starts in that window now finds no target at all and
 * builds a fresh one, rather than one being emptied under it. A cargo that
 * took the lock between the first check and the rename still holds the
 * moved lock file's inode, so the second check sees it: the dir is renamed
 * back when nothing has taken its place, and kept as it is when something
 * has. A leftover `.target-dropping-*` (an interrupted run) is swept as a
 * target of its own next time, under the same check.
 */
export function dropTarget(path) {
  if (targetBusy(path) !== false) throw new Error(`a cargo is building in ${path} (or its locks cannot be read) — left in place`)
  const dropping = DROPPING_RE.test(basename(path)) ? path : join(dirname(path), `.target-dropping-${process.pid}`)
  if (dropping !== path) {
    rmSync(dropping, { recursive: true, force: true }) // our own pid's leftover, if any
    renameSync(path, dropping)
  }
  if (targetBusy(dropping) !== false) {
    if (dropping !== path && !existsSync(path)) renameSync(dropping, path)
    throw new Error(`a cargo started building in ${path} as it was removed — left in place${existsSync(dropping) ? ` at ${dropping}` : ''}`)
  }
  rmSync(dropping, { recursive: true, force: true })
}

export function removeArtifact(artifact, roots) {
  if (!removalAllowed(artifact, roots)) {
    throw new Error(`refusing to remove ${artifact.kind} ${artifact.id}: path is outside the sweep`)
  }
  // Every row below that takes containers and volumes asks again, at the
  // moment of removal, whether a checkout or a `.talaria-keep` vouches for
  // it. The scan's answer is seconds or minutes old by now.
  if (artifact.kind === 'orphan-compose') {
    const vouched = projectVouchedBy(artifact.id, roots)
    if (vouched) throw new Error(`${vouched} vouches for ${artifact.id} — left in place`)
    dockerByLabel(artifact.id)
    return
  }
  if ((artifact.kind === 'stack' || artifact.kind === 'worktree' || artifact.kind === 'devbox') && keptNow(artifact)) {
    throw new Error(`${join(artifact.checkout || artifact.path, '.talaria-keep')} holds it — left in place`)
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
  if (artifact.kind === 'orphan-mcp-container') {
    // Asked again at the moment of removal, by id: still that package, still
    // labelled for the owner the scan found gone. Anything else and it stays.
    const out = dockerOut(['inspect', artifact.containerId])
    let now
    try {
      now = out === null ? null : JSON.parse(out)[0]
    } catch {
      now = null
    }
    if (!now || String(now.Name ?? '').replace(/^\//, '') !== artifact.id || ownerKey(now.Config?.Labels) !== artifact.owner) {
      throw new Error(`MCP package ${artifact.id} changed since the scan (or docker did not answer) — left in place`)
    }
    // The app database's retry, for the same busy daemon.
    let why = ''
    for (let attempt = 0; attempt < 2; attempt++) {
      why = dockerErr(['rm', '-f', artifact.containerId], 120_000)
      if (why === null || containerGone(artifact.containerId)) break
    }
    if (why !== null && !containerGone(artifact.containerId)) throw new Error(`docker rm ${artifact.id} failed: ${why}`)
    return
  }
  if (artifact.kind === 'orphan-mcp-network') {
    const out = dockerOut(['network', 'inspect', artifact.networkId])
    let now
    try {
      now = out === null ? null : JSON.parse(out)[0]
    } catch {
      now = null
    }
    if (!now || String(now.Name ?? '') !== artifact.id || ownerKey(now.Labels) !== artifact.owner) {
      throw new Error(`network ${artifact.id} changed since the scan (or docker did not answer) — left in place`)
    }
    // Its own containers went first in this pass; anything attached now,
    // running or stopped, is not, and keeps it.
    const attached = dockerOut(['ps', '-aq', '--filter', `network=${artifact.networkId}`])
    if (attached === null || attached.trim() !== '') {
      throw new Error(`network ${artifact.id} is in use (or docker did not answer) — left in place`)
    }
    if (dockerOut(['network', 'rm', artifact.networkId], 120_000) === null && !networkGone(artifact.networkId)) {
      throw new Error(`docker network rm ${artifact.id} failed`)
    }
    return
  }
  if (artifact.kind === 'tmp') {
    rmSync(artifact.path, { recursive: true, force: true })
    return
  }
  if (artifact.kind === 'stack') {
    downWorktreeStack(artifact.id, roots)
    // A merged worktree's stack is the first thing the gate takes; its
    // offload refs go with it. A stale one's are the daily prune's.
    return artifact.merged ? deleteOffloadRefs(artifact.id, roots) : []
  }
  if (artifact.kind === 'target') {
    dropTarget(artifact.path)
    return
  }
  if (artifact.kind === 'worktree') {
    // `git worktree remove --force` takes the build dirs with it, so they get
    // the build dirs' own check first: a cargo building there, or locks that
    // cannot be read, and the checkout stays.
    for (const rel of TARGET_RELS) {
      const tp = join(artifact.path, rel)
      if (existsSync(tp) && targetBusy(tp) !== false) {
        throw new Error(`${tp} is in use by a cargo (or its locks cannot be read) — checkout left in place`)
      }
    }
    downWorktreeStack(artifact.id, roots)
    tryExec('git', ['-C', roots.primary, 'worktree', 'remove', '--force', artifact.path])
    if (existsSync(artifact.path)) rmSync(artifact.path, { recursive: true, force: true })
    tryExec('git', ['-C', roots.primary, 'worktree', 'prune'])
    tryExec('git', ['-C', roots.primary, 'branch', '-D', `wt/${artifact.id}`])
    return artifact.merged ? deleteOffloadRefs(artifact.id, roots) : []
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

/**
 * Over the line: drop idle caches (pressureDrops' order), at most
 * POLICY.pressureDropMax, re-reading the disk after each, and stopping as
 * soon as it is under the line or a drop freed less than
 * POLICY.pressureMinFreedBytes, because then build caches are not what fills
 * the disk. `remove` and `read` are injected so the bounds can be tested
 * without a disk. Returns what went, what failed, the last reading, and
 * notes that say why it stopped and what is left.
 */
export function runPressureDrops(classified, pressure, { remove, read }, policy = POLICY) {
  const candidates = pressureDrops(classified, pressure, policy)
  const dropped = []
  const failed = []
  const notes = []
  let reading = pressure
  let why = ''
  for (const a of candidates) {
    if (!(reading.usedPct >= policy.pressurePct)) {
      why = `the disk is back under ${policy.pressurePct}%`
      break
    }
    if (dropped.length >= policy.pressureDropMax) {
      why = `${policy.pressureDropMax} caches is this run's limit`
      break
    }
    const before = reading.avail
    try {
      remove(a)
      dropped.push(a)
    } catch (e) {
      failed.push(`${a.kind} ${a.id}: ${e instanceof Error ? e.message : e}`)
      continue
    }
    try {
      reading = read()
    } catch {
      why = 'the disk could not be read again'
      break
    }
    const freed = reading.avail - before
    if (freed < policy.pressureMinFreedBytes) {
      why = `dropping ${a.id} freed only ${formatBytes(Math.max(0, freed))}, so build caches are not what fills the disk`
      break
    }
  }
  if (candidates.length) {
    const left = candidates.filter((a) => !dropped.includes(a) && !failed.some((f) => f.startsWith(`${a.kind} ${a.id}:`)))
    notes.push(
      `over the line: dropped ${dropped.length} idle build cache(s)${dropped.length ? ` (${dropped.map((a) => a.id).join(', ')})` : ''}` +
        (why ? `; stopped: ${why}` : '') +
        (left.length ? `; left: ${left.map((a) => `${a.id} ${formatBytes(a.bytes)}`).join(', ')}` : '; none left to drop'),
    )
  }
  return { dropped, failed, notes, pressure: reading }
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
    inventoried = inventory(root, Date.now(), {
      scanNetworks: mode !== 'gate',
      lifecycle: mode !== 'gate' && mode !== 'pressure',
      pressureHigh: pressure.usedPct >= POLICY.pressurePct,
    })
  } catch (e) {
    console.error(`cleanup sweep could not scan — not a pass: ${e instanceof Error ? e.message : e}`)
    process.exit(1)
  }

  let classified = measure(inventoried.artifacts)
  const removing = selectRemoval(classified, mode)
  const removed = []
  const failed = []
  // What a removal did beyond the row itself (a merged worktree's offload
  // refs), for the report.
  const sideNotes = []
  for (const a of removing) {
    try {
      const extra = removeArtifact(a, inventoried.roots)
      removed.push(a)
      logRemoval(`removed ${a.kind} ${a.id} ${a.path || ''}`.trim())
      if (Array.isArray(extra)) sideNotes.push(...extra)
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
  // Still over the line after the safe set: idle build caches, least
  // recently used first, one at a time, until the disk is back under it.
  // statfs after each one, not the summed `du`: a seeded target shares
  // extents with its source, so removing it frees less than it measures.
  const pressureNotes = []
  if (mode === 'apply') {
    const r = runPressureDrops(classified, pressure, {
      remove: (a) => removeArtifact(a, inventoried.roots),
      read: () => readPressure(root),
    })
    for (const a of r.dropped) {
      removed.push(a)
      logRemoval(`removed target ${a.id} ${a.path} (disk over the line; idle cache, least recently used first)`)
    }
    failed.push(...r.failed)
    classified = classified.filter((x) => !r.dropped.includes(x))
    pressure = r.pressure
    pressureNotes.push(...r.notes)
  }
  const v = verdict(classified, pressure)
  const notes = [...inventoried.notes, ...pressureNotes, ...sideNotes, ...failed.map((f) => `could not remove ${f}`)]
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
