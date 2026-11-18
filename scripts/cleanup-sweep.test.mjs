// The sweep's dangerous edges: what it will delete, what it must not, and
// when the stop gate has to say no. A wrong answer here fills the disk or
// removes someone else's checkout. Run by `bun run check`.
import { execFileSync, spawn, spawnSync } from 'node:child_process'
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { delimiter, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  POLICY,
  ageDays,
  appDbOwner,
  boxProjectOwners,
  boxStands,
  checkoutStands,
  classify,
  formatReport,
  mcpPkgOwner,
  mergedAtHead,
  mergedPrefilter,
  offloadRefsFor,
  orphanAppDbs,
  orphanMcpPackages,
  ownerKey,
  orphanNetworks,
  orphanProjects,
  parseDockerTime,
  parseLabels,
  prStateAt,
  pressureDrops,
  pressureKept,
  runPressureDrops,
  dropTarget,
  targetBusy,
  BTRFS_SUPER_MAGIC,
  procLocksDev,
  heldFlocks,
  projectVouchedBy,
  removalAllowed,
  removeArtifact,
  selectRemoval,
  verdict,
  worktreeMarker,
  worktreeName,
  worktreeProjectOwners,
} from './cleanup-sweep.mjs'

const DAY = 24 * 60 * 60 * 1000
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
  if (actual !== expected) throw new Error(`expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`)
}

const stale = (over = {}) => ({
  kind: 'worktree',
  id: 'old',
  path: '/work/talaria-old',
  bytes: 0,
  ageMs: POLICY.staleMs + DAY,
  running: false,
  dirty: false,
  unpushed: false,
  kept: false,
  current: false,
  primary: false,
  ...over,
})

check('wt/<name> at talaria-<name> is ours', () => {
  eq(worktreeName('refs/heads/wt/board-ui', 'talaria-board-ui'), 'board-ui')
})

check('a human checkout is not a sweepable worktree', () => {
  eq(worktreeName('refs/heads/desktop/flatpak', 'talaria-desktop-flatpak'), null)
  eq(worktreeName('refs/heads/rc', 'talaria-repo-workflow'), null)
  eq(worktreeName('refs/heads/wt/board-ui', 'talaria-other'), null)
})

check('a fresh or running or kept or current worktree stays', () => {
  eq(classify(stale({ ageMs: POLICY.staleMs - 1 })), 'keep')
  eq(classify(stale({ running: true })), 'keep')
  eq(classify(stale({ kept: true })), 'keep')
  eq(classify(stale({ current: true })), 'keep')
  eq(classify(stale({ primary: true })), 'keep')
})

check('a stale clean worktree is removable; dirty or unpushed is only flagged', () => {
  eq(classify(stale()), 'remove')
  eq(classify(stale({ dirty: true })), 'flag')
  eq(classify(stale({ unpushed: true })), 'flag')
})

check('old scratch is removable; yesterday is not', () => {
  eq(classify(stale({ kind: 'tmp', ageMs: POLICY.tmpStaleMs + 1 })), 'remove')
  eq(classify(stale({ kind: 'tmp', ageMs: POLICY.tmpStaleMs - 1 })), 'keep')
})

check("a -fleet project has two readings: a worktree's agents, or the stack of a worktree named -fleet", () => {
  eq(JSON.stringify(worktreeProjectOwners('talaria-wt-agent-todo')), '["agent-todo"]')
  eq(JSON.stringify(worktreeProjectOwners('talaria-wt-agent-todo-fleet')), '["agent-todo-fleet","agent-todo"]')
  eq(JSON.stringify(worktreeProjectOwners('talaria-wt-rel-fleet')), '["rel-fleet","rel"]')
  eq(JSON.stringify(worktreeProjectOwners('talaria-wt-fleet')), '["fleet"]')
  eq(worktreeProjectOwners('talaria-dogfood-fleet'), null)
  eq(worktreeProjectOwners('devbox-x'), null)
  eq(worktreeProjectOwners('talaria'), null)
})

check('an orphan compose project is removable even if it looks live', () => {
  eq(classify(stale({ kind: 'orphan-compose', running: true, path: '' })), 'remove')
})

check('an oversized target is flagged, never removed', () => {
  eq(classify(stale({ kind: 'target', bytes: POLICY.oversizedTargetBytes })), 'flag')
  eq(classify(stale({ kind: 'target', bytes: POLICY.oversizedTargetBytes - 1 })), 'keep')
})

check('a merged worktree is finished work, however fresh it looks', () => {
  const fresh = { ageMs: DAY, merged: true }
  eq(classify(stale(fresh)), 'remove')
  // A running stack no longer buys immortality once the work has landed.
  eq(classify(stale({ ...fresh, running: true })), 'remove')
  // But the usual protections still win over "merged".
  eq(classify(stale({ ...fresh, dirty: true })), 'flag')
  eq(classify(stale({ ...fresh, unpushed: true })), 'flag')
  eq(classify(stale({ ...fresh, kept: true })), 'keep')
  eq(classify(stale({ ...fresh, current: true })), 'keep')
  eq(classify(stale({ ...fresh, primary: true })), 'keep')
})

check('an unmerged worktree is still governed by the idle clock', () => {
  eq(classify(stale({ ageMs: DAY })), 'keep')
  eq(classify(stale({ ageMs: DAY, running: true })), 'keep')
})

check('a stack is removable unless it is the one we are standing in', () => {
  const s = { kind: 'stack', path: '', ageMs: DAY, merged: true }
  eq(classify(stale(s)), 'remove')
  eq(classify(stale({ ...s, current: true })), 'keep')
  // Bytes are irrelevant: stopping containers frees no disk, only RAM.
  eq(classify(stale({ ...s, dirty: true, unpushed: true })), 'remove')
})

check("a side worktree's build dir goes stale; the primary's never does", () => {
  const t = { kind: 'target', bytes: 1024 }
  eq(classify(stale({ ...t, ageMs: POLICY.staleTargetMs + DAY })), 'remove')
  eq(classify(stale({ ...t, ageMs: POLICY.staleTargetMs - DAY })), 'keep')
  eq(classify(stale({ ...t, ageMs: POLICY.staleTargetMs + DAY, primary: true })), 'keep')
  eq(classify(stale({ ...t, ageMs: POLICY.staleTargetMs + DAY, current: true })), 'keep')
  eq(classify(stale({ ...t, ageMs: POLICY.staleTargetMs + DAY, kept: true })), 'keep')
  // Oversized and primary is still only ever a flag, at any age.
  const big = { ...t, bytes: POLICY.oversizedTargetBytes, primary: true }
  eq(classify(stale({ ...big, ageMs: POLICY.staleTargetMs * 10 })), 'flag')
})

check('--gate does not delete a worktree; --apply does', () => {
  const rows = [
    { ...stale(), action: 'remove' },
    { ...stale({ kind: 'tmp', id: 'talaria-scratch' }), action: 'remove' },
    { ...stale({ kind: 'orphan-compose', id: 'talaria-wt-gone', path: '' }), action: 'remove' },
  ]
  const gate = selectRemoval(rows, 'gate').map((a) => a.kind)
  eq(gate.includes('worktree'), false)
  eq(gate.includes('tmp'), true)
  eq(gate.includes('orphan-compose'), true)
  eq(selectRemoval(rows, 'apply').length, 3)
  eq(selectRemoval(rows, 'report').length, 0)
})

check('--gate stops a finished stack but never deletes a build dir', () => {
  const rows = [
    { ...stale({ kind: 'stack', id: 'shipped', path: '' }), action: 'remove' },
    { ...stale({ kind: 'target', id: 'talaria-old/api/target', path: '/work/talaria-old/api/target' }), action: 'remove' },
  ]
  const gate = selectRemoval(rows, 'gate').map((a) => a.kind)
  eq(gate.includes('stack'), true)
  eq(gate.includes('target'), false)
  eq(selectRemoval(rows, 'apply').length, 2)
})

check('dirty bytes do not block; removable bytes and disk pressure do', () => {
  const pressure = { usedPct: 10, avail: 80 * 1024 ** 3 }
  const dirty = { ...stale({ dirty: true, bytes: 9 * 1024 ** 3 }), action: 'flag' }
  eq(verdict([dirty], pressure).exit, 0)
  const big = { ...stale({ bytes: POLICY.staleBudgetBytes }), action: 'remove' }
  eq(verdict([big], pressure).exit, 2)
  eq(verdict([], { usedPct: POLICY.pressurePct, avail: 1 }).exit, 2)
  eq(verdict([], { usedPct: POLICY.pressurePct - 0.1, avail: 1 }).exit, 0)
})

