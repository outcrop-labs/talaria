#!/usr/bin/env node
// judge-pr — measure a change against this repo's standards, from the diff.
//
// WHY IT EXISTS. The gates answer "does it pass". They cannot answer "is this
// the change this repository asked for": that the changelog claims a
// verification the diff does not support, that a behaviour change arrived with
// no test, that the pull request is aimed at `main`, that a generated file moved
// with nothing regenerating it, that a commit subject broke the house style.
// Those are decidable from the diff, and that is this file. The half that is NOT
// decidable — "does this actually work in Talaria" — is the skill beside it
// (`.claude/skills/judge-pr/SKILL.md`), because it needs someone to read the
// change against the app.
//
// WHAT IT IS NOT. It does not run the gates (that is `bun run verify`, and CI),
// and it does not replace a reviewer. Its findings are graded:
//
//   must    the repo states this about itself — branch flow, do-not-touch trees,
//           a user-visible change with no changelog entry, generated drift.
//   should  a convention with a reason behind it (commit subjects, a gate claim
//           that misses the surface it covers).
//   ask     a question for the human. NEVER a reason to hold a change.
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
// entry `rc` already has.
//
// USAGE
//   node scripts/judge-pr.mjs [--base <ref>] [--head <ref>] [--target <branch>]
//                             [--source <branch>] [--pr-body <file>]
//                             [--summary] [--json]
//
//   --base defaults to GITHUB_BASE_REF (CI) or origin/rc (the integration
//   branch, i.e. what a pull request would be measured against locally). The
//   range is `merge-base(base, head)..head` — three dots, so the report is about
//   this change and not about everything the base gained meanwhile (ci.yml's
//   `changes` job makes the same choice).
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

const baseArg = arg('--base') ?? 'origin/rc'
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

