// flow-guard's rules, against a literal commit graph — no git, no network —
// plus the two things only a real process can show: the CLI maps a verdict to
// its exit code, and a run from a symlinked path still runs the guard. That
// last one is the fail-open this file exists for: the CLI runs only when the
// file is invoked directly, and a check that mistook a symlinked run for an
// import would exit 0 having judged nothing.
// Run by `bun run check`.
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { decidePrBase, decidePush, prOptsFromEnv } from './flow-guard.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const GUARD = join(ROOT, 'scripts', 'flow-guard.mjs')
const ZERO = '0'.repeat(40)

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

function ok(cond, what) {
  if (!cond) throw new Error(what)
}

/** A verdict's kind, for one-line assertions. */
const kind = (v) => (v.ok === true ? 'ok' : v.ok === false ? 'block' : 'could-not-run')
const text = (v) => (v.lines ?? []).join('\n')

// ── the graph ───────────────────────────────────────────────────────────────

/** A commit's sha, from its name: real-shaped, stable, readable in a failure. */
const sha = (name) => createHash('sha1').update(name).digest('hex')

/**
 *   base                     the root; testing's stale tip sits here
 *   f1      <- base          a feature commit
 *   rcM     <- base, f1      a pull request's merge into rc
 *   r2      <- rcM           a non-merge commit on rc
 *   mainM   <- base, rcM     the promotion merge: rcM's tree, nothing else
 *   mainBad <- base, rcM     a promotion merge with an edit smuggled in
 *   s1      <- base          a commit on main only (what a sync of main carries)
 *   x1      <- base          a stray commit, on no long-lived branch
 *   tM      <- base, x1      a merge of the stray (what a PR merge into testing leaves)
 *
 * The lanes into rc, after the cutover — testing (t2) is ahead of rc (r2):
 *   t1      <- r2            a feature commit, on testing
 *   t2      <- r2, t1        testing's tip: the feature's merge (a snapshot is cut here)
 *   h1      <- r2            a hotfix cut from rc
 *   hBad    <- t1            a "hotfix" cut from testing: it carries t1
 *   rcSnap  <- r2, t2        the snapshot's merge into rc: t2's tree, nothing else
 *   rcSnapX <- h1, t2        a snapshot merged over a hotfix testing lacks: neither tree
 *   rcHot   <- r2, h1        the hotfix's merge into rc
 */
const GRAPH = {
  base: { parents: [], tree: 'T-base' },
  f1: { parents: ['base'], tree: 'T-f1' },
  rcM: { parents: ['base', 'f1'], tree: 'T-rc' },
  r2: { parents: ['rcM'], tree: 'T-r2' },
  mainM: { parents: ['base', 'rcM'], tree: 'T-rc' },
  mainBad: { parents: ['base', 'rcM'], tree: 'T-edited' },
  s1: { parents: ['base'], tree: 'T-s1' },
  x1: { parents: ['base'], tree: 'T-x1' },
  tM: { parents: ['base', 'x1'], tree: 'T-tM' },
  t1: { parents: ['r2'], tree: 'T-t1' },
  t2: { parents: ['r2', 't1'], tree: 'T-t2' },
  h1: { parents: ['r2'], tree: 'T-h1' },
  hBad: { parents: ['t1'], tree: 'T-hbad' },
  rcSnap: { parents: ['r2', 't2'], tree: 'T-t2' },
  rcSnapX: { parents: ['h1', 't2'], tree: 'T-t2+h1' },
  rcHot: { parents: ['r2', 'h1'], tree: 'T-rchot' },
}
const REFS = { 'origin/rc': 'r2', 'origin/main': 's1', 'origin/testing': 't2' }

/** What `git merge-tree --write-tree ours theirs` would write, over GRAPH:
 *  theirs' own tree when ours is already in its history (rc adds nothing),
 *  ours' when theirs is in ours, a conflict (null) for the pairs named in
 *  CONFLICTS, and otherwise a tree that is neither side's. */