check('the block message names the command that clears it', () => {
  const row = { ...stale({ bytes: POLICY.staleBudgetBytes }), action: 'remove' }
  const text = formatReport({
    pressure: { usedPct: 90, avail: 1024 ** 3 },
    remaining: [row],
    removed: [],
    notes: [],
  })
  if (!text.includes('bun talaria cleanup --apply')) throw new Error(text)
  if (!text.includes('over the 85% line')) throw new Error(text)
})

const roots = {
  primary: '/work/talaria',
  current: '/work/talaria-agent-cleanup',
  siblingParent: '/work',
  devboxHome: '/work/devboxes',
  tmp: '/tmp',
}

check('the executor refuses a path outside the convention', () => {
  eq(removalAllowed({ kind: 'worktree', id: 'old', path: '/work/talaria-old' }, roots), true)
  eq(removalAllowed({ kind: 'worktree', id: 'old', path: '/work/talaria' }, roots), false)
  eq(removalAllowed({ kind: 'worktree', id: 'old', path: '/work/talaria-agent-cleanup' }, roots), false)
  eq(removalAllowed({ kind: 'worktree', id: 'old', path: '/elsewhere/talaria-old' }, roots), false)
  eq(removalAllowed({ kind: 'tmp', id: 'talaria-scratch', path: '/tmp/talaria-scratch' }, roots), true)
  eq(removalAllowed({ kind: 'tmp', id: 'notes', path: '/tmp/notes' }, roots), false)
  eq(removalAllowed({ kind: 'tmp', id: 'talaria-x', path: '/tmp/talaria-x/../../etc/passwd' }, roots), false)
  eq(removalAllowed({ kind: 'devbox', id: 'demo', path: '/work/devboxes/demo' }, roots), true)
  eq(removalAllowed({ kind: 'devbox', id: 'shared', path: '/work/devboxes/shared' }, roots), false)
  eq(removalAllowed({ kind: 'orphan-compose', id: 'talaria-wt-gone', path: '' }, roots), true)
  eq(removalAllowed({ kind: 'orphan-compose', id: 'talaria', path: '' }, roots), false)
  eq(removalAllowed({ kind: 'target', id: 'api/target', path: '/work/talaria/api/target' }, roots), false)
})

check('a build dir is removable only inside a side worktree', () => {
  const t = (path) => removalAllowed({ kind: 'target', id: 'x', path }, roots)
  eq(t('/work/talaria-old/api/target'), true)
  eq(t('/work/talaria-old/desktop/src-tauri/target'), true)
  // The primary's warm cache and the tree we are running in are off limits.
  eq(t('/work/talaria/api/target'), false)
  eq(t('/work/talaria-agent-cleanup/api/target'), false)
  // Only the two paths the repo builds into, only directly under the parent.
  eq(t('/work/talaria-old/src'), false)
  eq(t('/work/talaria-old/api/target/../../.ssh'), false)
  eq(t('/elsewhere/talaria-old/api/target'), false)
  eq(t('/work/notalaria-old/api/target'), false)
})

check('a stack acts on a compose project, never on a path', () => {
  eq(removalAllowed({ kind: 'stack', id: 'shipped', path: '' }, roots), true)
  eq(removalAllowed({ kind: 'stack', id: 'shipped', path: '/work/talaria-shipped' }, roots), false)
  eq(removalAllowed({ kind: 'stack', id: '../evil', path: '' }, roots), false)
})

// ── Orphan networks (follow-up A8) ──────────────────────────────────────────
// Every compose project network holds a subnet from the daemon's ~30 address
// pools; interrupted and half-torn-down stacks left theirs behind until
// `compose up` failed with "all predefined address pools have been fully
// subnetted". These pin what may be removed, and the much longer list of what
// may not.

const NOW = Date.parse('2026-10-27T20:00:00Z')
const old = NOW - POLICY.orphanNetworkMinAgeMs - 1
const net = (name, project, over = {}) => ({
  id: 'a1b2c3d4e5f6',
  name,
  labels: project === null ? {} : { 'com.docker.compose.project': project, 'com.docker.compose.network': 'default' },
  createdMs: old,
  ...over,
})
const scanNets = (networks, containers = [], live = [], boxes) =>
  orphanNetworks({
    networks,
    containers,
    liveWorktree: (o) => live.includes(o),
    ...(boxes ? { liveBox: (n) => boxes.includes(n) } : {}),
    now: NOW,
  }).map((r) => r.id)

check('a talaria-* project network with nothing on it and nobody owning it is an orphan', () => {
  eq(scanNets([net('talaria-wt-gone_default', 'talaria-wt-gone')]).join(), 'talaria-wt-gone_default')
  eq(scanNets([net('talaria-appdb-x-probe_default', 'talaria-appdb-x-probe')]).join(), 'talaria-appdb-x-probe_default')
  const [row] = orphanNetworks({
    networks: [net('talaria-wt-gone_default', 'talaria-wt-gone')],
    containers: [],
    liveWorktree: () => false,
    now: NOW,
  })
  eq(row.kind, 'orphan-network')
  eq(row.networkId, 'a1b2c3d4e5f6')
  eq(classify(row), 'remove')
})

check('the shared talaria network is never an orphan, whatever its labels say', () => {
  eq(scanNets([net('talaria', 'talaria-dev'), net('talaria', null)]).length, 0)
})

check('a network outside a talaria-* or devbox-* project is not ours', () => {
  eq(scanNets([net('supabase_network_x', 'x'), net('mine', null), net('mine', 'my-project')], [], [], []).length, 0)
})

// ── devbox networks (follow-up R20) ─────────────────────────────────────────
// A box holds two networks — its project's `_default` and its private
// `devbox-<name>-fleet` — and a box whose containers were removed without
// `talaria box rm` (or an interrupted `box new`) left both holding subnets.

check('a box project network with nothing on it and no box checkout is an orphan — both of its networks', () => {
  const fleet = net('devbox-gone-fleet', 'devbox-gone', { labels: { 'com.docker.compose.project': 'devbox-gone', 'com.docker.compose.network': 'fleet' } })
  eq(scanNets([net('devbox-gone_default', 'devbox-gone'), fleet], [], [], []).join(), 'devbox-gone_default,devbox-gone-fleet')
  const [row] = orphanNetworks({ networks: [fleet], containers: [], liveWorktree: () => false, liveBox: () => false, now: NOW })
  eq(row.kind, 'orphan-network')
  eq(classify(row), 'remove')
})

check("a live box's networks stay — its own project, and the fleet project it renders", () => {
  eq(scanNets([net('devbox-mobile_default', 'devbox-mobile')], [], [], ['mobile']).length, 0)
  eq(scanNets([net('devbox-mobile-fleet_default', 'devbox-mobile-fleet')], [], [], ['mobile']).length, 0)
  // ...and a box that is itself named `<x>-fleet` keeps its project network.
  eq(scanNets([net('devbox-ops-fleet_default', 'devbox-ops-fleet')], [], [], ['ops-fleet']).length, 0)
})

check('a box network is kept when the caller cannot say whether its box lives', () => {
  eq(scanNets([net('devbox-gone_default', 'devbox-gone')]).length, 0)
})

check('a box network something names, or whose project has a container, stays', () => {
  const n = net('devbox-gone_default', 'devbox-gone')
  eq(scanNets([n], [{ project: 'elsewhere', networks: ['devbox-gone_default'] }], [], []).length, 0)
  eq(scanNets([n], [{ project: 'devbox-gone', networks: ['bridge'] }], [], []).length, 0)
})

check('a box network younger than the min age is mid-`up`', () => {
  eq(scanNets([net('devbox-new_default', 'devbox-new', { createdMs: NOW - 1000 })], [], [], []).length, 0)
})

check("a box project's network named outside devbox-* is not the sweep's; nor is the tools layer", () => {
  eq(scanNets([net('talaria-dev_default', 'devbox-gone'), net('custom', 'devbox-gone')], [], [], []).length, 0)
  eq(scanNets([net('devbox-shared_default', 'devbox-shared')], [], [], []).length, 0)
})

check('box projects parse to the boxes that could own them', () => {
  eq(JSON.stringify(boxProjectOwners('devbox-mobile')), '["mobile"]')
  eq(JSON.stringify(boxProjectOwners('devbox-mobile-fleet')), '["mobile-fleet","mobile"]')
  eq(JSON.stringify(boxProjectOwners('devbox-shared')), '[]')
  eq(boxProjectOwners('talaria-wt-x'), null)
  eq(boxProjectOwners('devbox-'), null)
})

