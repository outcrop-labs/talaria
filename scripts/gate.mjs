#!/usr/bin/env node
// gate — the local pre-push gate. `check` always; compile and test only what
// this diff touches.
//
// WHY IT EXISTS. `bun run verify`, `api:check` and `desktop:check` are the full
// surface gates CI runs. An agent that runs them locally — or a workspace
// `cargo test` / `cargo clippy` — takes an exclusive lock on `api/target` for
// the whole workspace and, uncapped, every core on the machine. Other agents
// in the same checkout cannot build; every other box on the host stalls.
// CI already skips a surface whose paths did not move (ci.yml's `changes`
// job). This is that decision locally, one level finer: a rust change compiles
// the packages the diff touches, not the workspace.
//
// not a workspace compile. This script never invokes `cargo test --workspace`,
// `cargo clippy --all-targets` without `-p`, `api:check`, or `desktop:check`.
// A lockfile or workspace-manifest change is CI's api job. Do not "finish the
// job" by running those anyway — that is the pin this file exists to stop.
//
// THE SAFE SIDE IS THE OPPOSITE OF CI'S. ci.yml runs everything when the base
// is missing, because a skipped required check is a hole. Here a compile we
// could not scope is the hole: it pins the machine, and CI still runs. A
// missing `origin/rc` is exit 2 and no compile. Fetch, then run again.
//
// USAGE
//   bun run gate            # from the repo root
//   bun run gate --dry-run  # print the plan, run nothing
//   bun scripts/gate.mjs --self-test
//
// Unknown arguments are a usage error. There is no flag that widens this gate.

import { execFileSync, spawnSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'

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

// A workspace-graph file. Compiling one package does not prove it, and
// compiling the workspace is the pin. CI's api job is the gate for these.
const API_GRAPH = new Set(['api/Cargo.toml', 'api/Cargo.lock', 'api/rust-toolchain.toml'])

function readPackageName(rel) {
  const abs = join(ROOT, rel)
  if (!existsSync(abs)) return null
  const text = readFileSync(abs, 'utf8')
  const m = /^\[package\][\s\S]*?^name\s*=\s*"([^"]+)"/m.exec(text)
  return m ? m[1] : null
}

function isRustFile(file) {
  return (
    file.endsWith('.rs') ||
    file.endsWith('/Cargo.toml') ||
    file.endsWith('/Cargo.lock') ||
    file.endsWith('/build.rs') ||
    file.endsWith('/rust-toolchain.toml')
  )
}

function isDesktopRust(file) {
  if (!file.startsWith('desktop/src-tauri/')) return false
  return isRustFile(file) || file.startsWith('desktop/src-tauri/.cargo/')
}

function isDesktopUi(file) {
  if (!file.startsWith('desktop/') || file.startsWith('desktop/src-tauri/')) return false
  return /\.(svelte|ts|js|css)$/.test(file) ||
    /(?:^|\/)(?:package\.json|tsconfig[^/]*\.json|svelte\.config\.[^/]+|vite\.config\.[^/]+)$/.test(file)
}

/**
 * @param {string[]} files
 * @param {(rel: string) => string | null} readPkg
 */
function classify(files, readPkg) {
  const plan = {
    ui: false,
    mcp: false,
    cli: false,
    apiPackages: [],
    apiGraph: [],
    apiUnmapped: [],
    desktopCargo: false,
    desktopTypecheck: false,
  }
  const pkgs = new Set()
  for (const file of files) {
    if (file.startsWith('ui/') || file.startsWith('apps/')) {
      plan.ui = true
    } else if (file.startsWith('mcp/')) {
      plan.mcp = true
    } else if (file.startsWith('cli/')) {
      plan.cli = true
    } else if (file.startsWith('desktop/')) {
      if (isDesktopRust(file)) plan.desktopCargo = true
      else if (isDesktopUi(file)) plan.desktopTypecheck = true
    } else if (file.startsWith('api/')) {
      if (API_GRAPH.has(file) || file.startsWith('api/.cargo/')) {
        plan.apiGraph.push(file)
      } else if (file.startsWith('api/crates/')) {
        if (!isRustFile(file)) continue
        const dir = file.split('/')[2]
        const name = dir ? readPkg(`api/crates/${dir}/Cargo.toml`) : null
        if (name) pkgs.add(name)
        else plan.apiUnmapped.push(file)
      } else if (file.startsWith('api/src/') || file.startsWith('api/tests/') || file === 'api/build.rs') {
        const name = readPkg('api/Cargo.toml')
        if (name) pkgs.add(name)
        else plan.apiUnmapped.push(file)
      }
    }
  }
  plan.apiPackages = [...pkgs].sort()
  return plan
}

