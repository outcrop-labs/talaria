#!/usr/bin/env node
// gate — the local pre-push gate. `check` always; compile and test only what
// this diff touches.
//
// WHERE IT SITS. the proof is CI (bun run gate:remote / the PR); this gate is
// for reproducing one failure locally — cheaply, scoped to what CI's log
// already named.
//
// WHY IT EXISTS. `bun run verify`, `api:check` and `desktop:check` are the full
// surface gates CI runs. An agent that runs them locally — or a workspace
// `cargo test` / `cargo clippy` — takes an exclusive lock on `api/target` for
// the whole workspace and, uncapped, every core on the machine. Other agents
// in the same checkout cannot build; every other box on the host stalls.
// CI already skips a surface whose paths did not move (ci.yml's `what changed`
// job, planned by scripts/surfaces.mjs). This is that decision locally, one
// level finer: a rust change compiles the packages the diff touches, not the
// workspace. The plan itself — classify, the install plan, the base — is
// imported from scripts/surfaces.mjs, so the two cannot drift.
//
// not a workspace compile. This script never invokes `cargo test --workspace`,
// `cargo clippy --all-targets` without `-p`, `api:check`, or `desktop:check`.
// A lockfile or workspace-manifest change is CI's api job. Do not "finish the
// job" by running those anyway — that is the pin this file exists to stop.
//
// THE BASE is the merge-base with the nearer of origin/testing and origin/rc —
// whichever leaves fewer commits on HEAD, i.e. the branch this one was cut
// from (surfaces.mjs's pickBase).
//
// THE SAFE SIDE IS THE OPPOSITE OF CI'S. ci.yml runs everything when the base
// is missing, because a skipped required check is a hole. Here a compile we
// could not scope is the hole: it pins the machine, and CI still runs. Neither
// base in the clone is exit 2 and no compile. Fetch, then run again.
//
// THE EXIT CONTRACT (scripts/hooks/README.md — the same one the stop gate and
// the watcher speak):
//   0       pass.
//   2       block. The FIRST stderr line is the token: `gate: base` (no base to
//           scope against) or `gate: red — <cmd> <args> in <dir>` (that command
//           failed). Every command's own output, its stderr included, goes to
//           stdout, so stderr carries only the gate's verdict.
//   other   the gate could not run (not at the repo root, a usage error, a
//           command that would not spawn). Never a pass.
//
// USAGE
//   bun run gate            # from the repo root
//   bun run gate --dry-run  # print the plan, run nothing
//   bun scripts/gate.mjs --self-test
//
// Unknown arguments are a usage error. There is no flag that widens this gate.

import { execFileSync, spawnSync } from 'node:child_process'
import { existsSync, lstatSync } from 'node:fs'
import { join, relative } from 'node:path'
import {
  bunPackages,
  classify,
  installLines,
  missingInstalls,
  pickBase,
  readPackageName,
} from './surfaces.mjs'

const ROOT = process.cwd()
const argv = process.argv.slice(2)

if (!existsSync(join(ROOT, 'scripts/gate.mjs'))) {
  console.error('gate: run from the repo root — not a pass')
  process.exit(1)
}

for (const a of argv) {
  if (a !== '--dry-run' && a !== '--self-test') {
    console.error(
      `gate: unknown argument ${a} — this gate does not widen. CI runs the full surface job.`,
    )
    process.exit(1)
  }
}

function lexistsInRoot(rel) {
  try {
    lstatSync(join(ROOT, rel))
    return true
  } catch {
    return false
  }
}

// What the plan is made of — classify, the install plan, the base — lives in
// scripts/surfaces.mjs, which CI's `what changed` job runs too. Its tests are
// scripts/surfaces.test.mjs. What stays here is the gate's own wiring.
function selfTest() {
  const failures = []
  const assert = (cond, msg) => {
    if (!cond) failures.push(msg)
  }
  const stub = (rel) => {
    if (rel === 'api/Cargo.toml') return 'talaria-api'
    if (rel === 'api/crates/talaria-error/Cargo.toml') return 'talaria-error'
    return null
  }
  const docs = classify(['docs/FOO.md', 'changelog/x.md', '.github/workflows/ci.yml'], stub)

  // A fresh worktree: ui/ linked to the primary's, cli/ and mcp/ not
  // installed. The install lands in the plan for exactly the packages the
  // diff typechecks, and never through the ui symlink.
  const fresh = (rel) => rel === 'ui/node_modules'
  const cliPlan = classify(['cli/src/ports.ts'], stub)
  const cliInstalls = missingInstalls(bunPackages(cliPlan), fresh)
  assert(cliInstalls.join() === 'cli', 'a missing cli/node_modules is installed before the cli typecheck')
  assert(
    installLines(cliInstalls).join('\n') === 'gate: run bun install --frozen-lockfile in cli/ (no cli/node_modules)',
    'the dry-run plan names the install',
  )
  assert(
    missingInstalls(bunPackages(classify(['ui/src/a.ts', 'mcp/src/index.ts'], stub)), fresh).join() === 'mcp',
    'a linked ui/node_modules is present; mcp/ is installed',
  )
  assert(missingInstalls(bunPackages(docs), fresh).length === 0, 'a docs-only diff installs nothing')
  assert(missingInstalls(['cli'], () => true).length === 0, 'an installed package is left alone')

  const real = classify(['api/crates/talaria-error/src/lib.rs'], readPackageName)
  assert(real.apiPackages[0] === 'talaria-error', 'the real talaria-error manifest names talaria-error')

  if (failures.length) {
    console.error(`gate: self-test failed\n${failures.join('\n')}`)
    process.exit(1)
  }
  process.exit(0)
}