const CONFLICTS = new Set()

/** The git adapter flow-guard reads, over GRAPH, with the origin refs given
 *  (by commit name). Unknown shas are not commits, as git would say. */
function fakeGit(refs = REFS) {
  const bySha = new Map(Object.keys(GRAPH).map((name) => [sha(name), name]))
  const node = (s) => {
    const name = bySha.get(s)
    if (!name) throw new Error(`fake git: unknown commit ${s}`)
    return GRAPH[name]
  }
  const ancestry = (s) => {
    const seen = new Set()
    const walk = (x) => {
      if (seen.has(x)) return
      seen.add(x)
      for (const p of node(x).parents) walk(sha(p))
    }
    walk(s)
    return seen
  }
  const resolveRef = (ref) => (ref in refs ? sha(refs[ref]) : ref)
  return {
    isCommit: (s) => bySha.has(s),
    isAncestor: (s, ref) => {
      const tip = resolveRef(ref)
      return bySha.has(tip) && ancestry(tip).has(s)
    },
    isMerge: (s) => node(s).parents.length > 1,
    revList: (after, before) => {
      const old = ancestry(before)
      return [...ancestry(after)].filter((s) => !old.has(s))
    },
    subject: (s) => `subject of ${bySha.get(s)}`,
    hasRef: (ref) => ref in refs,
    tree: (rev) => {
      const second = /^(.+)\^2$/.exec(rev)
      return second ? GRAPH[node(second[1]).parents[1]].tree : node(rev).tree
    },
    secondParent: (s) => sha(node(s).parents[1]),
    mergeTree: (ours, theirs) => {
      if (CONFLICTS.has(`${ours}:${theirs}`)) return null
      if (ancestry(theirs).has(ours)) return node(theirs).tree
      if (ancestry(ours).has(theirs)) return node(ours).tree
      return `T-merge(${bySha.get(ours)},${bySha.get(theirs)})`
    },
  }
}

const push = (branch, before, after, refs) =>
  decidePush(branch.startsWith('refs/') ? branch : `refs/heads/${branch}`, before, after, fakeGit(refs))
const S = sha
/** A pull request into rc, judged over GRAPH: base and head named by commit. */
const intoRc = (head, baseName, headName, opts = {}, refs) =>
  decidePrBase('rc', head, { baseSha: S(baseName), headSha: S(headName), ...opts }, fakeGit(refs))

// ── pr-base ─────────────────────────────────────────────────────────────────

check('pr-base: a feature branch into testing passes', () => {
  eq(kind(decidePrBase('testing', 'wt/x', {})), 'ok')
})

check('pr-base: a feature branch into rc is blocked, with the retarget', () => {
  for (const head of ['wt/x', 'agent/x', 'feat/x', 'talaria/x', 'ci/wt/x', 'backflow/rc-0123456789ab']) {
    const v = decidePrBase('rc', head, {})
    eq([head, kind(v)], [head, 'block'])
    ok(text(v).includes('feature pull requests target `testing`'), text(v))
    ok(text(v).includes('gh pr edit <n> --base testing'), text(v))
  }
  // with both shas too: the head's name decides before any git is read
  eq(kind(intoRc('wt/x', 'r2', 't1')), 'block')
})

check('pr-base: testing itself into rc is blocked — promote a frozen snapshot', () => {
  const v = intoRc('testing', 'r2', 't2')
  eq(kind(v), 'block')
  ok(text(v).includes('promote a frozen snapshot, never the moving branch'), text(v))
  ok(text(v).includes('gh workflow run promote-testing.yml --ref main -f open=true'), text(v))
  ok(text(v).includes('refs/heads/promote/testing-$(git rev-parse --short=12 origin/testing)'), text(v))
})

