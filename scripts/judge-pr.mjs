#!/usr/bin/env node
// judge-pr — measure a change against this repo's standards, from the diff.
//
// WHY IT EXISTS. The gates answer "does it pass". They cannot answer "is this
// the change this repository asked for": that the pull request is aimed at
// `main` (or a feature at `rc`), that a hotfix adds a migration, that a
// promoted changelog entry is rewritten, that a user-visible change carries no
// entry, that a do-not-touch tree moved. Those are decidable from git alone,
// and that is this file. The half that is NOT decidable — "does this actually
// work in Talaria" — is a reading: scripts/judge-review.mjs (`judge (review)`)
// and the skill beside it (`.claude/skills/judge-pr/SKILL.md`).
//
// WHAT IT IS NOT. It does not run the gates (that is `bun run verify`, and CI),
// and it does not replace a reviewer. Its findings are graded:
//
//   must    the repo states this about itself — branch flow (targets-main,
//           feature-targets-rc, hotfix-adds-migration, changelog-entry-rewritten),
//           do-not-touch trees (off-limits), a user-visible change with no
//           changelog entry (no-changelog), an entry appended into CHANGELOG.md
//           (changelog-not-a-file).
//   should  a convention with a reason behind it: an entry with no bold lead
//           (changelog-no-lead) or no word on verification
//           (changelog-no-verification), a skill off its documented shape
//           (skill-shape).
//   ask     a question for the human. NEVER a reason to hold a change: an
//           empty or unfilled pull-request body, and a change to the reviewer
//           with no calibration scorecard (reviewer-changed-no-scorecard).
//
// WHAT LEFT, AND WHY. The track record (#669–#725: 59 findings, none a defect)
// cut commit-subject, claim-misses-surface, cli-suite-not-covered-by-verify,
// no-test, product-skills and generated-drift/changed. `bun run check` already
// fails generated drift; the rest were noise. The reading moved to
// scripts/judge-review.mjs.
//
// TRUST. In CI this runs from the trusted branch's tree (judge.yml), so a pull
// request cannot loosen its own musts. It reads the pull request as git objects
// only — never its working tree.
//
// The exit code speaks the contract in scripts/hooks/README.md: 0 when nothing
// is a `must`, 2 when something is, 1 when the judge could not run at all.
//
// THE LANES. Some pull requests carry changes that were each judged in their
// own pull request already, and judging them again only re-finds the
// promotion's own shape: the `rc` → `main` promotion, a `promote/*` snapshot
// of `testing` into `rc`, the `backflow/*` → `testing` back-flow and a
// `sync/*` merge into `rc` get a report with no findings and exit 0. A
// `release/*` head into `rc` is the changelog roll, which writes CHANGELOG.md
// by design, so the CHANGELOG.md checks skip it. A `hotfix/*` head into `rc`
// may not add a migration; a pull request into `testing` may not rewrite an
// entry `rc` already has. Anything else into `rc` is a feature aimed at the
// wrong branch: feature pull requests target `testing`.
//
// USAGE
//   node scripts/judge-pr.mjs [--base <ref>] [--head <ref>] [--target <branch>]
//                             [--source <branch>] [--pr-body <file>]
//                             [--summary] [--json]
//
//   --base defaults to origin/<GITHUB_BASE_REF> (CI: the pull request's base),
//   else origin/testing when this clone has it (where a feature pull request
//   goes), else origin/rc. The range is `merge-base(base, head)..head` — three
//   dots, so the report is about this change and not about everything the base
//   gained meanwhile (ci.yml's `changes` job makes the same choice).
//
//   --target is the pull request's base branch (GITHUB_BASE_REF in CI);
//   --source is its head branch (GITHUB_HEAD_REF in CI). Both are only what
//   is said — never guessed from the diff — and the lane checks need both.
//   judge.yml passes a fork's head qualified (`owner/repo:branch`), so a fork
//   never matches a lane. `--head` stays the diff head, a ref in this clone.
//
//   --pr-body reads a pull request body (the workflow pipes
//   `github.event.pull_request.body` to a file) and checks the parts of the
//   template a machine can honestly check.

import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { answer } from './pr-body.mjs'

