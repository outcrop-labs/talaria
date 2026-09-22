#!/usr/bin/env node
// flow-guard — the branch model, as an executable policy.
//
// THE MODEL (docs/BRANCHES.md is the prose; this file is the enforcement).
//
//   rc       the integration branch. Every pull request targets it, and it is
//            what the RC channel and the staging deploy (rc-deploy.yml) build.
//   main     the release trunk. It moves one way only — an rc→main merge, and
//            only once rc's tip has been deployed and verified. Stable tags
//            land on main.
//   testing  retired. The nightly channel is built from rc now (release.yml).
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
//     testing — retired; a push here is refused with the reason.
//
//   Neither long-lived branch may be rewritten or deleted by a push: a
//   force-push past this guard erases commits the promotion record refers to.
//
//   A pull request to main may only come from rc. flow.yml ALSO requires that
//   the pull request's head commit has a green rc-deploy run; this script
//   checks the source, the workflow checks the deploy (it is the only one that
//   can read the Actions API).
//
// WHAT IT DELIBERATELY CANNOT SEE, and who covers it. That a merge into rc
//   came from a reviewed PR rather than a local `git merge`: that is branch
//   protection's job ("require a pull request before merging" on rc — the
//   required settings are in docs/BRANCHES.md). What is left for a client-side
//   check is the content shapes only a bypass produces, and those are the rules
//   above. They fail closed on an unknown: an unresolvable base, a missing
//   origin ref, a shallow clone — each blocks with the command to run, rather
//   than passing because it could not tell.
//
// USAGE
//   node scripts/flow-guard.mjs pr-base <base-branch> <head-branch>
//   node scripts/flow-guard.mjs push <ref> <before-sha> <after-sha>
//
//   The `push` form reads the remote-tracking refs (origin/main, origin/rc),
//   so fetch them first. flow.yml fetches explicitly; the hook uses what the
//   clone has, and says so when that is not enough.

import { execFileSync } from 'node:child_process'

/** The branches this file polices. Anything else — feature branches, wt/*,
 *  agent/* — is the pusher's own business, and the guard stays out of it. */
const TRUNK = 'main'
const INTEGRATION = 'rc'
const RETIRED = 'testing'

/** The remote-tracking refs the provenance rules read. */
const TRUNK_REF = 'origin/main'
const INTEGRATION_REF = 'origin/rc'

const ZERO = /^0{40}$/
const SHA = /^[0-9a-f]{40}$/

const USAGE = [
  'usage:',
  '  node scripts/flow-guard.mjs pr-base <base-branch> <head-branch>',
  '  node scripts/flow-guard.mjs push <ref> <before-sha> <after-sha>',
].join('\n')

/** The guard could not run. Never a pass, never a block: the caller decides
 *  what an unrunnable gate means (CI fails the job; the hook warns). */
const couldNotRun = (why) => {
  console.error(`flow-guard could not run — not a pass: ${why}`)
  process.exit(1)
}

/** Block. The message is the whole product: what is wrong, why it matters,
 *  and the command or route that does it correctly. */
const block = (lines) => {
  console.error(`flow-guard: BLOCKED\n\n${lines.filter(Boolean).join('\n')}\n`)
  process.exit(2)
}

const pass = () => process.exit(0)

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

const isCommit = (sha) => SHA.test(sha) && gitOk('cat-file', '-e', `${sha}^{commit}`)
const isAncestor = (sha, ref) => gitOk('merge-base', '--is-ancestor', sha, ref)
const shaOf = (ref) => git('rev-parse', '--verify', `${ref}^{commit}`)
const subject = (sha) => git('log', '-1', '--format=%s', sha)
const isMerge = (sha) => git('rev-list', '--parents', '-n', '1', sha).split(' ').length > 2

/** A ref the rules need must exist LOCALLY. A clone that never fetched rc, or
 *  a single-branch checkout, cannot be told from one where nothing is wrong —
 *  so it blocks with the fetch to run, rather than passing on ignorance. */
function requireRef(ref) {
  if (gitOk('rev-parse', '--verify', `${ref}^{commit}`)) return
  block([
    `origin ref \`${ref}\` is not in this repository, so the provenance rule cannot be checked.`,
    '',
    'Fetch it and retry:',
    `  git fetch origin ${TRUNK} ${INTEGRATION}`,
  ])
}

// ── pr-base ─────────────────────────────────────────────────────────────────

/** Which branch a pull request may come FROM, keyed by the branch it targets
 *  (`main` accepts one source; `rc` is the normal path; a retired branch takes
 *  none). Any other base is not this policy's business. */
function checkPrBase(base, head) {
  if (!base || !head) couldNotRun(`${USAGE}\n  pr-base needs a base and a head branch`)

  if (base === RETIRED) {
    block([
      '`testing` is retired: the nightly channel is published from `rc` now.',
      '',
      'Target the pull request at `rc` instead — it is the integration branch,',
      'and it is what both the RC channel and the staging deploy build.',
    ])
  }

  if (base === TRUNK && head !== INTEGRATION) {
    block([
      `\`${head}\` cannot be merged into \`main\`, and nothing but \`rc\` can.`,
      '',
      'main is the release trunk: it moves only by a promotion merge of rc, and',
      'only once rc is deployed and verified in staging. So:',
      '',
      '  1. open this pull request against `rc` — that is where review, CI and',
      '     the staging deploy all happen;',
      '  2. the promotion happens on its own: `.github/workflows/promote.yml` verifies',
      '     rc\'s tip once it is green and deployed, and offers the rc → main pull',
      '     request. Nobody opens one by hand.',
    ])
  }

  pass()
}