check('pr-base: a snapshot on testing that rc adds nothing to passes', () => {
  eq(kind(intoRc('promote/testing-0123456789ab', 'r2', 't2')), 'ok')
  // rc's own tip, on testing (right after the cutover's fast-forward)
  eq(kind(intoRc('promote/testing-0123456789ab', 'r2', 'r2')), 'ok')
})

check('pr-base: a snapshot when rc has content testing lacks is blocked, naming the back-flow', () => {
  const v = intoRc('promote/testing-0123456789ab', 'rcHot', 't2')
  eq(kind(v), 'block')
  ok(text(v).includes('rc has content testing lacks (a hotfix or a roll)'), text(v))
  ok(text(v).includes('`sync: rc → testing` from backflow/rc-*'), text(v))
})

check('pr-base: a snapshot whose merge into rc conflicts is blocked too', () => {
  CONFLICTS.add(`${S('rcHot')}:${S('t2')}`)
  try {
    const v = intoRc('promote/testing-0123456789ab', 'rcHot', 't2')
    eq(kind(v), 'block')
    ok(text(v).includes('conflicts with the snapshot'), text(v))
  } finally {
    CONFLICTS.clear()
  }
})

check('pr-base: a snapshot head that is not on testing is blocked', () => {
  const v = intoRc('promote/testing-0123456789ab', 'r2', 'h1')
  eq(kind(v), 'block')
  ok(text(v).includes('is not on testing, so it is not a snapshot of it'), text(v))
})

check('pr-base: a snapshot from another repository is blocked', () => {
  const v = intoRc('promote/testing-0123456789ab', 'r2', 't2', { sameRepo: false })
  eq(kind(v), 'block')
  ok(text(v).includes('from another repository cannot be merged into `rc`'), text(v))
  for (const head of ['hotfix/x', 'release/roll-1', 'sync/main-into-rc', 'testing']) {
    eq([head, kind(intoRc(head, 'r2', 'h1', { sameRepo: false }))], [head, 'block'])
  }
})

check('pr-base: a snapshot without origin/testing blocks with the fetch', () => {
  const v = intoRc('promote/testing-0123456789ab', 'r2', 't2', {}, { 'origin/rc': 'r2', 'origin/main': 's1' })
  eq(kind(v), 'block')
  ok(text(v).includes('git fetch origin main rc testing'), text(v))
})

check('pr-base: a sha the event names that this clone lacks blocks with the fetch', () => {
  const v = decidePrBase('rc', 'promote/testing-x', { baseSha: S('r2'), headSha: S('gone') }, fakeGit())
  eq(kind(v), 'block')
  ok(text(v).includes('not in this repository'), text(v))
  eq(kind(decidePrBase('rc', 'hotfix/x', { baseSha: S('gone'), headSha: S('h1') }, fakeGit())), 'block')
})

check('pr-base: a snapshot or a lane without the shas passes on its name, with a note', () => {
  for (const head of ['promote/testing-0123456789ab', 'hotfix/x', 'release/roll-0.14.0', 'sync/main-into-rc']) {
    const v = decidePrBase('rc', head, {})
    eq([head, kind(v)], [head, 'ok'])
    ok(typeof v.note === 'string' && v.note.includes('was not checked'), `${head}: ${JSON.stringify(v)}`)
  }
})

check('pr-base: a hotfix, a roll and a sync cut from rc pass', () => {
  for (const head of ['hotfix/x', 'release/roll-0.14.0', 'sync/main-into-rc']) {
    eq([head, kind(intoRc(head, 'r2', 'h1'))], [head, 'ok'])
  }
  // rc's own tip, as the verification's `pr-base rc hotfix/x <rc> <rc>`
  eq(kind(intoRc('hotfix/x', 'r2', 'r2')), 'ok')
})