// The repository the judge measures: the one you are standing in (the workflow
// runs it at the checkout root, and a local run happens in your checkout). Not
// this script's own directory — with a hooksPath or a symlinked tooling copy
// those differ, and the judge would then report on the wrong tree.
const ROOT = process.cwd()

// ── arguments ───────────────────────────────────────────────────────────────

const argv = process.argv.slice(2)
const arg = (name) => {
  const i = argv.indexOf(name)
  return i === -1 ? null : (argv[i + 1] ?? null)
}
const flag = (name) => argv.includes(name)

/** Does `ref` name a commit in this clone? */
const resolves = (ref) => {
  try {
    execFileSync('git', ['rev-parse', '--verify', '--quiet', `${ref}^{commit}`], { cwd: ROOT, stdio: 'ignore' })
    return true
  } catch {
    return false
  }
}

// The diff base: what was said, else the pull request's own base (CI), else
// where a feature pull request goes — testing, when this clone has it.
const baseArg =
  arg('--base') ??
  (process.env.GITHUB_BASE_REF ? `origin/${process.env.GITHUB_BASE_REF}` : resolves('origin/testing') ? 'origin/testing' : 'origin/rc')
const headArg = arg('--head') ?? 'HEAD'
const prBodyPath = arg('--pr-body')
// The pull request's TARGET branch, when something actually knows it: CI does
// (GITHUB_BASE_REF), and `--target` says it by hand. Deliberately NOT the diff
// base — locally the diff base is whatever you forked from, and judging a
// branch forked off `main` as "aimed at main" would be the judge inventing a
// finding, which is the one thing a judge must never do.
const targetArg = arg('--target') ?? process.env.GITHUB_BASE_REF ?? null
// The pull request's HEAD branch, the same way: CI knows it (GITHUB_HEAD_REF,
// empty outside a pull_request event), `--source` says it by hand. It is a
// name, not the diff head — `--head` stays a ref in this clone.
const sourceArg = arg('--source') ?? (process.env.GITHUB_HEAD_REF || null)

// ── git ─────────────────────────────────────────────────────────────────────

// 256 MiB: a roll's CHANGELOG.md diff once overflowed the default buffer
// (ENOBUFS) and the judge crashed instead of judging.
const git = (...args) =>
  execFileSync('git', args, { cwd: ROOT, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 256 * 1024 * 1024 }).trim()
/** Diffs as git objects, never through a filter the tree under judgement could name. */
const DIFF = ['diff', '--text', '--no-textconv', '--no-ext-diff']

const couldNotRun = (why) => {
  console.error(`judge-pr could not run — not a pass: ${why}`)
  process.exit(1)
}

let range
let changed
try {
  // A base that is not in this repository is never a pass: the report would be
  // empty and read as "nothing to see".
  execFileSync('git', ['rev-parse', '--verify', `${baseArg}^{commit}`], { cwd: ROOT, stdio: 'ignore' })
  const mergeBase = git('merge-base', baseArg, headArg)
  range = `${mergeBase}..${headArg}`
  changed = git(...DIFF, '--name-only', range).split('\n').filter(Boolean)
} catch (e) {
  couldNotRun(
    `cannot diff ${baseArg}...${headArg} — fetch the base first (\`git fetch origin testing rc main\`). ${e instanceof Error ? e.message : String(e)}`
  )
}

