#!/usr/bin/env node
// flow-guard — the branch model, as an executable policy.
//
// THE MODEL (docs/BRANCHES.md is the prose; this file is the enforcement).
// Three long-lived branches:
//
//   testing  the integration lane. Features and back-flows merge here, by
//            pull request, and it is where they are exercised together.
//   rc       the release candidate, and what the RC channel and the staging
//            deploy (rc-deploy.yml) build. It takes a snapshot of testing and
//            the hotfix/, release/ and sync/ lanes. Until pull requests move
//            to testing, rc still takes every pull request, and this file
//            judges no rc head.
//   main     the trunk. It takes only the promotion of rc — an rc→main merge,
//            once rc's tip has been deployed and verified. Stable tags land
//            on main.
//
// WHY THIS IS A SCRIPT AND NOT A FEW LINES OF SHELL IN THE WORKFLOW. Two
//   enforcers run the same policy, and they must not be able to disagree:
//
//     .github/workflows/flow.yml   the server-side tripwire: a red check on
//                                  every pull request and every push to a
//                                  long-lived branch.
//     scripts/hooks/pre-push       the local one: the push never leaves the
//                                  machine. `talaria setup` wires it.
//
//   Rules copied into both would drift, and the copy that drifts is always the
//   one nobody is running.
//
// THE CONTRACT (scripts/hooks/README.md — one contract, every gate here):
//   exit 0   pass, silent.
//   exit 2   block: the reason is on stderr, and it says what to do instead.
//   other    the guard could not run — bad usage, an unresolvable ref. NOT a
//            pass, whichever caller asked: CI fails the job, and git refuses a
//            push on any non-zero hook exit. It is deliberately not exit 2
//            because a block is a verdict and this is the absence of one; the
//            caller's default is still to stop.
//
// THE CHECKS, and what each can actually see.
//
//   Content provenance, not identity. A branch tip says nothing about who
//   pushed it, and no client-side guard can ask. What it can read is where the
//   commits came from — and that is what collapses three rules into one:
//
//     main — every commit this push introduces that is NOT a merge commit must
//            already be reachable from rc, and the merge commit itself must
//            carry rc's tree and nothing else. A promotion merge introduces no
//            content of its own; the commits it carries are rc's, and its tree
//            IS the tree it merged. So "a direct push to main", "a squash
//            merge", "a rebase merge" and a hand-made merge with an edit
//            smuggled into it all fail the same two tests, and the promotion
//            merge passes both.
//     rc   — the tip must be a merge commit (what a PR merge leaves behind), or
//            every introduced commit must already be reachable from main (a
//            fast-forward sync of the trunk — the one hand-made push rc
//            allows).
//     testing — the tip must be a merge commit (a PR merge, a back-flow's
//            included), or every introduced commit must already be reachable
//            from rc or main (a fast-forward to content those branches already
//            verified — how the lane is brought up to rc's tip).
//
//   No long-lived branch may be rewritten or deleted by a push: a force-push
//   past this guard erases commits the promotion record refers to.
//
//   A pull request is judged by what its merge will do to a long-lived branch:
//
//     into main, rc or testing — its title and body may not carry what makes
//            GitHub skip the push's workflows ([skip ci] and kin, or a
//            `skip-checks: true` trailer). The merge commit carries both (the
//            repository writes the title and body into it), so the branch push
//            would land with no CI and no guard run on it.
//     into rc or main — a pull request RETARGETED there is refused until a
//            push re-runs its checks: they ran in the mode the old base chose
//            (light, for anything but rc or main), and CI does not re-run on a
//            base change, so the required checks would be green for the wrong
//            question.
//     into main — the head must be this repository's `rc`; a fork's branch
//            named rc is not it. flow.yml ALSO requires that the head commit
//            has green CI and rc-deploy runs made on rc; this script checks
//            the source, the workflow checks the evidence (it is the only one
//            that can read the Actions API).
//
// WHAT IT DELIBERATELY CANNOT SEE, and who covers it. That a merge into rc or
//   testing came from a reviewed PR rather than a local `git merge`: that is
//   branch protection's job ("require a pull request before merging" — the
//   required settings are in docs/BRANCHES.md). What is left for a client-side
//   check is the content shapes only a bypass produces, and those are the rules
//   above. They fail closed on an unknown: an unresolvable base, a missing
//   origin ref, a shallow clone — each blocks with the command to run, rather
//   than passing because it could not tell.
//
// USAGE
//   node scripts/flow-guard.mjs pr-base <base-branch> <head-branch> [<base-sha> <head-sha>]
//   node scripts/flow-guard.mjs push <ref> <before-sha> <after-sha>
//
//   pr-base also reads the pull request from the environment, all optional
//   (flow.yml sets each from the event; a caller passing only the two branches
//   is judged on the branches alone):
//     FLOW_HEAD_SAME_REPO  'true' | 'false': the head branch lives in this
//                          repository. Absent means true; anything else but
//                          'true' is false.
//     FLOW_PR_TITLE        the title.
//     FLOW_PR_BODY         the body.
//     FLOW_ACTION          the pull_request event's action (opened, edited, …).
//     FLOW_BASE_FROM       on `edited`, the base it was retargeted FROM.
//
//   The `push` form reads the remote-tracking refs (origin/main, origin/rc),
//   so fetch them first. flow.yml fetches explicitly; the hook uses what the
//   clone has, and says so when that is not enough.
//
// AS A MODULE. decidePrBase and decidePush are the rules, exported for
//   scripts/flow-guard.test.mjs. They touch no process state and return a
//   verdict — {ok: true}, {ok: false, lines} or {couldNotRun: why} — and the
//   push rules read git through an adapter, so a test can hand them a literal
//   graph. The CLI at the bottom maps a verdict onto the contract above, and
//   runs only when this file is invoked directly, which it decides by comparing
//   REAL paths: flow.yml runs a copy materialized in $RUNNER_TEMP, and a hooks
//   path or a checkout can be a symlink. A guard that decided "imported, not
//   run" there would exit 0 having checked nothing.
//
// IT STANDS ALONE. flow.yml materializes this one file from a revision the
//   change cannot edit and runs it from $RUNNER_TEMP, so it imports nothing but
//   node's own modules.