check('a network a container names — even a stopped one — stays', () => {
  const n = net('talaria-wt-gone_default', 'talaria-wt-gone')
  eq(scanNets([n], [{ project: 'someone-else', networks: ['bridge', 'talaria-wt-gone_default'] }]).length, 0)
})

check('a network whose project still has any container stays (a stack, running or stopped)', () => {
  const n = net('talaria-dogfood_internal', 'talaria-dogfood')
  eq(scanNets([n], [{ project: 'talaria-dogfood', networks: ['talaria-dogfood'] }]).length, 0)
})

check("a live worktree's project network stays, its fleet's too", () => {
  eq(scanNets([net('talaria-wt-live_default', 'talaria-wt-live')], [], ['live']).length, 0)
  eq(scanNets([net('talaria-wt-live-fleet_default', 'talaria-wt-live-fleet')], [], ['live']).length, 0)
})

check('a network younger than the min age is mid-`up`, not an orphan; an unreadable age is young', () => {
  eq(scanNets([net('talaria-wt-new_default', 'talaria-wt-new', { createdMs: NOW - 1000 })]).length, 0)
  eq(scanNets([net('talaria-wt-odd_default', 'talaria-wt-odd', { createdMs: 0 })]).length, 0)
})

check('a pool probe the CLI could not remove is swept like an orphan', () => {
  const probe = net('talaria-pool-probe-4242', null, { labels: { 'talaria.pool-probe': '1' } })
  eq(scanNets([probe]).join(), 'talaria-pool-probe-4242')
  eq(scanNets([probe], [{ project: '', networks: ['talaria-pool-probe-4242'] }]).length, 0)
})

check('orphan networks go on --apply only, never from the stop gate', () => {
  const rows = [{ ...stale({ kind: 'orphan-network', id: 'talaria-wt-gone_default', path: '', networkId: 'a1b2c3d4e5f6' }), action: 'remove' }]
  eq(selectRemoval(rows, 'gate').length, 0)
  eq(selectRemoval(rows, 'apply').length, 1)
  eq(selectRemoval(rows, 'report').length, 0)
})

check('the executor removes a network only by a talaria- or devbox- name and a docker id', () => {
  const r = (over) => removalAllowed({ kind: 'orphan-network', id: 'talaria-wt-gone_default', networkId: 'a1b2c3d4e5f6', path: '', ...over }, roots)
  eq(r({}), true)
  eq(r({ id: 'talaria' }), false)
  eq(r({ id: 'devbox-x_default' }), true)
  eq(r({ id: 'devbox-x-fleet' }), true)
  eq(r({ id: 'devbox' }), false)
  eq(r({ id: 'devbox-' }), false)
  eq(r({ id: 'supabase_network_x' }), false)
  eq(r({ id: 'bridge' }), false)
  eq(r({ networkId: 'talaria; rm -rf /' }), false)
  eq(r({ networkId: undefined }), false)
  eq(r({ path: '/work/talaria-gone' }), false)
})

check("docker's network fields parse", () => {
  const l = parseLabels('com.docker.compose.network=default,com.docker.compose.project=talaria-wt-x,com.docker.compose.version=5.5.1')
  eq(l['com.docker.compose.project'], 'talaria-wt-x')
  eq(Object.keys(parseLabels('')).length, 0)
  eq(parseDockerTime('2026-10-08 20:59:58.97940353 -0600 MDT'), Date.parse('2026-10-09T02:59:58.979Z'))
  eq(parseDockerTime('2026-10-27 13:24:12 +0000 UTC'), Date.parse('2026-10-27T13:24:12Z'))
  eq(parseDockerTime('yesterday'), 0)
})

check('an age prints in minutes under an hour — an orphan network qualifies at ten', () => {
  const MIN = 60 * 1000
  eq(ageDays(13 * MIN), '13m')
  eq(ageDays(10 * 1000), '1m')
  eq(ageDays(90 * MIN), '2h')
  eq(ageDays(23 * 60 * MIN), '23h')
  eq(ageDays(3 * DAY), '3d')
  eq(ageDays(-1), '?')
  eq(ageDays(Number.NaN), '?')
})

// ── App databases a gone checkout or box left running (N9) ─────────────────
const dbRoots = { siblingParent: '/w', devboxHome: '/w/devboxes' }
const appdb = (over = {}) => ({
  id: 'a1b2c3d4e5f6a1b2c3d4e5f6',
  name: 'talaria-appdb-gone-notes',
  project: 'talaria-appdb-gone-notes',
  running: true,
  createdMs: NOW - 3 * DAY,
  source: '/w/talaria-gone/app-data/notes/pg',
  ...over,
})

check("an app database's owner is read from its cluster's mount: a sibling checkout, a box, or nobody", () => {
  const o = (src) => JSON.stringify(appDbOwner(src, dbRoots))
  eq(o('/w/talaria-gone/app-data/notes/pg'), JSON.stringify({ kind: 'checkout', dir: '/w/talaria-gone' }))
  eq(o('/w/devboxes/b1/state/app-data/notes/pg'), JSON.stringify({ kind: 'box', name: 'b1', dir: '/w/devboxes/b1' }))
  // The primary is `talaria`, not `talaria-<x>`: never placed, never swept.
  eq(o('/w/talaria/app-data/notes/pg'), 'null')
  eq(o('/w/devboxes/shared/x/pg'), 'null')
  eq(o('/w/devboxes/.hidden/pg'), 'null')
  eq(o('/w/other-repo/app-data/notes/pg'), 'null')
  eq(o('/var/lib/docker/volumes/x/_data'), 'null')
  eq(o('/data/app-data/notes/pg'), 'null')
  eq(o('/w/talaria-gone'), JSON.stringify({ kind: 'checkout', dir: '/w/talaria-gone' }))
  eq(o(''), 'null')
})

check("an app database whose checkout or box is gone is a row, running or stopped; a live owner's stays", () => {
  const rows = orphanAppDbs({
    dbs: [
      appdb(),
      appdb({ name: 'talaria-appdb-gonebox-notes', source: '/w/devboxes/gonebox/state/app-data/notes/pg', running: false }),
      appdb({ name: 'talaria-appdb-live-notes', source: '/w/talaria-live/app-data/notes/pg' }),
      appdb({ name: 'talaria-appdb-livebox-notes', source: '/w/devboxes/livebox/state/app-data/notes/pg' }),
      appdb({ name: 'talaria-appdb-talaria-notes', source: '/w/talaria/app-data/notes/pg' }),
      appdb({ name: 'talaria-appdb-image-notes', source: '/data/app-data/notes/pg' }),
      appdb({ name: 'not-an-appdb', source: '/w/talaria-gone/app-data/x/pg' }),
      appdb({ name: 'talaria-appdb-nomount-x', source: '' }),
    ],
    roots: dbRoots,
    liveCheckout: (dir) => dir === '/w/talaria-live',
    liveBox: (name) => name === 'livebox',
    now: NOW,
  })
  eq(rows.map((r) => r.id).join(','), 'talaria-appdb-gone-notes,talaria-appdb-gonebox-notes')
  eq(rows[0].kind, 'orphan-appdb')
  eq(rows[0].path, '')
  eq(rows[0].why, '/w/talaria-gone is gone; its data stays at /w/talaria-gone/app-data/notes/pg')
  eq(rows[1].why, 'box gonebox is gone; its data stays at /w/devboxes/gonebox/state/app-data/notes/pg')
  eq(rows[1].running, false)
  eq(ageDays(rows[0].ageMs), '3d')
})

check('a checkout directory still on disk keeps its app databases, listed by git or not, .git or not', () => {
  const parent = mkdtempSync(join(tmpdir(), 'talaria-sweep-test-'))
  try {
    // A sibling that still exists but is no git worktree: half removed, or
    // copied by hand. Its running database is not the sweep's.
    mkdirSync(join(parent, 'talaria-stray', 'app-data', 'notes', 'pg'), { recursive: true })
    eq(checkoutStands(join(parent, 'talaria-stray')), true)
    eq(checkoutStands(join(parent, 'talaria-gone')), false)
    const rows = orphanAppDbs({
      dbs: [
        appdb({ name: 'talaria-appdb-stray-notes', source: join(parent, 'talaria-stray', 'app-data', 'notes', 'pg') }),
        appdb({ name: 'talaria-appdb-gone-notes', source: join(parent, 'talaria-gone', 'app-data', 'notes', 'pg') }),
      ],
      roots: { siblingParent: parent, devboxHome: join(parent, 'devboxes') },
      liveCheckout: checkoutStands,
      now: NOW,
    })
    eq(rows.map((r) => r.id).join(','), 'talaria-appdb-gone-notes')
  } finally {
    rmSync(parent, { recursive: true, force: true })
  }
})

