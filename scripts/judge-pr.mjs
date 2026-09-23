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
// USAGE
//   node scripts/judge-pr.mjs [--base <ref>] [--head <ref>] [--pr-body <file>]
//                             [--summary] [--json]
//
//   --base defaults to GITHUB_BASE_REF (CI) or origin/rc (the integration
//   branch, i.e. what a pull request would be measured against locally). The
//   range is `merge-base(base, head)..head` — three dots, so the report is about
//   this change and not about everything the base gained meanwhile (ci.yml's
//   `changes` job makes the same choice).
//
//   --pr-body reads a pull request body (the workflow pipes
//   `github.event.pull_request.body` to a file) and checks the parts of the
//   template a machine can honestly check.

import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

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
// the exact conflict surface the files exist to end.
const appendedToChangelog = changelogAdditions !== null &&
  changed.includes('CHANGELOG.md') &&
  changelogAdditions.some((l) => /^-\s+\*\*/.test(l))

// ── 1. the flow: what this pull request is aimed at ────────────────────────
//
// Only when the target is actually known (CI passes it; `--target` says it).
// A local run's diff base is not a claim about where the change is going.

if (targetBranch === 'main') {
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

if (targetBranch === 'testing') {
  finding(
    'must',
    'targets-testing',
    'this change is aimed at `testing`, which is retired',
    [],
    ['The nightly channel is built from `rc` now. Retarget the pull request at `rc`.']
  )
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

const generatedDocs = touched(/^docs\/(api\/|CLI-REFERENCE\.md)/)
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
 *  The local command is `bun run gate` — it runs the surface's compile only when
 *  the diff touches it. The full CI commands (`verify`, `api:check`,
 *  `desktop:check`) still count if someone actually ran them. The judge holds
 *  the claim to those commands, never to a second opinion. */
const CLAIMS = [
  { re: /^ui\//, needs: /bun run (gate|verify|typecheck|test)|svelte-check/i, what: 'ui/', gate: 'bun run gate' },
  { re: /^mcp\//, needs: /bun run (gate|verify|typecheck)|mcp/i, what: 'mcp/', gate: 'bun run gate' },
  { re: /^api\//, needs: /bun run gate|api:check/i, what: 'api/', gate: 'bun run gate' },
  { re: /^desktop\//, needs: /bun run gate|desktop:check/i, what: 'desktop/', gate: 'bun run gate' },
]

if (changelogAdditions !== null) {
  for (const { re, needs, what, gate } of CLAIMS) {
    if (!anyTouched(re)) continue
    if (!needs.test(changelogText)) {
      finding(
        'should',
        'claim-misses-surface',
        `this change touches \`${what}\` and the verification does not name \`${gate}\``,
        [],
        [`Run it and name it: \`${gate}\`.`],
      )
    }
  }

  // `bun run verify` does not run the cli suite (check + ui/mcp typecheck + the
  // ui suite). `bun run gate` does, when the diff touches cli/. A cli change
  // whose only stated evidence is `verify` is an unverified change.
  if (anyTouched(/^cli\//) && !/bun run gate|cli|bun test/i.test(changelogText)) {
    finding(
      'ask',
      'cli-suite-not-covered-by-verify',
      'this change touches `cli/`, which `bun run verify` does not test',
      [],
      [
        '`bun run gate` runs the cli typecheck and `bun test` when `cli/` moved.',
        '`bun run verify` does not. Name `gate` (or the cli suite) so the claim matches the command.',
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
  const section = (name) => {
    const m = new RegExp(`^##\\s*${name}\\s*$([\\s\\S]*?)(?=^##\\s|\\s*$)`, 'im').exec(body)
    if (!m) return null
    // HTML comments are the template's prompts, not an answer.
    return m[1].replace(/<!--[\s\S]*?-->/g, '').trim()
  }
  const verified = section('Verified')
  const entry = section('CHANGELOG entry')
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
        'Every user-visible change appends to CHANGELOG.md; the entry rides with the change.',
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

const report = lines.join('\n')
if (flag('--json')) {
  console.log(JSON.stringify({ head: headSha, base: baseBranch, files: changed.length, counts, findings }, null, 2))
} else {
  console.log(report)
}

const summaryPath = process.env.GITHUB_STEP_SUMMARY
if (flag('--summary') && summaryPath) {
  writeFileSync(summaryPath, `${report}\n`, { flag: 'a' })
}

process.exit(counts.must > 0 ? 2 : 0)