if (argv.includes('--self-test')) selfTest()

const dry = argv.includes('--dry-run')

function git(args) {
  return execFileSync('git', args, {
    cwd: ROOT,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  }).trim()
}

function lines(s) {
  return s.split('\n').map((l) => l.trim()).filter(Boolean)
}

const picked = pickBase(git)

if (!picked) {
  console.error('gate: base')
  console.error(
    'Neither origin/testing nor origin/rc is in this clone, so committed changes cannot be scoped. ' +
      'Not a pass. `git fetch origin testing rc` and run again. Do not run api:check, desktop:check, ' +
      'or a workspace cargo instead — that is the pin this gate exists to avoid.',
  )
  process.exit(2)
}
const base = picked.sha

const files = [...new Set([
  ...lines(git(['diff', '--name-only', base])),
  ...lines(git(['ls-files', '--others', '--exclude-standard'])),
])]
const plan = classify(files, readPackageName)

console.log(`gate: ${files.length} file(s) since ${base.slice(0, 12)} (merge-base with ${picked.ref})`)
const say = (on, label) => console.log(`gate: ${on ? 'run' : 'skip'} ${label}`)
say(true, 'check')
say(plan.ui, 'ui typecheck + test')
say(plan.mcp, 'mcp typecheck')
say(plan.cli, 'cli typecheck + test')
if (plan.apiPackages.length) {
  console.log(`gate: run api packages: ${plan.apiPackages.join(' ')} — not a workspace compile`)
} else {
  console.log('gate: skip api compile')
}
if (plan.apiGraph.length) {
  console.log(
    `gate: api graph changed (${plan.apiGraph.join(', ')}) — not a workspace compile. ` +
      "CI's api job is that gate. Do not run api:check or cargo test --workspace.",
  )
}
if (plan.apiUnmapped.length) {
  console.log(
    `gate: api files not mapped to a live package (deleted, or no manifest): ${plan.apiUnmapped.join(', ')}. ` +
      'Not a workspace compile.',
  )
}
say(plan.desktopCargo, 'desktop cargo (talaria-desktop only)')
say(plan.desktopTypecheck, 'desktop typecheck')
say(plan.mobile, 'mobile (shared tests + android assemble)')
const installs = missingInstalls(bunPackages(plan), lexistsInRoot)
for (const line of installLines(installs)) console.log(line)

if (dry) process.exit(0)

// The hooks exit contract (header): a red command is exit 2 with the token as
// the FIRST stderr line, so the command's own stderr goes to our stdout.
function run(cmd, args, cwd = ROOT) {
  const r = spawnSync(cmd, args, { cwd, stdio: ['inherit', 'inherit', 1] })
  if (r.error) {
    console.error(`gate: could not run ${cmd}: ${r.error.message} — not a pass`)
    process.exit(1)
  }
  if (r.status !== 0) {
    console.error(`gate: red — ${[cmd, ...args].join(' ')} in ${relative(ROOT, cwd) || '.'}`)
    process.exit(2)
  }
}

run('bun', ['run', 'check'])

for (const pkg of installs) run('bun', ['install', '--frozen-lockfile'], join(ROOT, pkg))

if (plan.ui) {
  run('bun', ['run', 'typecheck'], join(ROOT, 'ui'))
  run('bun', ['run', 'test'], join(ROOT, 'ui'))
}
if (plan.mcp) run('bun', ['run', 'typecheck'], join(ROOT, 'mcp'))
if (plan.cli) {
  run('bun', ['run', 'typecheck'], join(ROOT, 'cli'))
  run('bun', ['run', 'test'], join(ROOT, 'cli'))
}

if (plan.apiPackages.length) {
  const p = plan.apiPackages.flatMap((name) => ['-p', name])
  const api = join(ROOT, 'api')
  // cwd, not --manifest-path: rustup honors api/rust-toolchain.toml from the
  // workspace directory. A root-cwd cargo is the host default, which is too old.
  run('cargo', ['fmt', ...p, '--check'], api)
  run('cargo', ['clippy', ...p, '--all-targets', '--', '-D', 'warnings'], api)
  run('cargo', ['test', ...p, '--quiet'], api)
}

if (plan.desktopCargo) {
  const desktop = join(ROOT, 'desktop/src-tauri')
  run('cargo', ['fmt', '--check'], desktop)
  run('cargo', ['clippy', '--all-targets', '--', '-D', 'warnings'], desktop)
  run('cargo', ['test', '--quiet'], desktop)
}
if (plan.desktopTypecheck) run('bun', ['run', 'typecheck'], join(ROOT, 'desktop'))

if (plan.mobile) {
  // The wrapper, not a host gradle: the build pins its own Gradle. Needs the
  // JDK + Android SDK a devbox gets from mobile/tools/devbox-install.sh.
  // iOS is absent on purpose — Kotlin/Native cannot build Apple targets off
  // macOS, so CI's macos job is that gate (docs/MOBILE.md).
  run('./gradlew', [':shared:jvmTest', ':androidApp:assembleDebug'], join(ROOT, 'mobile'))
}
