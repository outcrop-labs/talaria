// The sweep's dangerous edges: what it will delete, what it must not, and
// when the stop gate has to say no. A wrong answer here fills the disk or
// removes someone else's checkout. Run by `bun run check`.
import { spawnSync } from 'node:child_process'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  POLICY,
  classify,
  formatReport,
  removalAllowed,
  selectRemoval,
  verdict,
  worktreeName,
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