check('a caller that cannot say whether an owner lives keeps every app database', () => {
  eq(orphanAppDbs({ dbs: [appdb()], roots: dbRoots, now: NOW }).length, 0)
})

check('orphan app databases go on --apply only, never from the stop gate, and the report says why', () => {
  const [row] = orphanAppDbs({ dbs: [appdb()], roots: dbRoots, liveCheckout: () => false, now: NOW })
  eq(classify(row), 'remove')
  const rows = [{ ...row, action: 'remove' }]
  eq(selectRemoval(rows, 'gate').length, 0)
  eq(selectRemoval(rows, 'apply').length, 1)
  eq(selectRemoval(rows, 'report').length, 0)
  // Weighs nothing on disk: listed, never a reason for the gate to block.
  eq(verdict(rows, { usedPct: 10 }).exit, 0)
  const text = formatReport({ pressure: { usedPct: 10, avail: 1 }, remaining: rows, removed: [], notes: [] })
  if (!text.includes('orphan-appdb talaria-appdb-gone-notes') || !text.includes('/w/talaria-gone is gone')) throw new Error(text)
  if (!text.includes('bun talaria cleanup --apply')) throw new Error(text)
})

check('the executor removes an app database only by its name and a docker id, never a path', () => {
  const r = (over) => removalAllowed({ kind: 'orphan-appdb', id: 'talaria-appdb-gone-notes', containerId: 'a1b2c3d4e5f6', path: '', ...over }, roots)
  eq(r({}), true)
  eq(r({ id: 'talaria-postgres-dev' }), false)
  eq(r({ id: 'talaria-appdb-' }), false)
  eq(r({ id: 'devbox-b1-postgres' }), false)
  eq(r({ containerId: 'x; rm -rf /' }), false)
  eq(r({ containerId: undefined }), false)
  eq(r({ path: '/w/talaria-gone/app-data' }), false)
})

// ── MCP packages a gone worktree or box left running (UB3) ─────────────────
const pkgRoots = { siblingParent: '/w' }
const wtLabels = (dir, inst = 'gone') => ({ 'talaria.mcp.instance': inst, 'talaria.mcp.owner': 'worktree', 'talaria.mcp.checkout': dir })
const boxLabels = (name) => ({ 'talaria.mcp.instance': name, 'talaria.mcp.owner': 'devbox' })
const pkgC = (over = {}) => ({ id: 'c1c1c1c1c1c1c1c1', name: 'talaria-mcp-gone-search', labels: wtLabels('/w/talaria-gone'), running: true, createdMs: NOW - 2 * DAY, ...over })
const pkgN = (over = {}) => ({ id: 'n1n1n1n1n1n1'.replace(/n/g, 'a'), name: 'talaria-mcp-gone', labels: wtLabels('/w/talaria-gone'), createdMs: old, attached: [], ...over })

check("an MCP package's owner is read from its labels: a sibling checkout, a box, or nobody", () => {
  const o = (labels) => JSON.stringify(mcpPkgOwner(labels, pkgRoots))
  eq(o(wtLabels('/w/talaria-gone')), JSON.stringify({ kind: 'checkout', dir: '/w/talaria-gone' }))
  eq(o(boxLabels('b1')), JSON.stringify({ kind: 'box', name: 'b1' }))
  // The primary and an image install label themselves `instance`: never placed.
  eq(o({ 'talaria.mcp.instance': 'talaria', 'talaria.mcp.owner': 'instance' }), 'null')
  // A worktree label that names no checkout, or one outside the sibling root,
  // or a sibling that is not `talaria-<x>`: nothing to place it by.
  eq(o({ 'talaria.mcp.instance': 'x', 'talaria.mcp.owner': 'worktree' }), 'null')
  eq(o(wtLabels('/elsewhere/talaria-gone')), 'null')
  eq(o(wtLabels('/w/talaria-gone/nested')), 'null')
  eq(o(wtLabels('/w/talaria')), 'null')
  eq(o(wtLabels('/w/other-repo')), 'null')
  eq(o(boxLabels('shared')), 'null')
  eq(o(boxLabels('')), 'null')
  eq(o({}), 'null')
  eq(o(undefined), 'null')
})

check('a package whose worktree or box is gone is a row, containers before their network; a live owner keeps its own', () => {
  const { rows, unmarked } = orphanMcpPackages({
    containers: [
      pkgC(),
      pkgC({ id: 'c2c2c2c2c2c2c2c2', name: 'talaria-mcp-gonebox-fetch', labels: boxLabels('gonebox'), running: false }),
      pkgC({ id: 'c3c3c3c3c3c3c3c3', name: 'talaria-mcp-live-search', labels: wtLabels('/w/talaria-live', 'live') }),
      pkgC({ id: 'c4c4c4c4c4c4c4c4', name: 'talaria-mcp-livebox-x', labels: boxLabels('livebox') }),
      pkgC({ id: 'c5c5c5c5c5c5c5c5', name: 'talaria-mcp-talaria-x', labels: { 'talaria.mcp.instance': 'talaria', 'talaria.mcp.owner': 'instance' } }),
      pkgC({ id: 'c6c6c6c6c6c6c6c6', name: 'talaria-mcp-old-x', labels: {} }),
      pkgC({ id: 'c7c7c7c7c7c7c7c7', name: 'talaria-postgres-dev' }),
    ],
    networks: [
      // Only the gone package is attached, and it goes first: removable.
      pkgN({ attached: ['c1c1c1c1c1c1c1c1'] }),
      // Something else still names it, running or stopped: kept.
      pkgN({ id: 'bbbbbbbbbbbb', name: 'talaria-mcp-gonebox', labels: boxLabels('gonebox'), attached: ['c2c2c2c2c2c2c2c2', 'ffffffffffff'] }),
      // Too young: compose-style race guard.
      pkgN({ id: 'cccccccccccc', name: 'talaria-mcp-gone2', labels: wtLabels('/w/talaria-gone2'), createdMs: NOW - 1000 }),
      pkgN({ id: 'dddddddddddd', name: 'talaria-mcp-live', labels: wtLabels('/w/talaria-live', 'live') }),
      pkgN({ id: 'eeeeeeeeeeee', name: 'talaria-mcp-older', labels: {} }),
    ],
    roots: pkgRoots,
    liveCheckout: (dir) => dir === '/w/talaria-live',
    liveBox: (name) => name === 'livebox',
    now: NOW,
  })
  eq(rows.map((r) => `${r.kind}:${r.id}`).join(','), 'orphan-mcp-container:talaria-mcp-gone-search,orphan-mcp-container:talaria-mcp-gonebox-fetch,orphan-mcp-network:talaria-mcp-gone')
  eq(rows[0].why, '/w/talaria-gone is gone')
  eq(rows[1].why, 'box gonebox is gone')
  eq(rows[1].running, false)
  eq(rows[0].path, '')
  eq(rows[0].owner, ownerKey(wtLabels('/w/talaria-gone')))
  eq(ageDays(rows[0].ageMs), '2d')
  eq(unmarked.join(','), 'container talaria-mcp-old-x,network talaria-mcp-older')
})

check('a caller that cannot say whether an owner lives keeps every package', () => {
  eq(orphanMcpPackages({ containers: [pkgC()], networks: [pkgN()], roots: pkgRoots, now: NOW }).rows.length, 0)
})

check('orphan MCP packages go on --apply only, never from the stop gate, and the report says why', () => {
  const { rows } = orphanMcpPackages({ containers: [pkgC()], networks: [pkgN({ attached: ['c1c1c1c1c1c1c1c1'] })], roots: pkgRoots, liveCheckout: () => false, now: NOW })
  eq(rows.length, 2)
  for (const r of rows) eq(classify(r), 'remove')
  const marked = rows.map((r) => ({ ...r, action: 'remove' }))
  eq(selectRemoval(marked, 'gate').length, 0)
  eq(selectRemoval(marked, 'apply').length, 2)
  eq(selectRemoval(marked, 'report').length, 0)
  eq(verdict(marked, { usedPct: 10 }).exit, 0)
  const text = formatReport({ pressure: { usedPct: 10, avail: 1 }, remaining: marked, removed: [], notes: [] })
  if (!text.includes('orphan-mcp-container talaria-mcp-gone-search') || !text.includes('orphan-mcp-network talaria-mcp-gone')) throw new Error(text)
  if (!text.includes('/w/talaria-gone is gone')) throw new Error(text)
})