check('pr-base: a hotfix carrying a testing commit is blocked, naming it', () => {
  const v = intoRc('hotfix/x', 'r2', 'hBad')
  eq(kind(v), 'block')
  ok(text(v).includes('cut hotfixes and rolls from origin/rc — this head carries testing-only commits into rc'), text(v))
  ok(text(v).includes(S('t1').slice(0, 12)), `names the commit: ${text(v)}`)
  ok(!text(v).includes(S('hBad').slice(0, 12)), `the hotfix's own commit is not testing's: ${text(v)}`)
  for (const head of ['release/roll-x', 'sync/x']) eq([head, kind(intoRc(head, 'r2', 'hBad'))], [head, 'block'])
})

check('pr-base: a hotfix without origin/testing blocks with the fetch', () => {
  eq(kind(intoRc('hotfix/x', 'r2', 'h1', {}, { 'origin/rc': 'r2', 'origin/main': 's1' })), 'block')
})

check('pr-base: a feature branch into main is blocked', () => {
  const v = decidePrBase('main', 'wt/x', {})
  eq(kind(v), 'block')
  ok(text(v).includes('gh pr create --base main --head rc'), `the message names the command: ${text(v)}`)
})

check('pr-base: rc into main passes', () => {
  eq(kind(decidePrBase('main', 'rc', { sameRepo: true })), 'ok')
})

check("pr-base: a fork's branch named rc into main is blocked", () => {
  const v = decidePrBase('main', 'rc', { sameRepo: false })
  eq(kind(v), 'block')
  ok(text(v).includes('another repository'), text(v))
})

check('pr-base: a [skip ci] title into testing is blocked', () => {
  const v = decidePrBase('testing', 'wt/x', { title: '[skip ci] x' })
  eq(kind(v), 'block')
  ok(text(v).includes('title') && text(v).includes('[skip ci]'), text(v))
})

check('pr-base: every skip spelling, any case, in the title, into each long-lived branch', () => {
  for (const base of ['main', 'rc', 'testing']) {
    for (const tag of ['[skip ci]', '[ci skip]', '[no ci]', '[skip actions]', '[actions skip]', '[SKIP CI]']) {
      eq([base, tag, kind(decidePrBase(base, 'rc', { title: `fix ${tag}` }))], [base, tag, 'block'])
    }
  }
})

check('pr-base: a skip-checks: true body into rc is blocked', () => {
  const v = decidePrBase('rc', 'hotfix/x', { title: 'x', body: 'Summary.\n\n\nskip-checks: true\n' })
  eq(kind(v), 'block')
  ok(text(v).includes('body') && text(v).includes('skip-checks: true'), text(v))
})

check('pr-base: a [skip ci] in the body is blocked too — the merge commit carries the body', () => {
  eq(kind(decidePrBase('testing', 'wt/x', { title: 'x', body: 'merge with [skip ci]' })), 'block')
})

check('pr-base: skip text into a branch this policy does not police passes', () => {
  eq(kind(decidePrBase('wt/stack-root', 'wt/x', { title: '[skip ci] x' })), 'ok')
})

check('pr-base: an ordinary title and body pass', () => {
  eq(kind(decidePrBase('testing', 'wt/x', { title: 'ci: skip nothing', body: 'skip-checks are not a thing here' })), 'ok')
  eq(kind(decidePrBase('rc', 'hotfix/x', { title: 'ci: skip nothing', body: 'skip-checks are not a thing here' })), 'ok')
})

check('pr-base: a lane retargeted from ckpt/a into rc is blocked, with the re-run command', () => {
  const v = decidePrBase('rc', 'hotfix/x', { action: 'edited', baseFrom: 'ckpt/a', headSha: S('f1') })
  eq(kind(v), 'block')
  ok(text(v).includes('retargeted from `ckpt/a`'), text(v))
  ok(text(v).includes("git commit --allow-empty -m 'ci: re-run against rc' && git push"), text(v))
})

check('pr-base: a feature retargeted into rc gets the retarget to testing, not the re-run', () => {
  const v = decidePrBase('rc', 'wt/x', { action: 'edited', baseFrom: 'testing', headSha: S('f1') })
  eq(kind(v), 'block')
  ok(text(v).includes('gh pr edit <n> --base testing'), text(v))
  ok(!text(v).includes('re-run against rc'), text(v))
})