const git = (...args) =>
  execFileSync('git', args, { cwd: ROOT, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim()

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
  changed = git('diff', '--name-only', range).split('\n').filter(Boolean)
} catch (e) {
  couldNotRun(
    `cannot diff ${baseArg}...${headArg} — fetch the base first (\`git fetch origin rc main\`). ${e instanceof Error ? e.message : String(e)}`
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
  if (changed.includes('CHANGELOG.md'))
    additions.push(
      ...git('diff', '-U0', range, '--', 'CHANGELOG.md')
        .split('\n')
        .filter((l) => l.startsWith('+') && !l.startsWith('+++'))
        .map((l) => l.slice(1)),
    )
  if (changed.some((f) => f.startsWith('changelog/')))
    additions.push(
      ...git('diff', '-U0', range, '--', 'changelog')
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
      'Retarget the pull request at `rc` — review, CI and the staging deploy all happen there,',
      'and the promotion follows on its own. `flow.yml` refuses a pull request to main from',
      'anywhere but rc, so this is a red check as well as a wrong door: docs/BRANCHES.md.',
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
    ? addedLineNumbers(git('diff', '-U0', range, '--', PG)).filter((n) => n >= span.start && n <= span.end)
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
  const rewritten = git('diff', '--name-status', range)
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

const productSkills = touched(/^scripts\/skills\//)
if (productSkills.length) {
  finding(
    'ask',
    'product-skills',
    '`scripts/skills/` is product surface — those skills ship into Hermes agent containers',
    productSkills.slice(0, 8),
    ['If this is REPO TOOLING rather than product, it belongs in `.claude/skills/` (AGENTS.md).']
  )
}

// ── 3. generated references must be generated, not edited ──────────────────

const generatedDocs = touched(/^(docs\/(api\/|CLI-REFERENCE\.md)|scripts\/skills\/talaria-apps\/references\/skeleton\.md)/)
if (generatedDocs.length) {
  let drift = ''
  try {
    execFileSync('bun', ['scripts/gen-docs.mjs', '--check'], { cwd: ROOT, encoding: 'utf8' })
  } catch (e) {
    drift = `${e.stdout ?? ''}${e.stderr ?? ''}`.trim().split('\n').slice(-6).join('\n')
  }
  if (drift) {
    finding(
      'must',
      'generated-drift',
      'generated documentation changed and the generator disagrees',
      drift.split('\n'),
      [
        'These files are regenerated, never hand-edited: run `bun run docs:api` and commit the',
        'result. `bun run check` fails on the same drift, so this is also a red CI job.',
      ]
    )
  } else {
    finding('ask', 'generated-changed', 'generated documentation moved', generatedDocs.slice(0, 8), [
      'The generator is clean, so this is its output — good. Worth a sentence in the pull request',
      'on what changed in the reference, since reviewers read that page.',
    ])
  }
}

// ── 4. the changelog: an entry, and a verification that matches the diff ──

const USER_VISIBLE = [
  [/^ui\/src\//, 'the app'],
  [/^api\/crates\//, 'the api'],
  [/^cli\/src\//, 'the `talaria` CLI'],
  [/^mcp\/src\//, 'the MCP server'],
  [/^desktop\/(src|src-tauri)\//, 'the desktop shell'],
  [/^scripts\/skills\//, 'the shipped agent skills'],
  [/^(Dockerfile|docker\/)/, 'the container'],
  [/^RELEASING\.md$/, 'the release contract'],
]
const userVisible = USER_VISIBLE.filter(([re]) => anyTouched(re))

if (userVisible.length) {
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

// ── 5. does the claimed verification cover the surfaces the diff touches? ──

/** What a change to each surface has to have named, and where that comes from:
 *  CONTRIBUTING.md's "Before you send a PR" and the dev-loop skill's gate table.
 *  CI is the compile: `bun run gate:remote` runs the surface's CI jobs on a
 *  `ci/` push before a pull request exists, and the pull request's own checks
 *  run them after — so either, named, covers every surface. Locally,
 *  `bun run gate` runs the surface's compile when the diff touches it, and the
 *  full CI commands (`verify`, `api:check`, `desktop:check`) still count if
 *  someone actually ran them. The judge holds the claim to those, never to a
 *  second opinion. `CI` is matched in capitals only: a `ci.yml` in the entry is
 *  not a claim that CI ran. */
const CI_EVIDENCE = /gate:remote|\bCI\b/
const CLAIMS = [
  { re: /^ui\//, needs: /bun run (gate|gate:remote|verify|typecheck|test)|svelte-check/i, what: 'ui/' },
  { re: /^mcp\//, needs: /bun run (gate|gate:remote|verify|typecheck)|mcp/i, what: 'mcp/' },
  { re: /^api\//, needs: /bun run gate|gate:remote|api:check/i, what: 'api/' },
  { re: /^desktop\//, needs: /bun run gate|gate:remote|desktop:check/i, what: 'desktop/' },
]
const NAME_THE_GATE =
  "Name the gate: `bun run gate:remote`, or 'CI: this PR's checks' (put run links in the PR body, not the entry)."

if (changelogAdditions !== null) {
  for (const { re, needs, what } of CLAIMS) {
    if (!anyTouched(re)) continue
    if (!needs.test(changelogText) && !CI_EVIDENCE.test(changelogText)) {
      finding(
        'should',
        'claim-misses-surface',
        `this change touches \`${what}\` and the verification names no gate that covers it`,
        [],
        [NAME_THE_GATE],
      )
    }
  }

  // `bun run verify` does not run the cli suite (check + ui/mcp typecheck + the
  // ui suite). `bun run gate` does when the diff touches cli/, and so does CI
  // (`bun run gate:remote`, or the pull request's checks). A cli change whose
  // only stated evidence is `verify` is an unverified change.
  if (anyTouched(/^cli\//) && !/bun run gate|cli|bun test/i.test(changelogText) && !CI_EVIDENCE.test(changelogText)) {
    finding(
      'ask',
      'cli-suite-not-covered-by-verify',
      'this change touches `cli/`, which `bun run verify` does not test',
      [],
      [
        '`bun run gate` runs the cli typecheck and `bun test` when `cli/` moved, and so does CI.',
        '`bun run verify` does not. ' + NAME_THE_GATE,
      ]
    )
  }
}

// ── 6. a behaviour change with no test ────────────────────────────────────

const isTest = (f) => /\.(test|spec)\.[cm]?[jt]s$/.test(f) || /\.test\.svelte$/.test(f)
const BEHAVIOUR = [/^ui\/src\/.*\.(ts|svelte)$/, /^cli\/src\/.*\.ts$/, /^mcp\/src\/.*\.ts$/]
const behaviour = changed.filter((f) => BEHAVIOUR.some((re) => re.test(f)) && !isTest(f))
const tests = changed.filter(isTest)
if (behaviour.length && tests.length === 0) {
  finding(
    'ask',
    'no-test',
    'behaviour changed and no test moved with it',
    behaviour.slice(0, 8),
    [
      'Not every change needs one — a test earns its place only where a plausible bug would fail',
      'it (CONTRIBUTING.md, "Before you send a PR"). So this is a question, not a verdict: either',
      'the change is covered by an existing test (name it), or it is the kind that cannot be',
      '(say how you exercised it instead).',
    ]
  )
}

// ── 7. commit subjects, in the house style ────────────────────────────────

let subjects = []
try {
  subjects = git('log', '--no-merges', '--format=%s', range).split('\n').filter(Boolean)
} catch {
  /* an empty range is fine */
}
const SUBJECT = /^[a-z][a-z0-9/-]*: [a-z]/
const odd = subjects.filter((s) => !SUBJECT.test(s) || s.length > 88 || s.endsWith('.'))
if (odd.length) {
  finding(
    'should',
    'commit-subject',
    'a commit subject is not in the house style',
    odd.map((s) => `  ${s}`),
    [
      'The shape is `area: lowercase sentence — explanation`: an area prefix, a lowercase',
      'sentence, and an em-dash clause that says WHY rather than what (CONTRIBUTING.md step 4, and',
      '`git log` carries the voice). No trailing period.',
    ]
  )
}

// ── 8. the pull request body, when CI handed us one ───────────────────────

if (prBodyPath && existsSync(prBodyPath)) {
  const body = readFileSync(prBodyPath, 'utf8')
  // An answer is what the author added to a section: the template's own lines
  // for it (its comments, its `- **Gate:**` labels, its paragraphs) are not one.
  // scripts/pr-body.mjs says why, and pins it in scripts/judge-pr.test.mjs.
  const templatePath = join(ROOT, '.github/PULL_REQUEST_TEMPLATE.md')
  const template = existsSync(templatePath) ? readFileSync(templatePath, 'utf8') : ''
  const verified = answer(body, 'Verified', template)
  const entry = answer(body, 'CHANGELOG entry', template)
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

// ── 9. the skill half, for changes that touch the tooling ─────────────────

if (anyTouched(/^\.claude\/skills\//, /^AGENTS\.md$/)) {
  const newSkills = changed.filter((f) => /^\.claude\/skills\/[^/]+\/SKILL\.md$/.test(f))
  const problems = []
  for (const f of newSkills) {
    const text = readFileSync(join(ROOT, f), 'utf8')
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
  lines.push('Nothing to say: the flow, the changelog claim, the surfaces and the generated trees all match.')
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
  lines.push(
    'This judge reads the diff; it cannot tell whether the change WORKS. That half is a reader: ' +
      '`.claude/skills/judge-pr/SKILL.md`.'
  )
}

publish(lines.join('\n'), { head: headSha, base: baseBranch, files: changed.length, lane: null, counts, findings })

process.exit(counts.must > 0 ? 2 : 0)