check('the executor removes a package only by its name and a docker id, never a path', () => {
  const c = (over) => removalAllowed({ kind: 'orphan-mcp-container', id: 'talaria-mcp-gone-search', containerId: 'a1b2c3d4e5f6', path: '', ...over }, roots)
  eq(c({}), true)
  eq(c({ containerId: 'zz1b2c3d4e5f6' }), false)
  eq(c({ containerId: undefined }), false)
  eq(c({ containerId: 'a1b2c3d4e5f6', id: 'talaria-postgres-dev' }), false)
  eq(c({ containerId: 'a1b2c3d4e5f6', id: 'talaria-mcp-' }), false)
  eq(c({ containerId: 'a1b2c3d4e5f6', path: '/w/talaria-gone' }), false)
  const n = (over) => removalAllowed({ kind: 'orphan-mcp-network', id: 'talaria-mcp-gone', networkId: 'a1b2c3d4e5f6', path: '', ...over }, roots)
  eq(n({}), true)
  eq(n({ id: 'talaria' }), false)
  eq(n({ id: 'talaria-dev_default' }), false)
  eq(n({ networkId: 'x; rm -rf /' }), false)
})

// ── Compose projects whose worktree or box is gone (cli-1 / fleet-6) ────────
// 2026-11-02: the gate read `talaria-wt-rel-fleet` (the Postgres and Redis of
// the live worktree `rel-fleet`) as worktree `rel`'s agents, found no
// ../talaria-rel, and removed the stack's containers and volumes at a turn
// boundary. The project names are the ones `docker compose ls` showed.

const live = (names) => (n) => names.includes(n)
const orphans = (names, worktrees, boxes = []) =>
  orphanProjects({ projects: names.map((Name) => ({ Name })), liveWorktree: live(worktrees), liveBox: live(boxes) }).map((r) => r.id)

check('the stack of a live worktree named <x>-fleet is not an orphan (the rel-fleet data loss)', () => {
  eq(orphans(['talaria-wt-rel-fleet'], ['rel-fleet']).join(), '')
  // ...and its own agents' project, one more -fleet out, stays with it.
  eq(orphans(['talaria-wt-rel-fleet-fleet'], ['rel-fleet']).join(), '')
})

check("a worktree's agents stay while the worktree lives; either checkout keeps a -fleet project", () => {
  eq(orphans(['talaria-wt-agent-todo', 'talaria-wt-agent-todo-fleet'], ['agent-todo']).join(), '')
  eq(orphans(['talaria-wt-rel-fleet'], ['rel']).join(), '')
})

check('a talaria-wt-* project is an orphan only when no worktree it could be is live', () => {
  eq(orphans(['talaria-wt-rel-fleet', 'talaria-wt-gone', 'talaria-wt-rel'], ['rel-fleet']).join(), 'talaria-wt-gone,talaria-wt-rel')
  const [row] = orphanProjects({ projects: [{ Name: 'talaria-wt-gone-fleet' }], liveWorktree: () => false, liveBox: () => false })
  eq(row.kind, 'orphan-compose')
  eq(classify(row), 'remove')
  eq(removalAllowed(row, roots), true)
})

check('box projects keep their rule, the tools layer is never an orphan, and other projects are not ours', () => {
  eq(orphans(['devbox-gone', 'devbox-gone-fleet', 'devbox-ops-fleet', 'devbox-shared'], [], ['ops-fleet']).join(), 'devbox-gone,devbox-gone-fleet')
  eq(orphans(['talaria-dogfood', 'talaria-dev', 'supabase_x', 'talaria-fleet'], [], []).join(), '')
})

check('a caller that cannot say whether a worktree or box lives removes nothing', () => {
  eq(orphanProjects({ projects: [{ Name: 'talaria-wt-gone' }, { Name: 'devbox-gone' }] }).length, 0)
})