check('pr-base: a retarget into main is blocked', () => {
  eq(kind(decidePrBase('main', 'rc', { action: 'edited', baseFrom: 'testing' })), 'block')
})

check('pr-base: a retarget into testing passes — its checks run light either way', () => {
  eq(kind(decidePrBase('testing', 'wt/x', { action: 'edited', baseFrom: 'rc' })), 'ok')
})

check('pr-base: an edit with no base change (a title edit) into rc passes', () => {
  eq(kind(decidePrBase('rc', 'hotfix/x', { action: 'edited', baseFrom: '' })), 'ok')
  // the body refresh promote-testing makes: re-checked against the rc it now names
  eq(kind(intoRc('promote/testing-0123456789ab', 'r2', 't2', { action: 'edited', baseFrom: '' })), 'ok')
  eq(kind(intoRc('promote/testing-0123456789ab', 'rcHot', 't2', { action: 'edited', baseFrom: '' })), 'block')
})

check('pr-base: the push after a retarget (synchronize) passes', () => {
  eq(kind(decidePrBase('rc', 'hotfix/x', { action: 'synchronize', baseFrom: '' })), 'ok')
})

check('pr-base: a two-argument call is judged on the branches alone', () => {
  eq(kind(decidePrBase('testing', 'wt/x')), 'ok')
  eq(kind(decidePrBase('rc', 'wt/x')), 'block')
  eq(kind(decidePrBase('rc', 'hotfix/x')), 'ok')
  eq(kind(decidePrBase('main', 'rc')), 'ok')
  eq(kind(decidePrBase('main', 'wt/x')), 'block')
})

check('pr-base: a feature into main is told to target testing', () => {
  ok(text(decidePrBase('main', 'wt/x')).includes('open this pull request against `testing`'), text(decidePrBase('main', 'wt/x')))
})

check('pr-base: a missing branch could not run', () => {
  eq(kind(decidePrBase('rc', undefined)), 'could-not-run')
  eq(kind(decidePrBase('', 'wt/x')), 'could-not-run')
})

check('pr-base: the environment — absent same-repo means true, anything but "true" is false', () => {
  eq(prOptsFromEnv({}).sameRepo, true)
  eq(prOptsFromEnv({ FLOW_HEAD_SAME_REPO: 'true' }).sameRepo, true)
  eq(prOptsFromEnv({ FLOW_HEAD_SAME_REPO: 'false' }).sameRepo, false)
  eq(prOptsFromEnv({ FLOW_HEAD_SAME_REPO: '' }).sameRepo, false)
  eq(prOptsFromEnv({ FLOW_PR_TITLE: 't', FLOW_ACTION: 'edited', FLOW_BASE_FROM: 'x' }, 'b', 'h'), {
    sameRepo: true,
    title: 't',
    body: '',
    action: 'edited',
    baseFrom: 'x',
    baseSha: 'b',
    headSha: 'h',
  })
})

// ── push ────────────────────────────────────────────────────────────────────

check('push testing: a fast-forward to rc commits passes', () => {
  eq(kind(push('testing', S('base'), S('r2'))), 'ok')
})

check('push testing: a fast-forward to main commits passes', () => {
  eq(kind(push('testing', S('base'), S('s1'))), 'ok')
})

check('push testing: a merge tip passes', () => {
  eq(kind(push('testing', S('base'), S('tM'))), 'ok')
})

check('push testing: a non-merge commit on neither rc nor main is blocked', () => {
  const v = push('testing', S('base'), S('x1'))
  eq(kind(v), 'block')
  ok(text(v).includes('testing takes pull requests'), text(v))
  ok(text(v).includes(S('x1').slice(0, 12)), `names the commit: ${text(v)}`)
})

