// The sweep's dangerous edges: what it will delete, what it must not, and
// when the stop gate has to say no. A wrong answer here fills the disk or
// removes someone else's checkout. Run by `bun run check`.
import { spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  POLICY,
  ageDays,
  appDbOwner,
  boxProjectOwners,
  checkoutStands,
  classify,
  formatReport,
  orphanAppDbs,
  orphanNetworks,
  parseDockerTime,
  parseLabels,
  removalAllowed,
  selectRemoval,
  verdict,
  worktreeName,
  worktreeProjectOwner,
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

check("a worktree's fleet project belongs to the worktree, not to a worktree named -fleet", () => {
  eq(worktreeProjectOwner('talaria-wt-agent-todo'), 'agent-todo')
  eq(worktreeProjectOwner('talaria-wt-agent-todo-fleet'), 'agent-todo')
  eq(worktreeProjectOwner('talaria-dogfood-fleet'), null)
  eq(worktreeProjectOwner('talaria'), null)
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