const headSha = git('rev-parse', '--short=12', headArg)
const baseBranch = baseArg.replace(/^origin\//, '')
const targetBranch = targetArg ? targetArg.replace(/^refs\/heads\//, '') : null
const sourceBranch = sourceArg ? sourceArg.replace(/^refs\/heads\//, '') : null

// ── output ──────────────────────────────────────────────────────────────────

/** Print the report (or the JSON), and append it to the job summary when asked. */
function publish(report, payload) {
  if (flag('--json')) console.log(JSON.stringify(payload, null, 2))
  else console.log(report)
  const summaryPath = process.env.GITHUB_STEP_SUMMARY
  if (flag('--summary') && summaryPath) writeFileSync(summaryPath, `${report}\n`, { flag: 'a' })
}

// ── 0. the lanes whose changes were judged one by one ──────────────────────
//
// A promotion, a snapshot, a back-flow or a sync carries pull requests that
// were each judged on their own. Judged again whole, it only re-finds its own
// shape: promotion #722 drew `targets-main` and `changelog-not-a-file` for
// being the promotion and for carrying the roll. Same-repository is judge.yml's
// to say: it hands a fork's head over qualified, so no lane matches one.

/** The lane that skips the judge, in words, or null. */
function laneSkipOf(target, source) {
  if (!target || !source) return null
  if (target === 'main' && source === 'rc') return 'the promotion of `rc` into `main`'
  if (target === 'rc' && source.startsWith('promote/')) return `a snapshot of \`testing\` promoted into \`rc\` (\`${source}\`)`
  if (target === 'testing' && source.startsWith('backflow/')) return `the \`rc\` → \`testing\` back-flow (\`${source}\`)`
  if (target === 'rc' && source.startsWith('sync/')) return `a sync into \`rc\` (\`${source}\`)`
  return null
}

const laneSkip = laneSkipOf(targetBranch, sourceBranch)
if (laneSkip) {
  const report = [
    `## judge-pr — ${headSha} vs \`${baseBranch}\``,
    '',
    `${changed.length} file(s) changed. 0 must · 0 should · 0 ask.`,
    '',
    `${laneSkip} — each change was judged in its own pull request.`,
  ].join('\n')
  publish(report, {
    head: headSha,
    base: baseBranch,
    files: changed.length,
    lane: laneSkip,
    counts: { must: 0, should: 0, ask: 0 },
    findings: [],
  })
  process.exit(0)
}

// The changelog roll (`release/*` into `rc`) writes CHANGELOG.md by design.
const isRoll = targetBranch === 'rc' && sourceBranch !== null && sourceBranch.startsWith('release/')

// ── findings ────────────────────────────────────────────────────────────────

const findings = []
/** severity: 'must' | 'should' | 'ask' */
const finding = (severity, id, what, detail, fix) =>
  findings.push({ severity, id, what, detail: detail ?? [], fix: fix ?? [] })

const touched = (re) => changed.filter((f) => re.test(f))
const anyTouched = (...res) => res.some((re) => changed.some((f) => re.test(f)))

/** The changelog's added lines, which is where the claim lives. An entry is
 *  an added file under changelog/ (the post-cutover shape) or — only for the
 *  pre-cutover history this judge no longer expects — lines in CHANGELOG.md. */
const changelogAdditions = (() => {
  const additions = []
  // The roll rewrites CHANGELOG.md whole by design; its diff is not read.
  if (changed.includes('CHANGELOG.md') && !isRoll)
    additions.push(
      ...git(...DIFF, '-U0', range, '--', 'CHANGELOG.md')
        .split('\n')
        .filter((l) => l.startsWith('+') && !l.startsWith('+++'))
        .map((l) => l.slice(1)),
    )
  if (changed.some((f) => f.startsWith('changelog/')))
    additions.push(
      ...git(...DIFF, '-U0', range, '--', 'changelog')
        .split('\n')
        .filter((l) => l.startsWith('+') && !l.startsWith('+++'))
        .map((l) => l.slice(1)),
    )
  return additions.length ? additions : null
})()
const changelogText = changelogAdditions ? changelogAdditions.join('\n') : ''
// Bullets appended straight into CHANGELOG.md after the entry-file cutover:
// the exact conflict surface the files exist to end. Not on the roll: writing
// CHANGELOG.md is the roll's whole job.
const appendedToChangelog = !isRoll &&
  changelogAdditions !== null &&
  changed.includes('CHANGELOG.md') &&
  changelogAdditions.some((l) => /^-\s+\*\*/.test(l))

// ── 1. the flow: what this pull request is aimed at ────────────────────────
//
// Only when the target is actually known (CI passes it; `--target` says it).
// A local run's diff base is not a claim about where the change is going.

// The promotion of `rc` itself skipped the judge above; a `--source` that is
// not `rc` (or no source at all) into `main` is the wrong door.
if (targetBranch === 'main' && sourceBranch !== 'rc') {
  finding(
    'must',
    'targets-main',
    `this change is aimed at \`main\``,
    ['main takes one input: the promotion of `rc`, whole, once its staging deploy is green.'],
    [
      'Retarget the pull request at `testing` — it lands there at green and is exercised on the',
      'testing instance; rc takes a snapshot of testing, and main takes rc. `flow.yml` refuses a',
      'pull request to main from anywhere but rc, so this is a red check as well as a wrong door:',
      'docs/BRANCHES.md.',
    ]
  )
}

// rc takes the lanes and nothing else: a snapshot of testing (`promote/*`,
// which skipped the judge above), a hotfix, the roll, a sync. A source that is
// none of them — or no source at all, or a fork's (judge.yml qualifies it) —
// is a feature aimed at the wrong branch.
//
// Once rc enforces it. The judge raises no `must` the target's own policy does
// not hold: the change that brings this rule to rc is judged by rc's previous
// flow-guard in `pr target (flow)` (flow.yml reads the policy from the base),
// and the judge follows the same revision. So the rule wakes when origin/rc's
// flow-guard.mjs has its rc-head rule (`decideRcHead`, anchored by
// check-invariants.mjs); a clone with no such file at origin/rc is judged by it.
const RC_LANES = ['promote/', 'hotfix/', 'release/', 'sync/']
const rcRefusesFeatures = (() => {
  try {
    return git('show', 'origin/rc:scripts/flow-guard.mjs').includes('decideRcHead')
  } catch {
    return true
  }
})()
if (targetBranch === 'rc' && rcRefusesFeatures && !RC_LANES.some((p) => sourceBranch?.startsWith(p))) {
  finding(
    'must',
    'feature-targets-rc',
    'this change is aimed at `rc`, and feature pull requests target `testing`',
    [
      `rc takes a snapshot of testing (\`promote/*\`) and the \`hotfix/\`, \`release/\`, \`sync/\` lanes cut from rc${sourceBranch ? `; \`${sourceBranch}\` is none of them` : ''}.`,
    ],
    [
      'Retarget the pull request at `testing`: `gh pr edit <n> --base testing`. It lands there at',
      'green and is exercised on the testing instance; the next snapshot carries it to rc.',
      '`flow.yml` refuses it into rc too, so this is a red check as well as a wrong door:',
      'docs/BRANCHES.md.',
    ]
  )
}

// A hotfix into `rc` may not add a migration. MIGRATIONS is append-only and an
// entry's INDEX is its identity (ui/src/server/db/pg.ts), so a migration that
// lands on `rc` first takes an index `testing` has already given to another,
// and the two arrays diverge for good. Other pg.ts code is fine in a hotfix.

const PG = 'ui/src/server/db/pg.ts'
const MIGRATIONS_DECL = 'const MIGRATIONS: string[] = ['

/** The index past the string literal that opens at `i` (a quote or a
 *  backtick; a template's `${…}` may hold strings and braces of its own). */
function skipString(text, i) {
  const q = text[i]
  for (let j = i + 1; j < text.length; j++) {
    const c = text[j]
    if (c === '\\') j++
    else if (c === q) return j
    else if (q === '`' && c === '$' && text[j + 1] === '{') j = skipExpr(text, j + 2)
    else if (q !== '`' && c === '\n') return j
  }
  return text.length
}

/** The index of the `}` that closes a template expression opened before `j`. */
function skipExpr(text, j) {
  let depth = 1
  for (; j < text.length; j++) {
    const c = text[j]
    if (c === '`' || c === "'" || c === '"') j = skipString(text, j)
    else if (c === '{') depth++
    else if (c === '}' && --depth === 0) return j
  }
  return j
}

/** The MIGRATIONS array's lines in `text` — 1-based, the declaration line to
 *  its matching `];` — found by bracket depth outside strings and comments, so
 *  a `[` inside a SQL string does not move the end. null when it is absent. */
function migrationsSpan(text) {
  const decl = text.indexOf(MIGRATIONS_DECL)
  if (decl === -1) return null
  let depth = 0
  let i = decl + MIGRATIONS_DECL.length - 1
  for (; i < text.length; i++) {
    const c = text[i]
    if (c === '/' && text[i + 1] === '/') {
      const nl = text.indexOf('\n', i)
      if (nl === -1) break
      i = nl
    } else if (c === '/' && text[i + 1] === '*') {
      const close = text.indexOf('*/', i + 2)
      if (close === -1) break
      i = close + 1
    } else if (c === '`' || c === "'" || c === '"') i = skipString(text, i)
    else if (c === '[') depth++
    else if (c === ']' && --depth === 0) break
  }
  if (depth !== 0) return null
  const lineOf = (at) => text.slice(0, at).split('\n').length
  return { start: lineOf(decl), end: lineOf(i) }
}

/** The new-file line numbers a `git diff -U0` of one file adds. */
function addedLineNumbers(diff) {
  const added = []
  let line = 0
  let inHunk = false
  for (const l of diff.split('\n')) {
    const hunk = /^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(l)
    if (hunk) {
      line = Number(hunk[1])
      inHunk = true
    } else if (!inHunk) continue
    else if (l.startsWith('+')) added.push(line++)
    else if (l.startsWith(' ')) line++
  }
  return added
}

if (targetBranch === 'rc' && sourceBranch?.startsWith('hotfix/') && changed.includes(PG)) {
  let span = null
  try {
    span = migrationsSpan(git('show', `${headArg}:${PG}`))
  } catch {
    /* pg.ts deleted at the head: no array to add to */
  }
  const inside = span
    ? addedLineNumbers(git(...DIFF, '-U0', range, '--', PG)).filter((n) => n >= span.start && n <= span.end)
    : []
  if (inside.length) {
    finding(
      'must',
      'hotfix-adds-migration',
      'a hotfix may not add migrations: the array index is a migration\'s identity and testing\'s array would diverge. Land it through testing.',
      [`  ${PG}: line(s) ${inside.slice(0, 8).join(', ')} inside MIGRATIONS (lines ${span.start}–${span.end})`],
      [
        'Take the migration out of the hotfix and land it through `testing` (a pull request into',
        'testing, then the next promotion). Code elsewhere in pg.ts may stay in the hotfix.',
      ]
    )
  }
}

// A pull request into `testing` may not rewrite an entry `rc` already has. Once
// an entry is promoted, rc's roll owns it (it folds and deletes the file), so an
// edit on testing makes the rc → testing back-flow conflict.

/** Does `path` exist at `ref`? false when the ref does not resolve. */
function existsAt(ref, path) {
  try {
    execFileSync('git', ['cat-file', '-e', `${ref}:${path}`], { cwd: ROOT, stdio: 'ignore' })
    return true
  } catch {
    return false
  }
}

if (targetBranch === 'testing') {
  const rewritten = git(...DIFF, '--name-status', range)
    .split('\n')
    .filter(Boolean)
    .map((l) => l.split('\t'))
    // M and D name the entry; a rename (R…) moves the old path away, which is a delete.
    .filter(([status]) => /^[MDR]/.test(status))
    .map(([, path]) => path)
    .filter((f) => /^changelog\/[^/]+\.md$/.test(f) && f !== 'changelog/README.md')
    .filter((f) => existsAt('origin/rc', f))
  if (rewritten.length) {
    finding(
      'must',
      'changelog-entry-rewritten',
      "this entry is already promoted; rc's roll owns it from here. Add a new entry instead — an edit makes the rc → testing back-flow conflict.",
      rewritten.slice(0, 8).map((f) => `  ${f} (on origin/rc)`),
      ['Restore the entry as it is on `origin/rc` and add changelog/YYYY-MM-DD-<slug>.md for the change.']
    )
  }
}

// ── 2. trees the repo says not to touch ────────────────────────────────────

const OFF_LIMITS = [
  [/^apps\/(leadworks|waypoint)\//, 'a gitignored client subrepo with its own history'],
  [/^fleet\//, 'the rendered fleet, which the app generates from the chassis'],
  [/^mcp\/dist\//, 'mcp build output'],
  [/^ui\/src\/routeTree\.gen\.ts$/, 'generated route code'],
]
for (const [re, what] of OFF_LIMITS) {
  const hit = touched(re)
  if (hit.length) {
    finding('must', 'off-limits', `this change touches ${what}`, hit.slice(0, 8), [
      'AGENTS.md → "Do not touch" names these trees and the reason. If a generated tree really has',
      'to move, regenerate it from its source and say so in the pull request.',
    ])
  }
}

// ── 3. the changelog: an entry for a user-visible change ──────────────────

const USER_VISIBLE = [
  [/^ui\/src\//, 'the app'],
  [/^api\/crates\//, 'the api'],
  [/^cli\/src\//, 'the `talaria` CLI'],
  [/^mcp\/src\//, 'the MCP server'],
  [/^desktop\/(src|src-tauri)\//, 'the desktop shell'],
  [/^scripts\/skills\//, 'the shipped agent skills'],
  [/^(Dockerfile|docker\/)/, 'the container'],
]
// A test is not something a user sees: a diff whose user-visible paths are all
// tests needs no entry (#717 drew a false `no-changelog` for one).
const isTestFile = (f) => /\.(test|spec)\.[cm]?[jt]s$|\.test\.svelte$|(^|\/)tests\//.test(f)
const userVisible = USER_VISIBLE.filter(([re]) => changed.some((f) => re.test(f) && !isTestFile(f)))

// The roll is the changelog: it folds the entries into CHANGELOG.md and asks
// for none of its own.
if (userVisible.length && !isRoll) {
  if (changelogAdditions === null) {
    finding(
      'must',
      'no-changelog',
      'a user-visible change with no changelog entry',
      userVisible.map(([, what]) => `  this change touches ${what}`),
      [
        'Add changelog/YYYY-MM-DD-<slug>.md containing the entry: what changed, in a bold lead',
        'sentence, and what you verified — the gate you ran and how you exercised the path',
        '(CONTRIBUTING.md step 3). The changelog is the record reviewers and release-notes',
        'readers actually have; one file per entry is what keeps two open PRs from colliding.',
      ]
    )
  } else {
    if (appendedToChangelog) {
      finding(
        'must',
        'changelog-not-a-file',
        'the entry was appended into CHANGELOG.md instead of landing as an entry file',
        [],
        [
          'Move it to changelog/YYYY-MM-DD-<slug>.md, verbatim, and leave `## [Unreleased]` empty —',
          'appending at the same anchor as every other open PR is the merge-conflict surface the',
          'entry files exist to end (`bun scripts/changelog-roll.mjs --check` fails this too).',
        ]
      )
    }
    if (!/^-\s+\*\*/m.test(changelogText)) {
      finding(
        'should',
        'changelog-no-lead',
        "the changelog entry has no bold lead sentence",
        changelogText.split('\n').slice(0, 4),
        ['Every entry opens with a bold sentence naming the change, then the reasoning. `git log`',
         'and CHANGELOG.md carry that voice — match the entries around it.'],
      )
    }
    if (!/verif/i.test(changelogText)) {
      finding(
        'should',
        'changelog-no-verification',
        'the changelog entry says nothing about what was verified',
        [],
        ['Name the gate that ran and how the changed path was exercised. "Verified: typecheck" on a',
         'behaviour change is a red flag you should catch yourself (CONTRIBUTING.md step 3).'],
      )
    }
  }
}

// ── 4. the pull request body, when CI handed us one ───────────────────────

if (prBodyPath && existsSync(prBodyPath)) {
  const body = readFileSync(prBodyPath, 'utf8')
  // An answer is what the author added to a section: the template's own lines
  // for it (its comments, its `- **Gate:**` labels, its paragraphs) are not one.
  // scripts/pr-body.mjs says why, and pins it in scripts/judge-pr.test.mjs.
  const templatePath = join(ROOT, '.github/PULL_REQUEST_TEMPLATE.md')
  const template = existsSync(templatePath) ? readFileSync(templatePath, 'utf8') : ''
  const verified = answer(body, 'Verified', template)
  const entry = answer(body, 'CHANGELOG entry', template)
  // CI reviews a pull request with the TRUSTED branch's reviewer, not the one
  // the pull request carries, so a change to the reviewer is measured by the
  // calibration it ran instead: the scorecard belongs in the body.
  const REVIEWER = ['scripts/judge-review.mjs', 'scripts/judge-review-wire.mjs', 'scripts/judge-review.prompt.md', 'scripts/judge-review.calibration.json']
  const reviewerFiles = changed.filter((f) => REVIEWER.includes(f))
  if (reviewerFiles.length && !/scorecard/i.test(body)) {
    finding('ask', 'reviewer-changed-no-scorecard', 'this change edits the model reviewer and the body carries no calibration scorecard', reviewerFiles.map((f) => `  ${f}`), [
      'attach the scorecard from a `judge-calibrate/<name>` push — CI reviews this PR with the trusted',
      "branch's reviewer, not yours (docs/AGENT-TOOLING.md → Calibrating the reviewer).",
    ])
  }
  if (!body.trim()) {
    finding('ask', 'empty-pr-body', 'the pull request has no body', [], [
      'The body mirrors the changelog entry: what changed, and what was verified.',
    ])
  } else {
    if (verified !== null && verified.length < 10) {
      finding('ask', 'pr-body-unfilled', 'the pull request body has no Verified section filled in', [], [
        'Say which gate ran and how the changed path was exercised. That sentence is what a reviewer',
        'checks the diff against.',
      ])
    }
    if (entry !== null && entry.length < 5 && changelogAdditions === null) {
      finding('ask', 'pr-body-no-entry', 'the body names no changelog entry, and none is in the diff', [], [
        'Every user-visible change adds changelog/YYYY-MM-DD-<slug>.md; paste that entry here,',
        'or say why the change is internal-only.',
      ])
    }
  }
}

// ── 5. the skill half, for changes that touch the tooling ─────────────────

if (anyTouched(/^\.claude\/skills\//, /^AGENTS\.md$/)) {
  const newSkills = changed.filter((f) => /^\.claude\/skills\/[^/]+\/SKILL\.md$/.test(f))
  const problems = []
  for (const f of newSkills) {
    // The head's copy, as an object: in CI the working tree is the trusted
    // branch's, which may not have this skill at all.
    let text = ''
    try {
      text = git('show', `${headArg}:${f}`)
    } catch {
      continue // deleted at the head
    }
    const name = f.split('/')[2]
    // The frontmatter block, folded lines and all: a description may be wrapped,
    // and requiring the trigger on its first line would fail a correctly written
    // one.
    const front = /^---\n([\s\S]*?)\n---/.exec(text)?.[1] ?? ''
    const description = /^description:[ \t]*([\s\S]*?)(?=\n[a-z-]+:|\s*$)/m.exec(front)?.[1] ?? ''
    if (!front) problems.push(`  ${f}: no frontmatter block`)
    if (!new RegExp(`^name: ${name}$`, 'm').test(front)) {
      problems.push(`  ${f}: frontmatter name is not "${name}"`)
    }
    if (!/Use when/i.test(description)) {
      problems.push(`  ${f}: the description does not carry its trigger ("Use when …")`)
    }
  }
  if (problems.length) {
    finding('should', 'skill-shape', 'a skill does not match the documented shape', problems, [
      'docs/AGENT-TOOLING.md → The skills format: `name` matching the directory (opencode requires',
      'it), and a `description` written as the trigger, because that is what every harness shows',
      'when deciding whether to read the skill.',
    ])
  }
}

// ── report ──────────────────────────────────────────────────────────────────

const order = { must: 0, should: 1, ask: 2 }
findings.sort((a, b) => order[a.severity] - order[b.severity])

const counts = {
  must: findings.filter((f) => f.severity === 'must').length,
  should: findings.filter((f) => f.severity === 'should').length,
  ask: findings.filter((f) => f.severity === 'ask').length,
}

const lines = []
lines.push(`## judge-pr — ${headSha} vs \`${baseBranch}\``)
lines.push('')
lines.push(`${changed.length} file(s) changed. ${counts.must} must · ${counts.should} should · ${counts.ask} ask.`)
if (findings.length === 0) {
  lines.push('')
  lines.push('Nothing to say: the flow, the changelog entry, the do-not-touch trees and the body all match.')
} else {
  for (const f of findings) {
    lines.push('')
    lines.push(`### ${f.severity}: ${f.what}`)
    for (const d of f.detail) lines.push(d)
    if (f.fix.length) {
      lines.push('')
      for (const l of f.fix) lines.push(l)
    }
  }
  lines.push('')
  lines.push(
    counts.must > 0
      ? 'A `must` is something this repository says about itself — fix it, or argue it in the pull request.'
      : 'Nothing here blocks the change. `should` and `ask` are for the author and the reviewer to settle.'
  )
}
lines.push('')
lines.push("This judge reads the diff. Whether the change works is judge (review)'s reading and a person's.")

publish(lines.join('\n'), { head: headSha, base: baseBranch, files: changed.length, lane: null, counts, findings })

process.exit(counts.must > 0 ? 2 : 0)