check('push testing: without origin/main the fast-forward rule blocks with the fetch', () => {
  const v = push('testing', S('base'), S('r2'), { 'origin/rc': 'r2' })
  eq(kind(v), 'block')
  ok(text(v).includes('git fetch origin main rc testing'), text(v))
})

check('push testing: a delete is blocked', () => {
  const v = push('testing', S('base'), ZERO)
  eq(kind(v), 'block')
  ok(text(v).includes('refusing to delete `testing`'), text(v))
})

check('push testing: a create from zero is blocked', () => {
  eq(kind(push('testing', ZERO, S('r2'))), 'block')
})

check('push testing: a before-sha that is not an ancestor is blocked', () => {
  const v = push('testing', S('x1'), S('r2'))
  eq(kind(v), 'block')
  ok(text(v).includes('rewrites `testing`'), text(v))
})

check('push testing: a before-sha this repository does not have is blocked, with the fetch', () => {
  const v = push('testing', S('gone'), S('r2'))
  eq(kind(v), 'block')
  ok(text(v).includes('git fetch origin main rc testing'), text(v))
})

check('push testing: an after-sha that is not a commit could not run', () => {
  eq(kind(push('testing', S('base'), S('gone'))), 'could-not-run')
})

check('push rc: a merge tip passes', () => {
  eq(kind(push('rc', S('r2'), S('rcHot'))), 'ok')
})

check('push rc: a fast-forward of main passes', () => {
  eq(kind(push('rc', S('base'), S('s1'))), 'ok')
})

check('push rc: a non-merge commit not on main is blocked', () => {
  const v = push('rc', S('base'), S('x1'))
  eq(kind(v), 'block')
  ok(text(v).includes('rc takes pull requests'), text(v))
})

check('push rc: a delete is blocked', () => {
  eq(kind(push('rc', S('r2'), ZERO)), 'block')
})

check("push rc: a snapshot's merge carrying the snapshot's tree passes", () => {
  eq(kind(push('rc', S('r2'), S('rcSnap'))), 'ok')
})

check("push rc: a merge of testing content whose tree is not the snapshot's is blocked, naming the back-flow", () => {
  const v = push('rc', S('h1'), S('rcSnapX'))
  eq(kind(v), 'block')
  ok(text(v).includes("its tree is not that commit's"), text(v))
  ok(text(v).includes('`sync: rc → testing` from backflow/rc-*'), text(v))
})

check('push rc: a merge whose second parent is not on testing (a hotfix, a roll, a sync) is unchanged', () => {
  eq(kind(push('rc', S('r2'), S('rcHot'))), 'ok')
  // before the cutover's fast-forward, testing's stale tip carries none of rc's merges
  eq(kind(push('rc', S('base'), S('rcM'), { ...REFS, 'origin/testing': 'base' })), 'ok')
})

check('push rc: a merge without origin/testing blocks with the fetch', () => {
  const v = push('rc', S('r2'), S('rcSnap'), { 'origin/rc': 'r2', 'origin/main': 's1' })
  eq(kind(v), 'block')
  ok(text(v).includes('git fetch origin main rc testing'), text(v))
  // a fast-forward of main reads no testing
  eq(kind(push('rc', S('base'), S('s1'), { 'origin/rc': 'r2', 'origin/main': 's1' })), 'ok')
})

check('push main: a fast-forward to rc passes', () => {
  eq(kind(push('main', S('base'), S('r2'))), 'ok')
})

check("push main: the promotion merge, carrying rc's tree, passes", () => {
  eq(kind(push('main', S('base'), S('mainM'))), 'ok')
})

check('push main: a promotion merge with an edit smuggled in is blocked', () => {
  const v = push('main', S('base'), S('mainBad'))
  eq(kind(v), 'block')
  ok(text(v).includes("this merge's tree"), text(v))
})