import { execFileSync } from 'node:child_process'
import { realpathSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

/** The branches this file polices. Anything else — feature branches, wt/*,
 *  agent/*, ci/* — is the pusher's own business, and the guard stays out of it. */
const TRUNK = 'main'
const INTEGRATION = 'rc'
const LANE = 'testing'
const LONG_LIVED = [TRUNK, INTEGRATION, LANE]

/** The remote-tracking refs, one per long-lived branch. */
const TRUNK_REF = 'origin/main'
const INTEGRATION_REF = 'origin/rc'
const LANE_REF = 'origin/testing'

/** The one fetch every hint names: the long-lived branches, all of them. */
const FETCH = `git fetch origin ${LONG_LIVED.join(' ')}`

const ZERO = /^0{40}$/
const SHA = /^[0-9a-f]{40}$/

/** What makes GitHub skip a push's workflows, read from the head commit's
 *  message. A pull request's merge commit carries its title and body. */
const SKIP_CI = /\[(skip ci|ci skip|no ci|skip actions|actions skip)\]/i
const SKIP_CHECKS = /^skip-checks:\s*true/im

const USAGE = [
  'usage:',
  '  node scripts/flow-guard.mjs pr-base <base-branch> <head-branch> [<base-sha> <head-sha>]',
  '  node scripts/flow-guard.mjs push <ref> <before-sha> <after-sha>',
].join('\n')

// ── verdicts ────────────────────────────────────────────────────────────────

/** Pass. Frozen: a rule that returns it shares one object. */
const PASS = Object.freeze({ ok: true })

/** Block. The message is the whole product: what is wrong, why it matters,
 *  and the command or route that does it correctly. */
const blocked = (lines) => ({ ok: false, lines })

/** The guard could not run. Never a pass, never a block: the caller decides
 *  what an unrunnable gate means (CI fails the job; the hook warns). */
const cannot = (why) => ({ couldNotRun: why })

// ── git ─────────────────────────────────────────────────────────────────────

function git(...args) {
  return execFileSync('git', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim()
}

function gitOk(...args) {
  try {
    git(...args)
    return true
  } catch {
    return false
  }
}

/** Everything the push rules ask of git. A test passes its own over a literal
 *  graph; this one asks the repository the process is running in. */
export const gitAdapter = {
  isCommit: (sha) => SHA.test(sha) && gitOk('cat-file', '-e', `${sha}^{commit}`),
  isAncestor: (sha, ref) => gitOk('merge-base', '--is-ancestor', sha, ref),
  isMerge: (sha) => git('rev-list', '--parents', '-n', '1', sha).split(' ').length > 2,
  /** The commits `after` has that `before` does not, newest first. */
  revList: (after, before) => git('rev-list', after, '--not', before).split('\n').filter(Boolean),
  subject: (sha) => git('log', '-1', '--format=%s', sha),
  hasRef: (ref) => gitOk('rev-parse', '--verify', `${ref}^{commit}`),
  /** A revision's tree; `<sha>^2` names a merge's second parent. */
  tree: (rev) => git('rev-parse', `${rev}^{tree}`),
}

/** A ref the rules need must exist LOCALLY. A clone that never fetched rc, or
 *  a single-branch checkout, cannot be told from one where nothing is wrong —
 *  so it blocks with the fetch to run, rather than passing on ignorance. */
function requireRef(g, ref) {
  if (g.hasRef(ref)) return PASS
  return blocked([
    `origin ref \`${ref}\` is not in this repository, so the provenance rule cannot be checked.`,
    '',
    'Fetch it and retry:',
    `  ${FETCH}`,
  ])
}

/** The first five offending commits, one line each, and a count of the rest. */
function listed(g, shas) {
  const named = shas.slice(0, 5).map((sha) => `  ${sha.slice(0, 12)}  ${g.subject(sha)}`)
  return shas.length > 5 ? [...named, `  … and ${shas.length - 5} more`] : named
}

// ── pr-base ─────────────────────────────────────────────────────────────────

/**
 * Whether a pull request may merge as it stands.
 *
 * opts (all optional — a caller with only the two branches is judged on them):
 *   sameRepo  the head branch lives in this repository (default true)
 *   title     the pull request's title
 *   body      its body
 *   action    the pull_request event's action
 *   baseFrom  on `edited`, the base it was retargeted from
 *   baseSha   the base commit the event names
 *   headSha   the head commit the event names
 */
export function decidePrBase(base, head, opts = {}) {
  if (!base || !head) return cannot(`${USAGE}\n  pr-base needs a base and a head branch`)
  const { sameRepo = true, title = '', body = '', action = '', baseFrom = '', headSha = '' } = opts

  // (a) Nothing may merge into a long-lived branch carrying the text that
  // skips the branch push's own checks. The merge commit's message is the
  // title and the body (the repository's merge settings), so both are read,
  // as one message.
  if (LONG_LIVED.includes(base)) {
    const skips = (text) => SKIP_CI.exec(text)?.[0] ?? SKIP_CHECKS.exec(text)?.[0].trim()
    const where = skips(title ?? '') ? 'title' : 'body'
    const hit = skips(`${title ?? ''}\n\n${body ?? ''}`)
    if (hit) {
      return blocked([
        `this pull request's ${where} carries \`${hit}\`, and merging it into \`${base}\` would skip the checks.`,
        '',
        'The merge commit carries the pull request\'s title and body, and GitHub reads',
        'that text to skip a push\'s workflows. So this would skip the post-merge push',
        `checks (branch push, CI) on a long-lived branch: \`${base}\` would move with`,
        'nothing having checked the commit it moved to.',
        '',
        'Edit the title or body to remove it; the check re-runs on the edit.',
      ])
    }
  }

  // (b) A retarget into rc or main. The checks on this head ran against the
  // OLD base, in the mode that base chose — light, for anything but rc or
  // main — and CI does not re-run on a base change, so the same-named required
  // checks would stand green for a question nobody asked. A push re-runs them
  // against the new base, and this rule only reads `edited` events, so the
  // push's own `synchronize` passes it.
  if ((base === INTEGRATION || base === TRUNK) && action === 'edited' && baseFrom) {
    const which = headSha ? `\`${headSha.slice(0, 12)}\`` : 'this head'
    return blocked([
      `retargeted from \`${baseFrom}\`: ${which}'s checks ran against the old base, in its mode`,
      '(light, for anything but rc or main).',
      '',
      `\`${base}\` requires its checks in full, and CI does not re-run on a base change.`,
      `Push a commit so CI runs against \`${base}\`:`,
      `  git commit --allow-empty -m 'ci: re-run against ${base}' && git push`,
    ])
  }

  // (c) main takes this repository's rc, and nothing else.
  if (base === TRUNK && (head !== INTEGRATION || !sameRepo)) {
    return blocked([
      head === INTEGRATION
        ? '`rc` from another repository cannot be merged into `main`: only this repository\'s rc can.'
        : `\`${head}\` cannot be merged into \`main\`, and nothing but \`rc\` can.`,
      '',
      'main is the trunk: it moves only by a promotion merge of rc, and only once',
      'rc is deployed and verified in staging. So:',
      '',
      '  1. open this pull request against `rc` — that is where review, CI and',
      '     the staging deploy all happen;',
      '  2. the promotion follows: `.github/workflows/promote.yml` verifies rc\'s tip',
      '     once it is green and deployed and offers the rc → main pull request, or',
      '     `gh pr create --base main --head rc` opens it by hand.',
    ])
  }

  // (d) testing, rc, and any other base: pass.
  return PASS
}

// ── push ────────────────────────────────────────────────────────────────────

/** Shared by every long-lived branch: no push may rewrite or delete one, and
 *  the guard needs a base commit it can actually read. */
function checkNotRewritten(g, name, before, after) {
  if (ZERO.test(after)) {
    return blocked([
      `refusing to delete \`${name}\` by push.`,
      '',
      'Deleting a long-lived branch is a repository-settings decision, never a',
      'side effect of a push. If it is genuinely intended, remove the branch',
      `protection rule first (docs/BRANCHES.md → The required settings), then`,
      'delete it in the repo settings.',
    ])
  }
  if (!g.isCommit(after)) return cannot(`cannot resolve the pushed commit ${after}`)

  if (ZERO.test(before) || !before) {
    return blocked([
      `this push would create \`${name}\` from nothing (no base commit).`,
      '',
      'The long-lived branches already exist, and a push that has no base is a',
      'rewrite — the guard cannot tell it from one, so it refuses.',
      `Check where the base went: \`git reflog\` and \`git log origin/${name}\`.`,
    ])
  }
  if (!g.isCommit(before)) {
    return blocked([
      `this push has base \`${before}\`, which is not in this repository — so the`,
      'guard cannot check what the push introduces.',
      '',
      'That is the shape of a force-push onto a branch this clone has not seen.',
      `Fetch first (\`${FETCH}\`) and retry; if the base is`,
      'genuinely gone, the rewrite it implies is the thing to discuss.',
    ])
  }
  if (!g.isAncestor(before, after)) {
    return blocked([
      `this push rewrites \`${name}\`'s history: \`${before.slice(0, 12)}\` is not an ancestor`,
      `of \`${after.slice(0, 12)}\`.`,
      '',
      'No long-lived branch is ever force-pushed: the commits a promotion or an',
      'RC deploy record refer to have to stay reachable. Rebase your work on the',
      'branch tip and push that.',
    ])
  }
  return PASS
}

/** main: every introduced non-merge commit must already be on rc, and a merge
 *  must carry rc's tree. */
function checkTrunkPush(g, after, introduced) {
  const ref = requireRef(g, INTEGRATION_REF)
  if (!ref.ok) return ref

  // A merge commit's own content is never visible to the reachability test
  // below (a merge is exempt: it introduces no commits of its own). So the
  // merge gets the other half of the same claim — main takes rc WHOLE — as a
  // tree comparison: the merge's tree must be the tree of the branch it merged
  // (its second parent). `git merge --no-ff rc`, an edit, then a commit is a
  // merge whose tree is neither branch's, and it would otherwise sail through
  // a check named for exactly this.
  //
  // It holds for a real promotion by construction: main's content is always the
  // last promoted rc (that is the only thing that moves main), so merging the
  // current rc yields rc's tree exactly. And it is skipped when the pushed
  // commit is already rc's (`!isAncestor`) — the `git push origin rc:main`
  // fast-forward the model allows. That shape needs no tree test: if the commit
  // is on rc, its content is rc's by definition. Without this, the rule misfires
  // on it, because rc's tip is usually a PR MERGE and its second parent is then
  // the feature branch, not rc.
  if (g.isMerge(after) && !g.isAncestor(after, INTEGRATION_REF)) {
    if (g.tree(after) !== g.tree(`${after}^2`)) {
      return blocked([
        `this merge's tree is not the tree of \`${after.slice(0, 12)}^2\` — the branch it merged.`,
        '',
        'main takes rc whole: the promotion merge carries rc\'s content and adds',
        'nothing of its own, which is what makes "the promoted commit" and "the',
        'commit the staging deploy verified" the same content. A merge whose tree',
        'differs from its second parent is a merge with something smuggled into',
        'it — a conflict hand-resolved, an edit committed on top, a forged',
        'commit-tree — and none of those are reviewed, tested or deployed.',
        '',
        'If rc genuinely needs to differ, land the difference on rc first (a pull',
        'request, the staging deploy, then the promotion).',
      ])
    }
  }

  const foreign = introduced.filter((sha) => !g.isMerge(sha) && !g.isAncestor(sha, INTEGRATION_REF))
  if (foreign.length === 0) return PASS

  return blocked([
    'main moves by promotion only, and this push carries content that is not on rc:',
    '',
    ...listed(g, foreign),
    '',
    'main is the release trunk: it takes rc whole, after rc has been deployed and',
    'verified in staging. A commit that reaches main without passing through rc',
    'skipped review, CI and the staging deploy at once — which is why a direct',
    'push, a squash merge and a rebase merge are all this same refusal.',
    '',
    'Instead:',
    '  - land the change on rc first (pull requests target rc); or',
    '  - let the promotion happen: .github/workflows/promote.yml offers the rc →',
    '    main pull request once rc\'s tip is green and deployed, and only a MERGE',
    '    COMMIT can land it. A squash or rebase promotion fails this guard.',
  ])
}

/** rc: a PR merge (a merge commit), or a fast-forward sync from main. */
function checkIntegrationPush(g, after, introduced) {
  if (g.isMerge(after)) return PASS

  const ref = requireRef(g, TRUNK_REF)
  if (!ref.ok) return ref

  const foreign = introduced.filter((sha) => !g.isAncestor(sha, TRUNK_REF))
  if (foreign.length === 0) return PASS

  return blocked([
    'rc takes pull requests, and this push is neither a PR merge nor a sync of main:',
    '',
    `  the tip \`${after.slice(0, 12)}\` is not a merge commit, and these commits are not on main:`,
    '',
    ...listed(g, foreign),
    '',
    'rc is the integration branch: review, CI and the staging deploy all happen',
    'there, per pull request. A commit pushed straight to it skipped all three,',
    'and the promotion that follows would carry it to main unverified.',
    '',
    'Instead:',
    '  - push the branch and open a pull request against rc; or',
    '  - if this is meant to sync trunk content into rc, push it as a',
    '    fast-forward of main (every commit already on main passes this guard).',
    '',
    'The rules behind this: docs/BRANCHES.md.',
  ])
}

/** testing: a PR merge (a merge commit), or a fast-forward to content rc or
 *  main already carries. */
function checkLanePush(g, after, introduced) {
  if (g.isMerge(after)) return PASS

  for (const ref of [INTEGRATION_REF, TRUNK_REF]) {
    const has = requireRef(g, ref)
    if (!has.ok) return has
  }

  const foreign = introduced.filter((sha) => !g.isAncestor(sha, INTEGRATION_REF) && !g.isAncestor(sha, TRUNK_REF))
  if (foreign.length === 0) return PASS

  return blocked([
    `the tip \`${after.slice(0, 12)}\` is not a merge commit, and these commits are on neither rc nor main:`,
    '',
    ...listed(g, foreign),
    '',
    'testing takes pull requests: this push is neither a PR merge nor a fast-forward',
    'of rc or main. Push your branch and open a pull request into testing.',
    'Rules: docs/BRANCHES.md.',
  ])
}

/** Whether a push to `ref` may move it from `before` to `after`. `g` is the
 *  git adapter (the repository this process runs in, unless a test says
 *  otherwise). A git call that fails outright throws; the CLI reads that as
 *  "could not run". */
export function decidePush(ref, before, after, g = gitAdapter) {
  if (!ref || before === undefined || after === undefined) {
    return cannot(`${USAGE}\n  push needs a ref, a before-sha and an after-sha`)
  }
  // Tags and anything that is not a branch head: release.yml owns tag grammar,
  // and a tag is not a branch that content can leak past.
  if (!ref.startsWith('refs/heads/')) return PASS
  const name = ref.slice('refs/heads/'.length)

  if (name !== TRUNK && name !== INTEGRATION && name !== LANE) return PASS

  const kept = checkNotRewritten(g, name, before, after)
  if (!kept.ok) return kept

  const introduced = g.revList(after, before)

  if (name === TRUNK) return checkTrunkPush(g, after, introduced)
  if (name === INTEGRATION) return checkIntegrationPush(g, after, introduced)
  return checkLanePush(g, after, introduced)
}

// ── entry ───────────────────────────────────────────────────────────────────

/** The pull request as flow.yml hands it over: the two shas as arguments, the
 *  rest in the environment. */
export function prOptsFromEnv(env, baseSha = '', headSha = '') {
  const same = env.FLOW_HEAD_SAME_REPO
  return {
    sameRepo: same === undefined ? true : same === 'true',
    title: env.FLOW_PR_TITLE ?? '',
    body: env.FLOW_PR_BODY ?? '',
    action: env.FLOW_ACTION ?? '',
    baseFrom: env.FLOW_BASE_FROM ?? '',
    baseSha,
    headSha,
  }
}

/** A verdict onto the contract: 0 pass, 2 block, 1 could not run. A verdict of
 *  any other shape is not a pass either. */
function exitWith(verdict) {
  if (verdict?.ok === true) process.exit(0)
  if (verdict?.ok === false && Array.isArray(verdict.lines)) {
    console.error(`flow-guard: BLOCKED\n\n${verdict.lines.filter(Boolean).join('\n')}\n`)
    process.exit(2)
  }
  console.error(`flow-guard could not run — not a pass: ${verdict?.couldNotRun ?? 'no verdict'}`)
  process.exit(1)
}

/** Run as a program, or imported by the test? Real paths on both sides, so a
 *  symlinked checkout, hooks path or temp dir still reads as a direct run. */
function invokedDirectly() {
  const file = process.argv[1]
  if (!file) return false
  try {
    return realpathSync(fileURLToPath(import.meta.url)) === realpathSync(file)
  } catch {
    return pathToFileURL(resolve(file)).href === import.meta.url
  }
}

if (invokedDirectly()) {
  const [mode, ...args] = process.argv.slice(2)
  let verdict
  try {
    switch (mode) {
      case 'pr-base':
        verdict = decidePrBase(args[0], args[1], prOptsFromEnv(process.env, args[2], args[3]))
        break
      case 'push':
        verdict = decidePush(args[0], args[1], args[2])
        break
      default:
        verdict = cannot(`${mode ? `unknown check \`${mode}\`\n` : ''}${USAGE}`)
    }
  } catch (err) {
    verdict = cannot(err instanceof Error ? err.message : String(err))
  }
  exitWith(verdict)
}