function selfTest() {
  const failures = []
  const assert = (cond, msg) => {
    if (!cond) failures.push(msg)
  }
  const stub = (rel) => {
    if (rel === 'api/Cargo.toml') return 'talaria-api'
    if (rel === 'api/crates/talaria-error/Cargo.toml') return 'talaria-error'
    if (rel === 'api/crates/talaria-db/Cargo.toml') return 'talaria-db'
    return null
  }
  const docs = classify(['docs/FOO.md', 'changelog/x.md', '.github/workflows/ci.yml'], stub)
  assert(!docs.ui && !docs.mcp && !docs.cli && docs.apiPackages.length === 0 && !docs.desktopCargo, 'docs-only compiles nothing')
  assert(docs.apiGraph.length === 0, 'a workflow edit is not a local api compile')

  const one = classify(['api/crates/talaria-error/src/lib.rs'], stub)
  assert(one.apiPackages.length === 1 && one.apiPackages[0] === 'talaria-error', 'crate file maps to its package')
  assert(one.apiGraph.length === 0, 'a crate file is not a graph change')

  const graph = classify(['api/Cargo.lock', 'api/Cargo.toml', 'api/.cargo/config.toml'], stub)
  assert(graph.apiPackages.length === 0 && graph.apiGraph.length === 3, 'graph files do not select packages')

  const root = classify(['api/src/db.rs', 'api/tests/it/secretbox.rs'], stub)
  assert(root.apiPackages.length === 1 && root.apiPackages[0] === 'talaria-api', 'api/src and api/tests are the root package')

  const both = classify(['api/crates/talaria-db/src/lib.rs', 'api/crates/talaria-error/Cargo.toml'], stub)
  assert(both.apiPackages.join() === 'talaria-db,talaria-error', 'packages are deduped and sorted')

  const gone = classify(['api/crates/does-not-exist/src/lib.rs'], stub)
  assert(gone.apiPackages.length === 0 && gone.apiUnmapped.length === 1, 'a missing manifest is unmapped, not a workspace compile')

  const dockerfile = classify(['api/package.Dockerfile'], stub)
  assert(dockerfile.apiPackages.length === 0 && dockerfile.apiGraph.length === 0, 'a dockerfile is not a compile')

  const surfaces = classify([
    'ui/src/a.ts',
    'apps/x/y.ts',
    'mcp/src/index.ts',
    'cli/src/ports.ts',
    'desktop/src/App.svelte',
    'desktop/src-tauri/src/main.rs',
  ], stub)
  assert(surfaces.ui && surfaces.mcp && surfaces.cli, 'ui, apps, mcp, cli each select their surface')
  assert(surfaces.desktopTypecheck && surfaces.desktopCargo, 'desktop svelte and rust select separately')

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

let base = null
try {
  git(['rev-parse', '--verify', 'origin/rc^{commit}'])
  base = git(['merge-base', 'HEAD', 'origin/rc'])
} catch {
  base = null
}

if (!base) {
  console.error(
    'gate: origin/rc is not in this clone, so committed changes cannot be scoped. ' +
      'Not a pass. `git fetch origin rc` and run again. Do not run api:check, desktop:check, ' +
      'or a workspace cargo instead — that is the pin this gate exists to avoid.',
  )
  process.exit(2)
}

const files = [...new Set([
  ...lines(git(['diff', '--name-only', base])),
  ...lines(git(['ls-files', '--others', '--exclude-standard'])),
])]
const plan = classify(files, readPackageName)

console.log(`gate: ${files.length} file(s) since ${base.slice(0, 12)} (merge-base with origin/rc)`)
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

if (dry) process.exit(0)

function run(cmd, args, cwd = ROOT) {
  const r = spawnSync(cmd, args, { cwd, stdio: 'inherit' })
  if (r.error) {
    console.error(`gate: could not run ${cmd}: ${r.error.message} — not a pass`)
    process.exit(1)
  }
  if (r.status !== 0) process.exit(r.status ?? 1)
}

run('bun', ['run', 'check'])

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