check('push main: a commit not on rc is blocked', () => {
  const v = push('main', S('base'), S('x1'))
  eq(kind(v), 'block')
  ok(text(v).includes('main moves by promotion only'), text(v))
})

check('push main: without origin/rc it blocks with the fetch', () => {
  eq(kind(push('main', S('base'), S('r2'), { 'origin/main': 's1' })), 'block')
})

check('push: a working branch passes, whatever it carries', () => {
  eq(kind(push('wt/x', S('base'), S('x1'))), 'ok')
  eq(kind(push('wt/x', S('x1'), ZERO)), 'ok')
})

check('push: a tag passes', () => {
  eq(kind(push('refs/tags/v1.2.3', ZERO, S('x1'))), 'ok')
})

check('push: missing arguments could not run', () => {
  eq(kind(decidePush('refs/heads/main', undefined, undefined, fakeGit())), 'could-not-run')
  eq(kind(decidePush('', S('base'), S('r2'), fakeGit())), 'could-not-run')
})

// ── the CLI, as a process ───────────────────────────────────────────────────

/** The runtimes to spawn the guard under: this one (bun under `bun run
 *  check`), and node when it is on PATH — what flow.yml and the hook run. */
const runtimes = [process.execPath]
const node = spawnSync('node', ['-p', 'process.execPath'], { encoding: 'utf8' })
if (node.status === 0 && node.stdout.trim() && node.stdout.trim() !== process.execPath) runtimes.push(node.stdout.trim())

/** The parent's environment without any FLOW_* the caller happened to set. */
const cleanEnv = (extra = {}) => {
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith('FLOW_')))
  return { ...env, ...extra }
}

const run = (runtime, file, args, { cwd = ROOT, env = {} } = {}) =>
  spawnSync(runtime, [file, ...args], { cwd, env: cleanEnv(env), encoding: 'utf8' })

check('cli: pr-base reads the environment and maps the verdict to its exit code', () => {
  for (const rt of runtimes) {
    eq([rt, run(rt, GUARD, ['pr-base', 'testing', 'wt/x']).status], [rt, 0])
    const skip = run(rt, GUARD, ['pr-base', 'testing', 'wt/x'], { env: { FLOW_PR_TITLE: '[skip ci] x' } })
    eq([rt, skip.status], [rt, 2])
    ok(skip.stderr.startsWith('flow-guard: BLOCKED'), `${rt}: ${skip.stderr}`)
    eq([rt, run(rt, GUARD, ['pr-base', 'main', 'rc'], { env: { FLOW_HEAD_SAME_REPO: 'false' } }).status], [rt, 2])
    const feature = run(rt, GUARD, ['pr-base', 'rc', 'wt/x', S('base'), S('f1')])
    eq([rt, feature.status], [rt, 2])
    ok(feature.stderr.includes('gh pr edit <n> --base testing'), `${rt}: ${feature.stderr}`)
    const noted = run(rt, GUARD, ['pr-base', 'rc', 'hotfix/x'])
    eq([rt, noted.status], [rt, 0])
    ok(noted.stdout.startsWith('flow-guard: note — '), `${rt}: ${noted.stdout}`)
  }
})