// ── push ────────────────────────────────────────────────────────────────────

/** Shared by both long-lived branches: no push may rewrite or delete one, and
 *  the guard needs a base commit it can actually read. */
function checkNotRewritten(name, before, after) {
  if (ZERO.test(after)) {
    block([
      `refusing to delete \`${name}\` by push.`,
      '',
      'Deleting a long-lived branch is a repository-settings decision, never a',
      'side effect of a push. If it is genuinely intended, remove the branch',
      `protection rule first (docs/BRANCHES.md → The required settings), then`,
      'delete it in the repo settings.',
    ])
  }
  if (!isCommit(after)) couldNotRun(`cannot resolve the pushed commit ${after}`)

  if (ZERO.test(before) || !before) {
    block([
      `this push would create \`${name}\` from nothing (no base commit).`,
      '',
      'Both long-lived branches already exist, and a push that has no base is a',
      'rewrite — the guard cannot tell it from one, so it refuses.',
      `Check where the base went: \`git reflog\` and \`git log origin/${name}\`.`,
    ])
  }
  if (!isCommit(before)) {
    block([
      `this push has base \`${before}\`, which is not in this repository — so the`,
      'guard cannot check what the push introduces.',
      '',
      'That is the shape of a force-push onto a branch this clone has not seen.',
      `Fetch first (\`git fetch origin ${TRUNK} ${INTEGRATION}\`) and retry; if the base is`,
      'genuinely gone, the rewrite it implies is the thing to discuss.',
    ])
  }
  if (!isAncestor(before, after)) {
    block([
      `this push rewrites \`${name}\`'s history: \`${before.slice(0, 12)}\` is not an ancestor`,
      `of \`${after.slice(0, 12)}\`.`,
      '',
      'Neither long-lived branch is ever force-pushed: the commits a promotion or',
      'an RC deploy record refer to have to stay reachable. Rebase your work on',
      'the branch tip and push that.',
    ])
  }
}

/** main: every introduced non-merge commit must already be on rc, and a merge
 *  must carry rc's tree. */
function checkTrunkPush(before, after, introduced) {
  requireRef(INTEGRATION_REF)

  // A merge commit's own content is never visible to the reachability test
  // above (a merge is exempt: it introduces no commits of its own). So the
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
  if (isMerge(after) && !isAncestor(after, INTEGRATION_REF)) {
    const tree = git('rev-parse', `${after}^{tree}`)
    const merged = git('rev-parse', `${after}^2^{tree}`)
    if (tree !== merged) {
      block([
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

  const foreign = introduced.filter((sha) => !isMerge(sha) && !isAncestor(sha, INTEGRATION_REF))
  if (foreign.length === 0) pass()

  const named = foreign.slice(0, 5).map((sha) => `  ${sha.slice(0, 12)}  ${subject(sha)}`)
  const more = foreign.length > 5 ? [`  … and ${foreign.length - 5} more`] : []

  block([
    'main moves by promotion only, and this push carries content that is not on rc:',
    '',
    ...named,
    ...more,
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
function checkIntegrationPush(before, after, introduced) {
  if (isMerge(after)) pass()

  requireRef(TRUNK_REF)

  const foreign = introduced.filter((sha) => !isAncestor(sha, TRUNK_REF))
  if (foreign.length === 0) pass()

  const named = foreign.slice(0, 5).map((sha) => `  ${sha.slice(0, 12)}  ${subject(sha)}`)
  const more = foreign.length > 5 ? [`  … and ${foreign.length - 5} more`] : []

  block([
    'rc takes pull requests, and this push is neither a PR merge nor a sync of main:',
    '',
    `  the tip \`${after.slice(0, 12)}\` is not a merge commit, and these commits are not on main:`,
    '',
    ...named,
    ...more,
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

function checkPush(ref, before, after) {
  if (!ref || before === undefined || after === undefined) {
    couldNotRun(`${USAGE}\n  push needs a ref, a before-sha and an after-sha`)
  }
  // Tags and anything that is not a branch head: release.yml owns tag grammar,
  // and a tag is not a branch that content can leak past.
  if (!ref.startsWith('refs/heads/')) pass()
  const name = ref.slice('refs/heads/'.length)

  if (name === RETIRED) {
    // Deleting it IS the retirement — the documented command has to work from a
    // wired clone, or the branch outlives the decision to retire it. Anything
    // else aimed at it is refused.
    if (ZERO.test(after)) pass()
    block([
      '`testing` is retired: the nightly channel is published from `rc` now.',
      '',
      'Nothing needs to move it — release.yml builds `nightly` from rc\'s tip on',
      'its 03:17 UTC schedule. Delete it when you are ready:',
      '  git push origin :testing',
    ])
  }

  if (name !== TRUNK && name !== INTEGRATION) pass()

  checkNotRewritten(name, before, after)

  const introduced = git('rev-list', after, '--not', before)
    .split('\n')
    .filter(Boolean)

  if (name === TRUNK) checkTrunkPush(before, after, introduced)
  else checkIntegrationPush(before, after, introduced)
}

// ── entry ───────────────────────────────────────────────────────────────────

const [mode, ...args] = process.argv.slice(2)

switch (mode) {
  case 'pr-base':
    checkPrBase(args[0], args[1])
    break
  case 'push':
    checkPush(args[0], args[1], args[2])
    break
  default:
    couldNotRun(`${mode ? `unknown check \`${mode}\`\n` : ''}${USAGE}`)
}