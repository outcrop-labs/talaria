// release.yml's tag provenance: a stable vX.Y.Z publishes only from a commit on
// main, and a vX.Y.Z-rc.N only from one on rc. Run by `bun run check`. Stdlib
// only, no network.
//
// WHAT IS UNDER TEST IS THE WORKFLOW'S OWN SHELL. The `resolve` step's `run:`
// block is read out of .github/workflows/release.yml and run under bash with
// the environment a runner gives it. Its fetch points at a local repository
// served over file:// instead of GitHub. The history is shaped like the real
// one: main moves only by promotion merges of rc, rc moves on after a
// promotion, and a scratch branch stands in for testing or a feature branch.
// A copy of the logic here would pass after the workflow changed.

import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const WORKFLOW = join(ROOT, '.github/workflows/release.yml')
const STEP = '- name: Work out what this run publishes'

let failed = 0
let ran = 0

function check(name, fn) {
  ran++
  try {
    fn()
  } catch (e) {
    failed++
    console.error(`FAIL ${name}: ${e instanceof Error ? e.message : e}`)
  }
}

/** The step's `run: |` block, dedented: every line after `run: |` that is
 *  blank or indented deeper than the `run:` key. */
function resolveScript(text) {
  const lines = text.split('\n')
  const start = lines.findIndex((l) => l.trim() === STEP)
  if (start < 0) throw new Error(`release.yml has no step "${STEP.slice(8)}"`)
  const runAt = lines.findIndex((l, i) => i > start && /^\s*run: \|\s*$/.test(l))
  if (runAt < 0) throw new Error('the resolve step has no `run: |` block')
  const keyIndent = lines[runAt].search(/\S/)
  const body = []
  for (const l of lines.slice(runAt + 1)) {
    if (l.trim() !== '' && l.search(/\S/) <= keyIndent) break
    body.push(l)
  }
  const indent = Math.min(...body.filter((l) => l.trim()).map((l) => l.search(/\S/)))
  return body.map((l) => l.slice(indent)).join('\n')
}

const isolated = {
  PATH: process.env.PATH,
  HOME: process.env.HOME ?? tmpdir(),
  GIT_CONFIG_GLOBAL: '/dev/null',
  GIT_CONFIG_NOSYSTEM: '1',
  GIT_AUTHOR_NAME: 'provenance-test',
  GIT_AUTHOR_EMAIL: 'provenance-test@example.invalid',
  GIT_COMMITTER_NAME: 'provenance-test',
  GIT_COMMITTER_EMAIL: 'provenance-test@example.invalid',
}

function git(cwd, ...args) {
  const r = spawnSync('git', args, { cwd, env: isolated, encoding: 'utf8' })
  if (r.status !== 0) throw new Error(`git ${args.join(' ')}: ${r.stderr}`)
  return r.stdout.trim()
}