check('cli: the rc lanes against a real repository — git merge-tree and the ancestry, not a fake', () => {
  const tmp = mkdtempSync(join(tmpdir(), 'flow-guard-lanes-'))
  try {
    const g = (...args) => {
      const r = spawnSync('git', ['-c', 'user.name=t', '-c', 'user.email=t@example.invalid', '-c', 'commit.gpgsign=false', '-c', 'core.hooksPath=/dev/null', ...args], {
        cwd: tmp,
        encoding: 'utf8',
        env: Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith('GIT_'))),
      })
      if (r.status !== 0) throw new Error(`git ${args.join(' ')}: ${r.stderr}`)
      return r.stdout.trim()
    }
    const commit = (file, body, msg) => {
      writeFileSync(join(tmp, file), body)
      g('add', file)
      g('commit', '-q', '-m', msg)
      return g('rev-parse', 'HEAD')
    }
    g('init', '-q', '-b', 'rc')
    const rc = commit('a.txt', 'a\n', 'base')
    g('update-ref', 'refs/remotes/origin/main', rc)
    g('switch', '-q', '-c', 'testing')
    const feature = commit('b.txt', 'b\n', 'feature')
    g('update-ref', 'refs/remotes/origin/testing', feature)
    g('switch', '-q', '-c', 'hot', rc)
    const hot = commit('c.txt', 'c\n', 'hotfix')
    g('switch', '-q', '-c', 'hotbad', feature)
    const hotBad = commit('d.txt', 'd\n', 'hotfix cut from testing')
    g('switch', '-q', 'rc')
    g('merge', '-q', '--no-ff', '--no-edit', 'hot')
    const rcAhead = g('rev-parse', 'HEAD')
    g('update-ref', 'refs/remotes/origin/rc', rc)
    for (const rt of runtimes) {
      const st = (args) => run(rt, GUARD, ['pr-base', 'rc', ...args], { cwd: tmp }).status
      eq([rt, 'snapshot', st(['promote/testing-x', rc, feature])], [rt, 'snapshot', 0])
      eq([rt, 'rc tip on testing', st(['promote/testing-x', rc, rc])], [rt, 'rc tip on testing', 0])
      eq([rt, 'rc ahead', st(['promote/testing-x', rcAhead, feature])], [rt, 'rc ahead', 2])
      eq([rt, 'not on testing', st(['promote/testing-x', rc, hot])], [rt, 'not on testing', 2])
      eq([rt, 'hotfix', st(['hotfix/x', rc, hot])], [rt, 'hotfix', 0])
      eq([rt, 'hotfix from testing', st(['hotfix/x', rc, hotBad])], [rt, 'hotfix from testing', 2])
      eq([rt, 'feature', st(['wt/x', rc, feature])], [rt, 'feature', 2])
    }
  } finally {
    rmSync(tmp, { recursive: true, force: true })
  }
})

check('cli: bad usage is exit 1, never a pass', () => {
  for (const rt of runtimes) {
    const r = run(rt, GUARD, ['bogus'])
    eq([rt, r.status], [rt, 1])
    ok(r.stderr.includes('could not run'), `${rt}: ${r.stderr}`)
    eq([rt, run(rt, GUARD, []).status], [rt, 1])
  }
})

check('cli: a run from a symlinked path still judges — a guard that sees an import exits 0 on everything', () => {
  const tmp = mkdtempSync(join(tmpdir(), 'flow-guard-test-'))
  try {
    const real = join(tmp, 'real')
    mkdirSync(real)
    copyFileSync(GUARD, join(real, 'flow-guard.mjs'))
    symlinkSync(real, join(tmp, 'linked-dir'), 'dir')
    symlinkSync(join(real, 'flow-guard.mjs'), join(tmp, 'linked-file.mjs'))
    const repo = join(tmp, 'repo')
    mkdirSync(repo)
    const init = spawnSync('git', ['init', '-q', repo], { encoding: 'utf8' })
    ok(init.status === 0, `git init: ${init.stderr}`)
    for (const rt of runtimes) {
      for (const file of [join(tmp, 'linked-dir', 'flow-guard.mjs'), join(tmp, 'linked-file.mjs')]) {
        const r = run(rt, file, ['push', 'refs/heads/main', S('base'), ZERO], { cwd: repo })
        eq([rt, file, r.status], [rt, file, 2])
        ok(r.stderr.includes('refusing to delete `main`'), `${rt} ${file}: ${r.stderr}`)
      }
    }
  } finally {
    rmSync(tmp, { recursive: true, force: true })
  }
})

if (failed) {
  console.error(`flow-guard: ${failed} of ${ran} failed`)
  process.exit(1)
}
console.log(`flow-guard: self-test ok (${ran} checks)`)