check('at the moment of removal, a checkout, a box.env or a .talaria-keep still vouches for a project', () => {
  const dir = mkdtempSync(join(tmpdir(), 'talaria-sweep-vouch-'))
  try {
    const where = { siblingParent: join(dir, 'dev'), devboxHome: join(dir, 'devboxes') }
    mkdirSync(join(dir, 'dev', 'talaria-rel-fleet'), { recursive: true })
    // The stack of worktree rel-fleet, by its own checkout.
    eq(projectVouchedBy('talaria-wt-rel-fleet', where), join(dir, 'dev', 'talaria-rel-fleet'))
    // Nothing on disk for this one: removable.
    eq(projectVouchedBy('talaria-wt-gone', where), null)
    eq(projectVouchedBy('talaria-wt-gone-fleet', where), null)
    // A placeholder directory that holds only a .talaria-keep holds the stack
    // whose checkout is gone: the shield that kept rel-fleet's stack alive.
    mkdirSync(join(dir, 'dev', 'talaria-held'))
    writeFileSync(join(dir, 'dev', 'talaria-held', '.talaria-keep'), '')
    eq(projectVouchedBy('talaria-wt-held-fleet', where), join(dir, 'dev', 'talaria-held'))
    // A box directory whose box.env is gone but which holds a .talaria-keep.
    mkdirSync(join(dir, 'devboxes', 'ops'), { recursive: true })
    eq(projectVouchedBy('devbox-ops', where), null)
    eq(boxStands(where.devboxHome, 'ops'), false)
    writeFileSync(join(dir, 'devboxes', 'ops', '.talaria-keep'), '')
    eq(projectVouchedBy('devbox-ops-fleet', where), join(dir, 'devboxes', 'ops', '.talaria-keep'))
    eq(boxStands(where.devboxHome, 'ops'), true)
    mkdirSync(join(dir, 'devboxes', 'mobile'))
    writeFileSync(join(dir, 'devboxes', 'mobile', 'box.env'), '')
    eq(projectVouchedBy('devbox-mobile', where), join(dir, 'devboxes', 'mobile', 'box.env'))
    eq(projectVouchedBy('devbox-shared', where), 'the box tools layer')
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

// ── removeArtifact asks again at the moment of removal (#654) ───────────────
// The scan and the removal are not one instant: the gate sizes every
// candidate between them, and a `talaria worktree` or a `.talaria-keep` can
// land in that gap. These drive removeArtifact itself, with the checkout or
// the keep appearing after the scan. `docker` and `git` are stand-ins on PATH
// that only log their arguments, and a probe proves they are the ones that
// answer before anything else runs, so even a regression here reaches no real
// daemon and no real repository. Each refusal is paired with the same row and
// nothing in the way, which does reach the stand-ins: the log shows that the
// re-check, not a dead PATH, is what stopped the first call.

function withStandIns(fn) {
  const dir = mkdtempSync(join(tmpdir(), 'talaria-sweep-remove-'))
  const bin = join(dir, 'bin')
  const log = join(dir, 'calls.log')
  mkdirSync(bin)
  for (const tool of ['docker', 'git']) {
    writeFileSync(join(bin, tool), `#!/bin/sh\necho "${tool} $*" >> '${log}'\n`)
    chmodSync(join(bin, tool), 0o755)
  }
  const calls = () => (existsSync(log) ? readFileSync(log, 'utf8').split('\n').filter(Boolean) : [])
  const path = process.env.PATH
  process.env.PATH = `${bin}${delimiter}${path}`
  try {
    execFileSync('docker', ['probe'], { stdio: 'ignore' })
    execFileSync('git', ['probe'], { stdio: 'ignore' })
    if (calls().join() !== 'docker probe,git probe') throw new Error(`the stand-ins did not answer: ${calls().join(' | ')}`)
    writeFileSync(log, '')
    const roots = { primary: join(dir, 'primary'), siblingParent: join(dir, 'dev'), devboxHome: join(dir, 'devboxes'), tmp: join(dir, 'tmp') }
    mkdirSync(roots.siblingParent)
    mkdirSync(roots.devboxHome)
    fn({ roots, calls })
  } finally {
    process.env.PATH = path
    rmSync(dir, { recursive: true, force: true })
  }
}

function refuses(fn, pattern) {
  try {
    fn()
  } catch (e) {
    if (!pattern.test(String(e?.message))) throw new Error(`refused, but for another reason: ${e?.message}`)
    return
  }
  throw new Error('removeArtifact went ahead')
}

const POSIX = process.platform !== 'win32' // the stand-ins are sh scripts

check('removeArtifact leaves an orphan project whose checkout or .talaria-keep appeared after the scan', () => {
  if (!POSIX) return
  withStandIns(({ roots, calls }) => {
    const checkout = join(roots.siblingParent, 'talaria-zz-late')
    const box = join(roots.devboxHome, 'zz-late')
    mkdirSync(box) // a box directory with no box.env: not live
    const [wt, bx] = orphanProjects({
      projects: [{ Name: 'talaria-wt-zz-late' }, { Name: 'devbox-zz-late' }],
      liveWorktree: (n) => existsSync(join(roots.siblingParent, `talaria-${n}`)),
      liveBox: (n) => boxStands(roots.devboxHome, n),
    })
    eq(`${wt?.id} ${bx?.id}`, 'talaria-wt-zz-late devbox-zz-late')
    eq(removalAllowed(wt, roots) && removalAllowed(bx, roots), true)
    // After the scan: `talaria worktree zz-late` makes the checkout, and a
    // human holds the box with a .talaria-keep.
    mkdirSync(checkout)
    writeFileSync(join(box, '.talaria-keep'), '')
    refuses(() => removeArtifact(wt, roots), /talaria-zz-late vouches for talaria-wt-zz-late/)
    refuses(() => removeArtifact(bx, roots), /\.talaria-keep vouches for devbox-zz-late/)
    eq(calls().length, 0)
    // The same rows with nothing in the way go to docker.
    rmSync(checkout, { recursive: true })
    rmSync(join(box, '.talaria-keep'))
    removeArtifact(wt, roots)
    removeArtifact(bx, roots)
    const log = calls().join('\n')
    if (!log.includes('label=com.docker.compose.project=talaria-wt-zz-late')) throw new Error(log)
    if (!log.includes('label=com.docker.compose.project=devbox-zz-late')) throw new Error(log)
  })
})

check('removeArtifact leaves a stack, worktree or box a .talaria-keep was touched in after the scan', () => {
  if (!POSIX) return
  withStandIns(({ roots, calls }) => {
    const checkout = join(roots.siblingParent, 'talaria-zz-done')
    const box = join(roots.devboxHome, 'zz-done')
    mkdirSync(checkout)
    mkdirSync(box)
    writeFileSync(join(box, 'box.env'), '')
    // The scan's rows: nothing kept them when it looked.
    const stack = { ...stale({ kind: 'stack', id: 'zz-done', path: '', running: true }), checkout }
    const worktree = stale({ kind: 'worktree', id: 'zz-done', path: checkout })
    const devbox = stale({ kind: 'devbox', id: 'zz-done', path: box })
    for (const row of [stack, worktree, devbox]) {
      eq(classify(row), 'remove')
      eq(removalAllowed(row, roots), true)
    }
    writeFileSync(join(checkout, '.talaria-keep'), '')
    writeFileSync(join(box, '.talaria-keep'), '')
    for (const row of [stack, worktree, devbox]) refuses(() => removeArtifact(row, roots), /\.talaria-keep holds it/)
    eq(calls().length, 0)
    eq(existsSync(checkout) && existsSync(box), true)
    // With the keeps gone, each goes ahead: the stack is downed, the
    // worktree removed through git, the box downed and its directory gone.
    rmSync(join(checkout, '.talaria-keep'))
    rmSync(join(box, '.talaria-keep'))
    removeArtifact(stack, roots)
    if (!calls().some((c) => c.startsWith('docker compose -p talaria-wt-zz-done ') && c.endsWith(' down -v'))) throw new Error(calls().join('\n'))
    removeArtifact(worktree, roots)
    if (!calls().includes(`git -C ${roots.primary} worktree remove --force ${checkout}`)) throw new Error(calls().join('\n'))
    eq(existsSync(checkout), false)
    removeArtifact(devbox, roots)
    if (!calls().some((c) => c.startsWith('docker compose -p devbox-zz-done ') && c.endsWith(' down -v --remove-orphans'))) throw new Error(calls().join('\n'))
    eq(existsSync(box), false)
  })
})

// ── A merged worktree's offload refs (bun run gate:remote) ─────────────────
// `git ls-remote origin 'refs/heads/ci/*wt/<name>'` is a tail glob whose `*`
// crosses `/`, so it lists other branches' refs too. Only the exact pair goes.

check('offloadRefsFor: exactly ci/<branch> and ci/full/<branch>, never a look-alike', () => {
  const listing = [
    'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\trefs/heads/ci/wt/x',
    'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\trefs/heads/ci/full/wt/x',
    'cccccccccccccccccccccccccccccccccccccccc\trefs/heads/ci/xwt/x',
    'dddddddddddddddddddddddddddddddddddddddd\trefs/heads/ci/full/a/wt/x',
    'eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee\trefs/heads/ci/wt/x-2',
    '',
  ]
  eq(offloadRefsFor('wt/x', listing).join(), 'refs/heads/ci/wt/x,refs/heads/ci/full/wt/x')
  eq(offloadRefsFor('wt/x', listing.join('\n')).join(), 'refs/heads/ci/wt/x,refs/heads/ci/full/wt/x')
  eq(offloadRefsFor('wt/x', [listing[1]]).join(), 'refs/heads/ci/full/wt/x')
  eq(offloadRefsFor('wt/x', [listing[2], listing[3], listing[4]]).length, 0)
  eq(offloadRefsFor('wt/x', '').length, 0)
  eq(offloadRefsFor('wt/x', undefined).length, 0)
})

check('a merged stack or worktree takes its offload refs; a stale one, and a failed listing, do not stop it', () => {
  if (!POSIX) return
  withStandIns(({ roots, calls }) => {
    const dir = dirname(roots.primary)
    const log = join(dir, 'calls.log')
    // git answers ls-remote with the pair and a look-alike.
    const answering = [
      '#!/bin/sh',
      `echo "git $*" >> '${log}'`,
      'if [ "$3" = "ls-remote" ]; then',
      "  printf 'aaaa\\trefs/heads/ci/wt/zz-merged\\n'",
      "  printf 'bbbb\\trefs/heads/ci/full/wt/zz-merged\\n'",
      "  printf 'cccc\\trefs/heads/ci/xwt/zz-merged\\n'",
      'fi',
      '',
    ].join('\n')
    writeFileSync(join(dir, 'bin', 'git'), answering)
    const checkout = join(roots.siblingParent, 'talaria-zz-merged')
    mkdirSync(checkout)
    const stack = { ...stale({ kind: 'stack', id: 'zz-merged', path: '', running: true, merged: true }), checkout }
    const notes = removeArtifact(stack, roots)
    const pushed = `git -C ${roots.primary} push --quiet origin :refs/heads/ci/wt/zz-merged :refs/heads/ci/full/wt/zz-merged`
    if (!calls().includes(`git -C ${roots.primary} ls-remote origin refs/heads/ci/*wt/zz-merged`)) throw new Error(calls().join('\n'))
    if (!calls().includes(pushed)) throw new Error(calls().join('\n'))
    eq(notes.join(), 'deleted the offload refs of merged wt/zz-merged: ci/wt/zz-merged, ci/full/wt/zz-merged')
    // The checkout's own teardown (--apply) asks again: the refs may be back.
    writeFileSync(log, '')
    removeArtifact(stale({ kind: 'worktree', id: 'zz-merged', path: checkout, merged: true }), roots)
    if (!calls().includes(pushed)) throw new Error(calls().join('\n'))
    // A stale, unmerged stack is the daily prune's: no network call at all.
    writeFileSync(log, '')
    eq(removeArtifact({ ...stale({ kind: 'stack', id: 'zz-stale', path: '', running: true, merged: false }), checkout }, roots).length, 0)
    if (calls().some((c) => c.includes('ls-remote') || c.includes(' push '))) throw new Error(calls().join('\n'))
    // origin unreachable: a note, never a throw — the stack is already down.
    writeFileSync(join(dir, 'bin', 'git'), ['#!/bin/sh', `echo "git $*" >> '${log}'`, 'echo "fatal: unable to access origin" >&2', 'exit 128', ''].join('\n'))
    const down = removeArtifact({ ...stale({ kind: 'stack', id: 'zz-merged', path: '', running: true, merged: true }), checkout }, roots)
    eq(down.length, 1)
    if (!/could not list the offload refs of wt\/zz-merged \(the daily prune removes them\): fatal: unable to access origin/.test(down[0])) throw new Error(down[0])
  })
})

check('a box whose box.env is gone but which holds a .talaria-keep keeps its networks', () => {
  const gone = net('devbox-ops_default', 'devbox-ops')
  eq(scanNets([gone], [], [], []).join(), 'devbox-ops_default')
  const dir = mkdtempSync(join(tmpdir(), 'talaria-sweep-keep-'))
  try {
    mkdirSync(join(dir, 'ops'))
    writeFileSync(join(dir, 'ops', '.talaria-keep'), '')
    const rows = orphanNetworks({ networks: [gone], containers: [], liveWorktree: () => false, liveBox: (n) => boxStands(dir, n), now: NOW })
    eq(rows.length, 0)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

check("a live worktree named <x>-fleet keeps its stack's network too", () => {
  eq(scanNets([net('talaria-wt-rel-fleet_default', 'talaria-wt-rel-fleet')], [], ['rel-fleet']).length, 0)
  eq(scanNets([net('talaria-wt-rel-fleet_default', 'talaria-wt-rel-fleet')], [], ['rel']).length, 0)
  eq(scanNets([net('talaria-wt-rel-fleet_default', 'talaria-wt-rel-fleet')], [], []).join(), 'talaria-wt-rel-fleet_default')
})

// ── Merged means THIS commit's PR merged (cli-4) ────────────────────────────
// `gh pr list --head <branch> --state all --json number,state,headRefOid`,
// as GitHub answered on 2026-11-02. wt/r6-ui merged as #629; a recut of
// wt/r6-ui from rc sits on rc's tip with 0 commits ahead, and reading the
// branch name alone called it finished. wt/work-view was cut twice and merged
// twice (#495, #501).

const RC_TIP = '678dd627166123da1f47523acd21a2c0ebc7844b'
const R6_UI = [{ headRefOid: '48aa4af1613494d5e08fbde75ad924df46e69ee6', number: 629, state: 'MERGED' }]
const WORK_VIEW = [
  { headRefOid: 'b3b4167bc552e4bb37cc5a33c043423622928fef', number: 501, state: 'MERGED' },
  { headRefOid: '4b7aa325918317163fe61e2023a3725572c7fb11', number: 495, state: 'MERGED' },
]

check('a recut wt/<name> is not merged because an older PR of that name merged', () => {
  eq(mergedAtHead(R6_UI, RC_TIP), false)
  eq(mergedAtHead(WORK_VIEW, RC_TIP), false)
})

check('the tree a merged PR was pushed from is merged — whichever of its PRs that was', () => {
  eq(mergedAtHead(R6_UI, '48aa4af1613494d5e08fbde75ad924df46e69ee6'), true)
  eq(mergedAtHead(WORK_VIEW, '4b7aa325918317163fe61e2023a3725572c7fb11'), true)
  eq(mergedAtHead(WORK_VIEW, 'b3b4167bc552e4bb37cc5a33c043423622928fef'), true)
})

check('an open or closed PR at the head, no PR, or no answer is not merged', () => {
  const head = '48aa4af1613494d5e08fbde75ad924df46e69ee6'
  eq(mergedAtHead([{ ...R6_UI[0], state: 'OPEN' }], head), false)
  eq(mergedAtHead([{ ...R6_UI[0], state: 'CLOSED' }], head), false)
  eq(mergedAtHead([], head), false)
  eq(mergedAtHead(null, head), false)
  eq(mergedAtHead(R6_UI, ''), false)
})

check('the merged prefilter: 0 ahead of rc or of testing may be merged; an unresolved base is no answer', () => {
  // merged into testing: 0 ahead of it, still ahead of rc until the promotion
  eq(mergedPrefilter(3, 0), true)
  eq(mergedPrefilter(null, 0), true)
  // merged into rc (before the cutover), and a clone with no testing ref
  eq(mergedPrefilter(0, 5), true)
  eq(mergedPrefilter(0, null), true)
  // ahead of both: unfinished, never asked of gh
  eq(mergedPrefilter(3, 5), false)
  // neither base resolves: nothing to say, so not merged
  eq(mergedPrefilter(null, null), false)
})

check('build caches are reported at any disk reading', () => {
  const text = formatReport({
    pressure: { usedPct: 15, avail: 500 * 1024 ** 3 },
    remaining: [{ ...stale({ kind: 'target', id: 'api/target', bytes: 34 * 1024 ** 3, primary: true }), action: 'flag' }],
    removed: [],
    notes: [],
  })
  // The whole point: visible at 15%, not only once the disk is nearly full.
  if (!text.includes('build caches')) throw new Error(text)
  if (!text.includes('34 GiB')) throw new Error(text)
  if (!text.includes('cargo clean')) throw new Error(text)
  if (text.includes('nothing stale.')) throw new Error(text)
})


// ── Build caches: a finished PR's goes first; over the line, LRU idle ones ──
const HOUR = 60 * 60 * 1000
const cache = (over = {}) => ({
  kind: 'target',
  id: 'talaria-x/api/target',
  path: '/work/talaria-x/api/target',
  owner: '/work/talaria-x',
  bytes: 5 * 1024 ** 3,
  ageMs: 2 * HOUR,
  busy: false,
  stackRunning: false,
  pr: null,
  dirty: false,
  unpushed: false,
  inspected: true,
  kept: false,
  current: false,
  primary: false,
  foreign: false,
  ...over,
})

check('a merged or closed PR drops its idle cache; open, none, or kept keeps it', () => {
  eq(classify(cache({ pr: 'merged' })), 'remove')
  eq(classify(cache({ pr: 'closed' })), 'remove')
  eq(classify(cache({ pr: 'open' })), 'keep')
  eq(classify(cache({ pr: null })), 'keep')
  eq(classify(cache({ pr: 'merged', kept: true })), 'keep')
  eq(classify(cache({ pr: 'open', bytes: POLICY.oversizedTargetBytes })), 'flag')
})

check('a finished PR in a dirty or unpushed tree: a human decides', () => {
  eq(classify(cache({ pr: 'merged', dirty: true })), 'flag')
  eq(classify(cache({ pr: 'closed', unpushed: true })), 'flag')
})

check('never a cache a cargo builds in, one used this hour, one whose stack runs', () => {
  eq(classify(cache({ pr: 'merged', busy: true })), 'keep')
  // null: the locks could not be read (not Linux). Unknown is not idle,
  // and that holds for the 14-day clock too.
  // null: the lock check cannot be trusted (no /proc/locks off Linux, or
  // btrfs). What would have gone is flagged for a human, never deleted.
  eq(classify(cache({ pr: 'merged', busy: null })), 'flag')
  eq(classify(cache({ ageMs: POLICY.staleTargetMs + DAY, busy: null })), 'flag')
  eq(classify(cache({ pr: 'open', busy: null })), 'keep')
  eq(classify(cache({ pr: 'merged', ageMs: POLICY.targetIdleMs - 1 })), 'keep')
  eq(classify(cache({ pr: 'merged', stackRunning: true })), 'keep')
  eq(classify(cache({ ageMs: POLICY.staleTargetMs + DAY, stackRunning: true })), 'keep')
})

check('never the primary, the current checkout, or a checkout talaria worktree did not make', () => {
  eq(classify(cache({ pr: 'merged', primary: true })), 'keep')
  eq(classify(cache({ pr: 'merged', current: true })), 'keep')
  eq(classify(cache({ pr: 'merged', foreign: true })), 'keep')
  eq(classify(cache({ ageMs: POLICY.staleTargetMs + DAY, foreign: true })), 'keep')
  eq(classify(cache({ foreign: true, bytes: POLICY.oversizedTargetBytes })), 'flag')
})

check('over the line: idle clean caches, unseeded then least recently used first; a keep is absolute', () => {
  const over = { usedPct: POLICY.pressurePct + 1 }
  const rows = [
    cache({ id: 'recent', ageMs: 3 * HOUR, pr: 'open' }),
    cache({ id: 'oldest', ageMs: 9 * DAY, kept: true }),
    cache({ id: 'middle', ageMs: 2 * DAY }),
    cache({ id: 'dirty', ageMs: 20 * DAY, dirty: true }),
    cache({ id: 'busy', ageMs: 20 * DAY, busy: true }),
    cache({ id: 'hot', ageMs: 10 * 60 * 1000 }),
    cache({ id: 'primary', ageMs: 30 * DAY, primary: true }),
    cache({ id: 'foreign', ageMs: 30 * DAY, foreign: true }),
    cache({ id: 'uninspected', ageMs: 30 * DAY, inspected: false }),
    cache({ id: 'seeded-old', ageMs: 5 * DAY, seeded: true }),
  ].map((a) => ({ ...a, action: classify(a) }))
  eq(pressureDrops(rows, over).map((a) => a.id).join(','), 'middle,recent,seeded-old')
  eq(pressureDrops(rows, { usedPct: POLICY.pressurePct - 1 }).length, 0)
  // The kept tree's cache is reported, flagged, and never dropped.
  eq(pressureKept(rows, over).map((a) => a.id).join(','), 'oldest')
  const text = formatReport({ pressure: { ...over, avail: 1, total: 2, used: 1 }, remaining: rows, removed: [], notes: [] })
  if (!/flagged, not dropped: idle build caches a `.talaria-keep` holds[\s\S]*oldest/.test(text)) throw new Error(text)
})

check('the pressure loop: at most pressureDropMax, and it stops when a drop frees too little', () => {
  const GiB = 1024 ** 3
  const over = { usedPct: POLICY.pressurePct + 5, avail: 10 * GiB }
  const rows = ['a', 'b', 'c', 'd', 'e'].map((id, i) => ({ ...cache({ id, ageMs: (10 - i) * DAY }), action: 'keep' }))
  // Every drop frees 2 GiB and the disk stays over: the cap stops it.
  let avail = over.avail
  let r = runPressureDrops(rows, over, { remove: () => {}, read: () => ({ usedPct: over.usedPct, avail: (avail += 2 * GiB) }) })
  eq(r.dropped.map((a) => a.id).join(','), 'a,b,c')
  if (!/stopped: 3 caches is this run's limit; left: d .*, e /.test(r.notes[0])) throw new Error(r.notes[0])
  // The first drop frees almost nothing: the pressure is not build caches.
  r = runPressureDrops(rows, over, { remove: () => {}, read: () => ({ usedPct: over.usedPct, avail: over.avail + 1024 }) })
  eq(r.dropped.map((a) => a.id).join(','), 'a')
  if (!/build caches are not what fills the disk/.test(r.notes[0])) throw new Error(r.notes[0])
  // Back under the line after one: stops there.
  r = runPressureDrops(rows, over, { remove: () => {}, read: () => ({ usedPct: POLICY.pressurePct - 1, avail: 99 * GiB }) })
  eq(r.dropped.length, 1)
  if (!/back under/.test(r.notes[0])) throw new Error(r.notes[0])
  // A failed removal is reported and does not count toward the cap.
  r = runPressureDrops(rows, over, {
    remove: (a) => {
      if (a.id === 'a') throw new Error('busy')
    },
    read: () => ({ usedPct: over.usedPct, avail: (avail += 2 * GiB) }),
  })
  eq(r.dropped.map((a) => a.id).join(','), 'b,c,d')
  eq(r.failed.length, 1)
})

check('a removable checkout counts its build dir once, and the build dir goes first', () => {
  const wt = { ...stale({ path: '/work/talaria-x', merged: true }), bytes: 10, action: 'remove' }
  const t = { ...cache({ pr: 'merged', bytes: 7 }), action: 'remove' }
  eq(verdict([wt, t], { usedPct: 10 }).removableBytes, 10)
  eq(selectRemoval([wt, t], 'apply')[0].kind, 'target')
})

check('PR state is the PR at this HEAD; an old PR of a reused name finishes nothing', () => {
  const prs = [
    { number: 1, state: 'MERGED', headRefName: 'wt/a', headRefOid: 'aaa' },
    { number: 2, state: 'CLOSED', headRefName: 'feat/b', headRefOid: 'bbb' },
    { number: 3, state: 'OPEN', headRefName: 'feat/c', headRefOid: 'ccc' },
  ]
  eq(prStateAt(prs, 'wt/a', 'aaa'), 'merged')
  eq(prStateAt(prs, 'wt/a', 'a-later-commit'), null)
  eq(prStateAt(prs, 'feat/b', 'bbb'), 'closed')
  eq(prStateAt(prs, 'feat/c', 'whatever'), 'open')
  eq(prStateAt(prs, 'feat/none', 'x'), null)
  eq(prStateAt(null, 'wt/a', 'aaa'), null)
})

check('btrfs: the lock check cannot be trusted, so a target reads as unknown', () => {
  eq(targetBusy('/nowhere', '', BTRFS_SUPER_MAGIC), null)
  eq(targetBusy('/nowhere', null), null)
  eq(targetBusy('/nowhere', '', 0x58465342), false) // XFS, nothing held
})

// The real lock: a `flock -x` holder, as cargo takes .cargo-build-lock.
const haveFlock = spawnSync('sh', ['-c', 'command -v flock']).status === 0 && existsSync('/proc/locks')
function holdLock(file) {
  // Its own process group: `sleep` inherits the lock fd, so the group goes.
  const child = spawn('flock', ['-x', file, 'sleep', '30'], { stdio: 'ignore', detached: true })
  for (let i = 0; i < 100 && targetBusy(dirname(dirname(file))) !== true; i++) spawnSync('sleep', ['0.05'])
  return child
}

check('dropTarget renames, re-checks, and removes; a held lock keeps the dir where it was', () => {
  if (!haveFlock) return
  const root = mkdtempSync(join(tmpdir(), 'talaria-sweeptest-'))
  try {
    const t = join(root, 'talaria-x', 'api', 'target')
    mkdirSync(join(t, 'debug', 'deps'), { recursive: true })
    writeFileSync(join(t, 'debug', '.cargo-build-lock'), '')
    const holder = holdLock(join(t, 'debug', '.cargo-build-lock'))
    try {
      let threw = false
      try {
        dropTarget(t)
      } catch {
        threw = true
      }
      eq(threw, true)
      eq(existsSync(join(t, 'debug', 'deps')), true)
      // ...and a merged worktree holding that busy target is not removed either.
      const wt = { ...stale({ path: join(root, 'talaria-x'), id: 'x', merged: true }), action: 'remove' }
      let why = ''
      try {
        removeArtifact(wt, { primary: join(root, 'talaria'), current: '/elsewhere', siblingParent: root, devboxHome: join(root, 'devboxes'), tmp: tmpdir() })
      } catch (e) {
        why = String(e?.message ?? e)
      }
      if (!why.includes('checkout left in place')) throw new Error(why)
      eq(existsSync(t), true)
    } finally {
      process.kill(-holder.pid, 'SIGKILL')
    }
    for (let i = 0; i < 100 && targetBusy(t) !== false; i++) spawnSync('sleep', ['0.05'])
    dropTarget(t)
    eq(existsSync(t), false)
    eq(readdirSync(join(root, 'talaria-x', 'api')).length, 0) // no .target-dropping-* left
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

check('a leftover .target-dropping-* is a removable target of its own', () => {
  eq(classify(cache({ dropping: true, busy: false, kept: true, pr: 'open' })), 'remove')
  eq(classify(cache({ dropping: true, busy: true })), 'flag')
  const roots = { primary: '/work/talaria', current: '/work/talaria', siblingParent: '/work' }
  eq(removalAllowed({ kind: 'target', path: '/work/talaria-x/api/.target-dropping-123' }, roots), true)
  eq(removalAllowed({ kind: 'target', path: '/work/talaria-x/api/.target-dropping-x' }, roots), false)
  eq(removalAllowed({ kind: 'target', path: '/work/talaria/api/.target-dropping-1' }, roots), false)
})

check('cargo locks from /proc/locks, and the worktree marker', () => {
  const held = heldFlocks('1: FLOCK  ADVISORY  WRITE 3512 fc:00:621256963 0 EOF\n18:  -> FLOCK  ADVISORY  WRITE 9 fc:00:7 0 EOF\n2: POSIX  ADVISORY  WRITE 1 00:a0:5 0 EOF\n')
  eq(held.has('fc:00:621256963'), true)
  eq(held.has('fc:00:7'), true)
  eq(held.has('00:a0:5'), false)
  eq(procLocksDev(64512), 'fc:00')
  eq(worktreeMarker('PORT=5306\nTALARIA_WORKTREE=docnotice\n'), 'docnotice')
  eq(worktreeMarker('PORT=5306\n'), null)
})

check('--help and --pressure exit without deleting', () => {
  const script = resolve(dirname(fileURLToPath(import.meta.url)), 'cleanup-sweep.mjs')
  const help = spawnSync(process.execPath, [script, '--help'], { encoding: 'utf8' })
  eq(help.status, 0)
  if (!help.stdout.includes('--apply')) throw new Error(help.stdout)
  const pressure = spawnSync(process.execPath, [script, '--pressure'], { encoding: 'utf8' })
  eq(pressure.status, 0)
  if (pressure.stdout && !pressure.stdout.includes('disk')) throw new Error(pressure.stdout)
})

if (failed) {
  console.error(`cleanup-sweep: ${failed} of ${ran} failed`)
  process.exit(1)
}
console.log(`cleanup-sweep: ${ran} checks, all clean`)