const base = realpathSync(mkdtempSync(join(tmpdir(), 'release-provenance-test-')))
try {
  const script = readFileSync(WORKFLOW, 'utf8')
  const run = resolveScript(script)
  const runFile = join(base, 'resolve.sh')
  writeFileSync(runFile, run)

  check('the resolve script takes everything from env, not from ${{ }}', () => {
    if (run.includes('${{')) throw new Error('an expression in run: would be blank here and is an injection path on GitHub')
    if (!run.includes('on_branch rc') || !run.includes('on_branch main')) throw new Error('the tag cases do not call on_branch')
  })

  // The served repository: <base>/outcrop/talaria, reached as
  // file://<base>/outcrop/talaria the way the step builds its URL.
  const repo = join(base, 'outcrop', 'talaria')
  git(base, 'init', '-q', '-b', 'main', repo)
  git(repo, 'config', 'uploadpack.allowFilter', 'true')
  const commit = (msg) => {
    git(repo, 'commit', '-q', '--allow-empty', '-m', msg)
    return git(repo, 'rev-parse', 'HEAD')
  }
  const a = commit('a: the first commit, on main')
  git(repo, 'checkout', '-q', '-b', 'rc')
  const b = commit('b: a feature merged into rc')
  git(repo, 'checkout', '-q', 'main')
  git(repo, 'merge', '-q', '--no-ff', '-m', 'promote: rc into main', 'rc')
  const promotion = git(repo, 'rev-parse', 'HEAD')
  git(repo, 'checkout', '-q', 'rc')
  const rcTip = commit('d: rc moves on after the promotion')
  git(repo, 'checkout', '-q', '-b', 'scratch', b)
  const scratch = commit('c: only on a scratch branch (testing, or a feature)')
  git(repo, 'checkout', '-q', 'main')

  /** Tag `sha` as `tag` (annotated, as tag-release.yml cuts them) and run the
   *  step as a push of that tag. GITHUB_SHA is the tagged commit, as GitHub
   *  sets it for an annotated tag's push. */
  function asTagPush(tag, sha) {
    git(repo, 'tag', '-f', '-a', '-m', tag, tag, sha)
    return runStep({ EVENT: 'push', GITHUB_REF_TYPE: 'tag', GITHUB_REF_NAME: tag, GITHUB_REF: `refs/tags/${tag}`, GITHUB_SHA: sha })
  }

  function runStep(vars) {
    const temp = mkdtempSync(join(base, 'runner-'))
    const output = join(temp, 'output')
    writeFileSync(output, '')
    const r = spawnSync('bash', ['--noprofile', '--norc', '-eo', 'pipefail', runFile], {
      env: {
        ...isolated,
        DISPATCH_CHANNEL: '',
        GH_TOKEN: 'not-a-real-token',
        GITHUB_SERVER_URL: `file://${base}`,
        GITHUB_REPOSITORY: 'outcrop/talaria',
        GITHUB_OUTPUT: output,
        RUNNER_TEMP: temp,
        ...vars,
      },
      encoding: 'utf8',
    })
    const out = Object.fromEntries(
      readFileSync(output, 'utf8')
        .split('\n')
        .filter(Boolean)
        .map((l) => [l.slice(0, l.indexOf('=')), l.slice(l.indexOf('=') + 1)]),
    )
    return { status: r.status, log: `${r.stdout}${r.stderr}`, out, fetched: existsSync(join(temp, 'provenance')) }
  }

  const passes = (r, channel) => {
    if (r.status !== 0) throw new Error(`refused (exit ${r.status}): ${r.log.trim()}`)
    if (r.out.channel !== channel) throw new Error(`channel ${r.out.channel}, wanted ${channel}`)
    if (!/provenance: .* is on /.test(r.log)) throw new Error(`no provenance line: ${r.log.trim()}`)
  }
  const refused = (r, branch) => {
    if (r.status === 0) throw new Error(`published: ${JSON.stringify(r.out)}`)
    if (!new RegExp(`::error::tag \\S+ points at [0-9a-f]{12}, which is not on ${branch} `).test(r.log)) {
      throw new Error(`refused for another reason: ${r.log.trim()}`)
    }
    if (Object.keys(r.out).length) throw new Error(`wrote outputs before refusing: ${JSON.stringify(r.out)}`)
  }

  check('a stable tag on main publishes', () => passes(asTagPush('v1.0.0', promotion), 'stable'))
  check('a stable tag on main below its tip publishes', () => passes(asTagPush('v0.9.0', a), 'stable'))
  check('an rc tag on rc publishes', () => passes(asTagPush('v1.1.0-rc.1', rcTip), 'rc'))
  check('an rc tag on rc below its tip publishes', () => passes(asTagPush('v1.0.0-rc.1', b), 'rc'))
  check('a stable tag on a commit only rc has is refused', () => refused(asTagPush('v1.1.0', rcTip), 'main'))
  check('a stable tag on a scratch branch is refused', () => refused(asTagPush('v1.2.0', scratch), 'main'))
  check('an rc tag on a scratch branch is refused', () => refused(asTagPush('v1.2.0-rc.1', scratch), 'rc'))
  check("an rc tag on main's promotion merge, which rc does not have, is refused", () =>
    refused(asTagPush('v1.0.1-rc.1', promotion), 'rc'),
  )

  // tag-release.yml's hand-off, which is how a stable release really starts:
  // it pushes the tag with the workflow token, whose push starts no run, and
  // dispatches this workflow AT the tag. A stable dispatch first asks GitHub
  // for every tag (the newest-stable check), so a `gh` on PATH answers from
  // the served repository. GITHUB_SHA is given as the tag OBJECT, the
  // unpeeled form: the check must hold whichever form a dispatch reports.
  const bin = join(base, 'bin')
  mkdirSync(bin)
  writeFileSync(
    join(bin, 'gh'),
    [
      '#!/bin/sh',
      'case "$*" in',
      `  "api repos/outcrop/talaria/git/matching-refs/tags/v "*) exec git -C '${repo}' tag -l 'v*' ;;`,
      'esac',
      'echo "fake gh: unexpected call: $*" >&2',
      'exit 1',
      '',
    ].join('\n'),
    { mode: 0o755 },
  )
  function asTagDispatch(tag, sha) {
    git(repo, 'tag', '-f', '-a', '-m', tag, tag, sha)
    return runStep({
      EVENT: 'workflow_dispatch',
      PATH: `${bin}:${process.env.PATH}`,
      GITHUB_REF_TYPE: 'tag',
      GITHUB_REF_NAME: tag,
      GITHUB_REF: `refs/tags/${tag}`,
      GITHUB_SHA: git(repo, 'rev-parse', tag),
    })
  }

  check("a dispatch at a stable tag on main (tag-release.yml's hand-off) publishes", () =>
    passes(asTagDispatch('v9.0.0', promotion), 'stable'),
  )
  check('a dispatch at the newest stable tag, cut on a scratch branch, is refused', () =>
    refused(asTagDispatch('v9.1.0', scratch), 'main'),
  )
  check('a dispatch at an rc tag cut on a scratch branch is refused', () =>
    refused(asTagDispatch('v9.1.0-rc.1', scratch), 'rc'),
  )

  check('a malformed tag fails on its shape, before any fetch', () => {
    const r = asTagPush('v1.0', promotion)
    if (r.status === 0) throw new Error('published')
    if (!r.log.includes('is neither vX.Y.Z nor vX.Y.Z-rc.N')) throw new Error(`refused for another reason: ${r.log.trim()}`)
    if (r.fetched) throw new Error('fetched before the shape check')
  })

  check('the nightly schedule builds rc with no provenance check', () => {
    const r = runStep({ EVENT: 'schedule', GITHUB_REF_TYPE: 'branch', GITHUB_REF_NAME: 'main', GITHUB_REF: 'refs/heads/main', GITHUB_SHA: promotion })
    if (r.status !== 0) throw new Error(`exit ${r.status}: ${r.log.trim()}`)
    if (r.out.channel !== 'nightly' || r.out.ref !== 'rc') throw new Error(JSON.stringify(r.out))
    if (r.fetched) throw new Error('a schedule run fetched for a provenance check')
  })

  check('a channel dispatch from a branch builds the rc tip with no provenance check', () => {
    const r = runStep({ EVENT: 'workflow_dispatch', DISPATCH_CHANNEL: 'rc', GITHUB_REF_TYPE: 'branch', GITHUB_REF_NAME: 'scratch', GITHUB_REF: 'refs/heads/scratch', GITHUB_SHA: scratch })
    if (r.status !== 0) throw new Error(`exit ${r.status}: ${r.log.trim()}`)
    if (r.out.channel !== 'rc' || r.out.ref !== 'rc') throw new Error(JSON.stringify(r.out))
    if (r.fetched) throw new Error('a channel dispatch fetched for a provenance check')
  })

  check('an unreachable repository refuses rather than publishing', () => {
    git(repo, 'tag', '-f', '-a', '-m', 'v2.0.0', 'v2.0.0', promotion)
    const r = runStep({
      EVENT: 'push',
      GITHUB_REF_TYPE: 'tag',
      GITHUB_REF_NAME: 'v2.0.0',
      GITHUB_REF: 'refs/tags/v2.0.0',
      GITHUB_SHA: promotion,
      GITHUB_REPOSITORY: 'outcrop/missing',
    })
    if (r.status === 0) throw new Error('published')
    if (!r.log.includes('could not fetch main, rc and refs/tags/v2.0.0')) throw new Error(`refused for another reason: ${r.log.trim()}`)
  })
} finally {
  rmSync(base, { recursive: true, force: true })
}

if (failed) {
  console.error(`release-provenance: ${failed} of ${ran} failed`)
  process.exit(1)
}
console.log(`release-provenance: ${ran} ok`)
