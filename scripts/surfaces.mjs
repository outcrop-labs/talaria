#!/usr/bin/env node
// surfaces — which parts of the tree a change touches, decided ONCE for CI
// (ci.yml's `what changed` job) and for the local gate (scripts/gate.mjs).
//
// WHY ONE MODULE. The surface lists used to live twice: as bash in ci.yml's
// `changes` job and as a hand copy in gate.mjs, held together by a self-test
// that parsed the workflow. They now live here, and both callers import them.
// The same module also picks the CI MODE and the DIFF RANGE, which used to be
// a bash `if` that only knew "PR" and "push".
//
// STDLIB ONLY, AND SELF-CONTAINED. It imports nothing but `node:*`, and never
// a sibling file: for a pull request ci.yml runs the copy of this file from
// the PR's BASE sha, materialized into $RUNNER_TEMP, so a PR cannot narrow its
// own CI by editing the planner. Nothing here may assume it sits in scripts/.
//
// THE CONTRACT: the env/output interface changes ADDITIVELY only — main's
// ci.yml and base-materialized copies run against newer trees. A key is never renamed or
// dropped. A new output key needs a default in ci.yml's "Normalize the plan"
// step that runs MORE (a surface defaults to true), because an older copy of
// this file will not emit it.
//
// THE MODES (`mode=`; ci.yml's platform jobs read it):
//   light        a pull request into anything but rc or main (testing, a
//                stacked branch), a push to ci/<branch> (the offload lane,
//                ci-offload.yml), any other push. The surfaces the diff moved,
//                Linux jobs only: macOS, Windows, iOS and e2e are skipped.
//   integration  a push to testing. Linux jobs, on everything since testing's
//                last green push run.
//   full         a pull request into rc or main, a push to rc or main,
//                release.yml's call, a push to ci/full/<branch>, and an offload
//                of a hotfix/, release/, sync/, promote/ or backflow/ branch
//                (those lanes go straight into rc). The platform jobs run with
//                their surface.
//   none         the push deleted the branch. Nothing runs.
//
// THE RANGES. The safe side of every unknown is "every surface runs": a missing
// base, a rewritten history, a diff that will not resolve. A skipped check is
// cheap; a check nobody ran is the thing this repo keeps paying for.
//   pull request     HEAD^1..HEAD on the merge ref (built against the exact
//                    base tip), else <base sha>...HEAD.
//   push main        since main's last green push run of ci.yml, else the
//                    push's `before`, else everything.
//   push rc          since rc's last green push run, else the merge-base
//                    with origin/main.
//   push testing     since testing's last green push run, else the merge-base
//                    with origin/rc.
//   other push and   since the merge-base with the nearer of origin/testing and
//   a light call     origin/rc (pickBase).
//   full or integration call, or a call with no mode (release.yml): everything.
//
// "Since the last green run", not "since `before`": on a long-lived branch a
// push whose run was cancelled or went red is covered by the next push's run,
// because that run starts from the last commit CI actually passed.
//
// THE API PLAN. Beside the surfaces, `ci` writes the api job's test plan
// (API_KEYS: the scope, the packages, the live modules, whether postgres and
// redis start, whether the binary is uploaded) and the upgrade baseline of the
// migrations job (`migration_baseline`). "The api job's scope" below has the
// rules.
//
// USAGE
//   node scripts/surfaces.mjs ci                 # ci.yml: env in, $GITHUB_OUTPUT out
//   node scripts/surfaces.mjs plan [--full] [--json]
//                                                # this branch, as an offload run would see it
//
// The tests are scripts/surfaces.test.mjs (run by `bun run check`).

import { execFileSync } from 'node:child_process'
import { appendFileSync, existsSync, readFileSync, readdirSync, realpathSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

// ── The surfaces ─────────────────────────────────────────────────────────────
//
// One list of globs per CI job family. A glob has bash `case` semantics: `*`
// spans `/`, every other character is literal (see globMatch).
//
// The job-definition files are in EVERY list on purpose: ci.yml, the offload
// workflow that calls it, this planner, and the composite actions every job
// installs through. An edit to any of them changes what these jobs are, so the
// run that carries it should exercise all of them.
//
// docs/sdk/ and the app scaffolder are in the ui list because the ui suite
// compiles the SDK docs' examples and every file `talaria app new` writes
// against the SDK (ui/src/sdk/client-docs.test.ts and server.test.ts). Without
// them, a docs-only or scaffold-only edit that stops compiling skips this job,
// and a skip reports success.
//
// The same rule holds for every file a job reads from OUTSIDE its own
// directory: each one below is a file the job's tests read, import or compile
// in, so an edit to it alone can turn the job red.
//   ui    the fixtures it shares with the api (app grants, the app manifest,
//         secretbox vectors and their generator), the Rust sources it holds its
//         tables to (the harness registry, the gateway guard, the platform
//         agents), the chassis template and the workbench image files
//         (chassis.test.ts), and check-invariants.mjs
//         (server-import-invariant.test.ts runs it).
//   cli   every api .rs file (box.test.ts reads them for the env keys a box's
//         api must be handed), the grants fixture, docs/CONTAINER.md's up
//         command, the devbox compose, and the repo's .gitignore
//         (ignore.test.ts asks git about it).
//   api   the chassis template, the Hermes toolset and skill-authority files
//         (include_str!), the shipped skills tree, mcp/src/index.ts (the
//         fitness catalog's sync test), and the migration array with the
//         script that replays it: the job's live steps run the database suite
//         against the schema that array builds.
//   desktop  ui/src/lib/dither-engine.ts, which the launcher imports as
//         `@dither` (desktop/vite.config.ts, tsconfig.json).
//   db    (ci.yml's `migrations (replay + upgrade)` job) the array and its
//         snapshot, the replay, upgrade and mirror-check scripts, and
//         ui/package.json, which names those scripts and pins the postgres
//         driver they run on.
//
// The end-to-end suite (ui/e2e/*.spec.ts: the MCP story and the notification
// specs) boots the api binary its job builds with `cargo build --bin
// talaria-api`, the built ui and the stubs. So its surface is everything that
// goes into that binary, not a list of the crates a spec seems to be about: the
// root package depends on every workspace crate, 218 of them directly and the
// rest through those (243 of 243 on 2026-11-02, read from the manifests), so a
// change to any crate, a shared one such as talaria-db, -state, -auth, -config,
// -error, -params or -permissions included, can change what the specs meet. A
// hand list of the "relevant" crates missed exactly those, and a required check
// this filter skips still reports success (job-level `if`). `api/crates/*` also
// covers a crate added later with no edit here. With it: the binary itself
// (api/src), the workspace manifest and lockfile (a dependency's features, such
// as the TLS roots the stubs' throwaway CA relies on, can change with no
// lockfile line), the build's own config and toolchain pin, and the chassis
// template the job copies in for the hire. Not api/tests or the package
// Dockerfile: neither goes into the binary. The cost is that job on every Rust
// pull request; it runs beside the api job, which takes longer, so the wait for
// a green run does not grow.

/** The files that define what the CI jobs are. In every surface. */
export const JOB_FILES = [
  '.github/workflows/ci.yml',
  '.github/workflows/ci-offload.yml',
  'scripts/surfaces.mjs',
  '.github/actions/**',
]

export const SURFACES = {
  ui: [
    'ui/*', 'apps/*', 'docs/sdk/*', 'cli/src/cmd/app/*',
    'api/tests/it/fixtures/*', 'api/crates/talaria-harness-defs/src/registry.rs',
    'api/crates/talaria-gateway/src/guard.rs', 'api/crates/talaria-platform-agents/src/lib.rs',
    'scripts/chassis.template.yml', 'scripts/workbench.Dockerfile', 'scripts/build-workbench-image.sh',
    'scripts/check-invariants.mjs', 'scripts/gen-secretbox-vectors.mjs',
    ...JOB_FILES,
  ],
  mcp: ['mcp/*', ...JOB_FILES],
  cli: [
    'cli/*', 'api/*.rs', 'api/tests/it/fixtures/*', 'docs/CONTAINER.md', 'docker/devbox.compose.yml',
    '.gitignore',
    ...JOB_FILES,
  ],
  api: [
    'api/*', 'scripts/chassis.template.yml', 'scripts/hermes-toolsets.json',
    'scripts/hermes-skill-authority.json', 'scripts/skills/*', 'mcp/src/index.ts',
    'ui/src/server/db/*', 'ui/scripts/check-migrations.ts',
    ...JOB_FILES,
  ],
  desktop: ['desktop/*', 'ui/src/lib/dither-engine.ts', ...JOB_FILES],
  mobile: ['mobile/*', ...JOB_FILES],
  e2e: [
    'ui/*', 'scripts/e2e/*', 'api/crates/*', 'api/src/*', 'api/Cargo.toml', 'api/Cargo.lock',
    'api/.cargo/*', 'api/rust-toolchain.toml', 'scripts/chassis.template.yml',
    ...JOB_FILES,
  ],
  db: [
    'ui/src/server/db/*', 'ui/scripts/check-migrations.ts', 'ui/scripts/migrate-file.ts',
    'ui/scripts/check-git-hosts-mirror.sql', 'ui/package.json',
    ...JOB_FILES,
  ],
}

export const SURFACE_NAMES = Object.keys(SURFACES)

/**
 * The api job's plan (planApi below), in the order `ci` writes it:
 *   api_scope         workspace | packages
 *   api_packages      the `-p` list for a packages scope, space-separated
 *   api_lib_packages  the ones with a library (the ignored in-crate tests)
 *   api_it            true when any api/tests/it module runs live
 *   api_it_modules    those modules, space-separated, or `*` for every one
 *   api_live          true when the job needs postgres and redis at all
 *   api_bin           true when the job uploads the talaria-api binary
 */
export const API_KEYS = ['api_scope', 'api_packages', 'api_lib_packages', 'api_it', 'api_it_modules', 'api_live', 'api_bin']

/** Every key `ci` writes to $GITHUB_OUTPUT, always, in this order. */
export const OUTPUT_KEYS = ['mode', 'base', 'range', 'reason', ...SURFACE_NAMES, ...API_KEYS, 'migration_baseline']

export const MODES = ['light', 'integration', 'full']

/** Branch prefixes whose work goes straight into rc: offloaded, they run full. */
export const FULL_LANES = ['hotfix/', 'release/', 'sync/', 'promote/', 'backflow/']

const ZERO_SHA = '0000000000000000000000000000000000000000'

/**
 * A glob with bash `case` semantics: `*` matches any run of characters, `/`
 * included; every other character is literal (`?` and `[` too — no list here
 * uses them, and a literal reading cannot match more than it should).
 * @param {string} glob
 * @param {string} file
 */
export function globMatch(glob, file) {
  const body = glob
    .split('*')
    .map((part) => part.replace(/[.+?^${}()|[\]\\]/g, '\\$&'))
    .join('.*')
  return new RegExp(`^${body}$`, 's').test(file)
}

/**
 * Which CI surfaces a list of changed files moves.
 * @param {string[]} files
 * @returns {Record<string, boolean>}
 */
export function ciSurfaces(files) {
  const out = {}
  for (const [name, globs] of Object.entries(SURFACES)) {
    out[name] = files.some((file) => file && globs.some((glob) => globMatch(glob, file)))
  }
  return out
}

/** @param {boolean} on */
function everySurface(on) {
  return Object.fromEntries(SURFACE_NAMES.map((name) => [name, on]))
}

/**
 * The files the ui suite reads, imports or compiles in, ui/ and apps/
 * included: SURFACES.ui minus the job-definition files (they pick CI jobs, not
 * a local compile), with each `x/*` written as the prefix `x/`. Exact paths
 * match exactly; a trailing `/` is a prefix. Derived, so the local gate and CI
 * cannot disagree about what runs the ui suite.
 */
export const UI_SURFACE = SURFACES.ui
  .filter((glob) => !glob.startsWith('.github/') && glob !== 'scripts/surfaces.mjs')
  .map((glob) => (glob.endsWith('/*') ? glob.slice(0, -1) : glob))

/**
 * The files outside api/ the api job's crates read, in the same prefix form:
 * what a scoped api run has to map to the crates that read them. The
 * migration array is not one of them: no crate reads it, the live steps do
 * (API_DB_INPUTS).
 */
export const API_EXTERNAL_INPUTS = SURFACES.api
  .filter((glob) => !glob.startsWith('api/') && !JOB_FILES.includes(glob) && !SURFACES.db.includes(glob))
  .map((glob) => (glob.endsWith('/*') ? glob.slice(0, -1) : glob))

/**
 * The migration-array files the api job reads: a change to one runs every
 * live module against the schema it builds, and compiles no crate of its own.
 */
export const API_DB_INPUTS = SURFACES.api.filter((glob) => SURFACES.db.includes(glob) && !JOB_FILES.includes(glob))

/**
 * Files that change how EVERY crate builds or what the job is: a light run
 * that touches one tests the workspace.
 */
export const API_WORKSPACE_INPUTS = ['api/.cargo/*', 'api/rust-toolchain.toml', ...JOB_FILES]

// ── The local gate's plan (scripts/gate.mjs) ─────────────────────────────────
//
// One level finer than CI's surfaces: a rust change compiles the packages the
// diff touches, not the workspace.

// A workspace-graph file. Compiling one package does not prove it, and
// compiling the workspace is the pin. CI's api job is the gate for these.
export const API_GRAPH = new Set(['api/Cargo.toml', 'api/Cargo.lock', 'api/rust-toolchain.toml'])

/**
 * The `[package] name` of a Cargo manifest, or null.
 * @param {string} rel  repo-relative path to a Cargo.toml
 * @param {string} [root]
 */
export function readPackageName(rel, root = process.cwd()) {
  const abs = join(root, rel)
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

function isMobileSource(file) {
  if (!file.startsWith('mobile/')) return false
  // The provisioner is a shell script run by hand inside a devbox; it compiles
  // nothing and Gradle never reads it.
  if (file.startsWith('mobile/tools/')) return false
  return /\.(kt|kts|xml|pro|toml|properties)$/.test(file) || file === 'mobile/gradlew'
}

/** @param {string} file */
function onUiSurface(file) {
  return UI_SURFACE.some((p) => (p.endsWith('/') ? file.startsWith(p) : file === p))
}

/** The launcher bundles this one ui file as `@dither` (desktop/vite.config.ts,
 *  tsconfig.json), so the desktop typecheck reads it, as the desktop surface
 *  says. */
const DESKTOP_READS_FROM_UI = 'ui/src/lib/dither-engine.ts'

/**
 * @param {string[]} files
 * @param {(rel: string) => string | null} readPkg
 */
export function classify(files, readPkg) {
  const plan = {
    ui: false,
    mcp: false,
    cli: false,
    apiPackages: [],
    apiGraph: [],
    apiUnmapped: [],
    desktopCargo: false,
    desktopTypecheck: false,
    mobile: false,
  }
  const pkgs = new Set()
  for (const file of files) {
    // A file the ui suite reads from outside ui/ runs it (UI_SURFACE). It is
    // also whatever the chain below makes of it: a scaffolder file is cli
    // source, a Rust file compiles its package.
    if (onUiSurface(file)) plan.ui = true
    if (file === DESKTOP_READS_FROM_UI) plan.desktopTypecheck = true
    if (file.startsWith('ui/') || file.startsWith('apps/')) {
      plan.ui = true
    } else if (file.startsWith('mcp/')) {
      plan.mcp = true
    } else if (file.startsWith('cli/')) {
      plan.cli = true
    } else if (file.startsWith('desktop/')) {
      if (isDesktopRust(file)) plan.desktopCargo = true
      else if (isDesktopUi(file)) plan.desktopTypecheck = true
    } else if (file.startsWith('mobile/')) {
      if (isMobileSource(file)) plan.mobile = true
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

/** The packages this plan runs bun scripts in (typecheck, test), in order. */
export function bunPackages(plan) {
  const pkgs = []
  if (plan.ui) pkgs.push('ui')
  if (plan.mcp) pkgs.push('mcp')
  if (plan.cli) pkgs.push('cli')
  if (plan.desktopTypecheck) pkgs.push('desktop')
  return pkgs
}

/**
 * The ones with NO node_modules at all, which the gate installs before it
 * typechecks them. A fresh `talaria worktree` had no cli/node_modules (and
 * often no mcp/), so the gate's first `tsc` in every new worktree failed on a
 * missing binary — a red gate that said nothing about the change. `bun
 * install --frozen-lockfile` is the branch's own lockfile and bun's hardlink
 * cache: seconds, and no drift. A SYMLINK counts as present, dangling or not:
 * a worktree's ui/node_modules links the primary's, and an install through it
 * would write into another checkout.
 *
 * @param {string[]} pkgs
 * @param {(rel: string) => boolean} lexists
 */
export function missingInstalls(pkgs, lexists) {
  return pkgs.filter((pkg) => !lexists(`${pkg}/node_modules`))
}

/** @param {string[]} installs */
export function installLines(installs) {
  return installs.map((pkg) => `gate: run bun install --frozen-lockfile in ${pkg}/ (no ${pkg}/node_modules)`)
}

// ── Bases and ranges ─────────────────────────────────────────────────────────
//
// `git` everywhere below is a function (args: string[]) => string that returns
// trimmed stdout and THROWS on a non-zero exit. The CLI passes the real one;
// the tests pass a fake.

/** @param {string} s */
function lines(s) {
  return String(s ?? '').split('\n').map((l) => l.trim()).filter(Boolean)
}

/** @param {unknown} e */
function oneLine(e) {
  const text = e instanceof Error ? e.message : String(e)
  return lines(text)[0] ?? 'unknown error'
}

/**
 * The files a range changed. `--no-renames`: a rename shows both paths, so a
 * file moved out of a surface still runs that surface.
 * @param {(args: string[]) => string} git
 * @param {string} range
 */
export function changedFiles(git, range) {
  return lines(git(['diff', '--name-only', '--no-renames', range]))
}

/**
 * The base a working branch diffs against: of the candidates that resolve, the
 * one whose merge-base with HEAD leaves the FEWEST commits on HEAD — the branch
 * it was actually cut from. List order breaks a tie. A stale origin/testing
 * (left behind before the cutover) has a huge count, so rc wins.
 * @param {(args: string[]) => string} git
 * @param {string[]} [candidates]
 * @returns {{ref: string, sha: string, ahead: number} | null}
 */
export function pickBase(git, candidates = ['origin/testing', 'origin/rc']) {
  let best = null
  for (const ref of candidates) {
    let sha
    let ahead
    try {
      git(['rev-parse', '--verify', '--quiet', `${ref}^{commit}`])
      sha = git(['merge-base', 'HEAD', ref])
      ahead = Number.parseInt(git(['rev-list', '--count', `${sha}..HEAD`]), 10)
    } catch {
      continue
    }
    if (!sha || !Number.isFinite(ahead)) continue
    if (best === null || ahead < best.ahead) best = { ref, sha, ahead }
  }
  return best
}

/**
 * The newest first-parent ancestor of HEAD (HEAD itself excluded: its run is
 * this one) that has a SUCCESSFUL push run of ci.yml on `branch`, and whose
 * tree has this file — that run was planned by this logic, so it covered
 * everything since the green run before it. Any error, or no match, is null,
 * and the caller widens.
 * @param {{repo: string, branch: string, token?: string, fetchImpl?: Function, git: (args: string[]) => string}} o
 * @returns {Promise<string | null>}
 */
export async function lastGreen({ repo, branch, token, fetchImpl = globalThis.fetch, git }) {
  try {
    if (!repo || !branch || typeof fetchImpl !== 'function') return null
    const url =
      `https://api.github.com/repos/${repo}/actions/workflows/ci.yml/runs` +
      `?branch=${encodeURIComponent(branch)}&event=push&status=success&per_page=100`
    const headers = {
      Accept: 'application/vnd.github+json',
      'X-GitHub-Api-Version': '2022-11-28',
      'User-Agent': 'talaria-surfaces',
    }
    if (token) headers.Authorization = `Bearer ${token}`
    const init = { headers }
    if (typeof AbortSignal?.timeout === 'function') init.signal = AbortSignal.timeout(20_000)
    const res = await fetchImpl(url, init)
    if (!res || !res.ok) return null
    const body = await res.json()
    const green = new Set((body?.workflow_runs ?? []).map((run) => run?.head_sha).filter(Boolean))
    if (green.size === 0) return null
    for (const sha of lines(git(['rev-list', '--first-parent', '--max-count=500', 'HEAD^']))) {
      if (!green.has(sha)) continue
      try {
        git(['cat-file', '-e', `${sha}:scripts/surfaces.mjs`])
        return sha
      } catch {
        // Green, but planned by the bash that came before this file.
      }
    }
    return null
  } catch {
    return null
  }
}

/** The branch an offload ref carries: `refs/heads/ci/full/x` and `refs/heads/ci/x` are `x`. */
export function offloadBranch(ref) {
  const m = /^refs\/heads\/ci\/(?:full\/)?(.+)$/.exec(ref || '')
  return m ? m[1] : null
}

/** @param {string} branch */
export function needsFull(branch) {
  return FULL_LANES.some((prefix) => String(branch || '').startsWith(prefix))
}

// ── The api job's scope ─────────────────────────────────────────────────────
//
// ci.yml's api job compiles, lints and tests the Rust workspace, and runs the
// live-database suite (the #[ignore]d tests: api/tests/it and the in-crate
// ones) against a scratch postgres and redis. Clippy runs the same in every
// mode, and the root package depends on every crate, so a crate's library that
// no longer compiles against a dependency's change is red in every mode. What
// a light run narrows is the TESTS (and the test code they compile): the
// packages the diff touches, the packages that read a touched file by its path
// (a shared fixture, another crate's source), and the live modules that reach
// them.
// Integration (testing) and full (rc, main, a release) test the workspace and
// run every live module, so the light run's narrower answer is checked again
// before anything ships.
//
// A light plan widens to the workspace on anything it cannot read: a
// toolchain, cargo config or job-definition change, a lockfile entry with a
// `source` (a registry or git crate moved), a manifest line outside the
// workspace's own wiring, an input outside api/ no crate names, a crate file
// that maps to no manifest.
//
// The api tree is read from `root` (the checkout); the BASE side of the
// lockfile and the root manifest comes from git.

/** @type {Map<string, string | null>} */
const textCache = new Map()

/** A repo-relative file's text, or null. Memoized: one plan reads a file once. */
function readText(root, rel) {
  const key = `${root}\u0000${rel}`
  if (!textCache.has(key)) {
    let text = null
    try {
      text = readFileSync(join(root, rel), 'utf8')
    } catch {
      text = null
    }
    textCache.set(key, text)
  }
  return textCache.get(key)
}

/** @type {Map<string, string[]>} */
const rustCache = new Map()

/** Every .rs file under a repo-relative path, sorted; target/ and dot dirs skipped. */
function rustFiles(root, dir) {
  const key = `${root}\u0000${dir}`
  if (rustCache.has(key)) return rustCache.get(key)
  const out = []
  const walk = (rel) => {
    let entries
    try {
      entries = readdirSync(join(root, rel), { withFileTypes: true })
    } catch {
      return
    }
    for (const entry of entries) {
      if (entry.name.startsWith('.') || entry.name === 'target' || entry.name === 'node_modules') continue
      const child = `${rel}/${entry.name}`
      if (entry.isDirectory()) walk(child)
      else if (entry.isFile() && entry.name.endsWith('.rs')) out.push(child)
    }
  }
  walk(dir)
  out.sort()
  rustCache.set(key, out)
  return out
}

/** @type {Map<string, {root: string, dirs: Map<string, string>, byIdent: Map<string, string>}>} */
const layoutCache = new Map()

/**
 * The api workspace's packages: name → repo-relative dir, and the same keyed
 * by the crate's Rust identifier. The root package is `api`; the members are
 * the manifests under api/crates/ (the workspace's `members = ["crates/*"]`).
 * @param {string} [root]
 */
export function apiLayout(root = process.cwd()) {
  if (layoutCache.has(root)) return layoutCache.get(root)
  const dirs = new Map()
  const rootName = readPackageName('api/Cargo.toml', root) ?? 'talaria-api'
  dirs.set(rootName, 'api')
  let entries = []
  try {
    entries = readdirSync(join(root, 'api/crates'), { withFileTypes: true })
  } catch {
    entries = []
  }
  for (const entry of entries) {
    if (!entry.isDirectory()) continue
    const name = readPackageName(`api/crates/${entry.name}/Cargo.toml`, root)
    if (name) dirs.set(name, `api/crates/${entry.name}`)
  }
  const byIdent = new Map([...dirs.keys()].map((name) => [ident(name), name]))
  const layout = { root: rootName, dirs, byIdent }
  layoutCache.set(root, layout)
  return layout
}

/** A package name as Rust spells its crate: `talaria-runs-store` → `talaria_runs_store`. */
function ident(name) {
  return name.replace(/-/g, '_')
}

/** @param {string} s */
function canonLines(s) {
  return String(s ?? '')
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line && !line.startsWith('#'))
    .join('\n')
}

/** api/Cargo.lock as {rest, blocks}: each [[package]] keyed by name, version and source. */
function lockBlocks(text) {
  const parts = String(text ?? '').replace(/\r\n/g, '\n').split(/^\[\[package\]\][ \t]*$/m)
  const rest = [parts[0]]
  const blocks = new Map()
  for (const part of parts.slice(1)) {
    // Another table ([metadata], [[patch.unused]]) ends the block.
    const cut = part.search(/^\[/m)
    const body = cut === -1 ? part : part.slice(0, cut)
    if (cut !== -1) rest.push(part.slice(cut))
    const field = (key) => new RegExp(`^${key}\\s*=\\s*"([^"]*)"`, 'm').exec(body)?.[1] ?? ''
    const block = { name: field('name'), version: field('version'), source: field('source'), text: canonLines(body) }
    blocks.set(`${block.name} ${block.version} ${block.source}`, block)
  }
  return { rest: canonLines(rest.join('\n')), blocks }
}

/**
 * What an api/Cargo.lock edit changes. A [[package]] block with a `source` is
 * a registry or git crate: adding, removing or changing one (its version, its
 * checksum, its dependency list) can change how every crate builds, so the
 * answer is the workspace. A block with no source is a workspace member: an
 * added one, or one whose dependency list changed, is the package to test. A
 * removed member leaves nothing to test; the members that named it changed
 * their lists, so they are in.
 *
 * Most lockfile edits are the second kind (a new crate, a new path
 * dependency): 48 of rc's 95 api merges from Oct 20 to Nov 10 touched the lock,
 * and none of those lock diffs had a `source =` line.
 *
 * @param {string} baseText
 * @param {string} headText
 * @returns {{workspace: true, why: string} | {workspace: false, packages: string[]}}
 */
export function lockDelta(baseText, headText) {
  const base = lockBlocks(baseText)
  const head = lockBlocks(headText)
  if (base.rest !== head.rest) return { workspace: true, why: 'the lockfile changed outside its [[package]] blocks' }
  const packages = new Set()
  for (const key of new Set([...base.blocks.keys(), ...head.blocks.keys()])) {
    const before = base.blocks.get(key)
    const after = head.blocks.get(key)
    if (before && after && before.text === after.text) continue
    const source = before?.source || after?.source
    if (source) {
      const block = after ?? before
      return { workspace: true, why: `${block.name} ${block.version} (${source}) changed in the lockfile` }
    }
    if (after) packages.add(after.name)
  }
  return { workspace: false, packages: [...packages].sort() }
}

/** `line` with a trailing `#` comment cut, quotes respected. */
function stripTomlComment(line) {
  let quote = ''
  for (let i = 0; i < line.length; i++) {
    const c = line[i]
    if (quote) {
      if (c === '\\' && quote === '"') i++
      else if (c === quote) quote = ''
    } else if (c === '"' || c === "'") {
      quote = c
    } else if (c === '#') {
      return line.slice(0, i)
    }
  }
  return line
}

/** Open brackets minus closed ones, outside strings: > 0 while a value continues. */
function tomlDepth(s) {
  let depth = 0
  let quote = ''
  for (let i = 0; i < s.length; i++) {
    const c = s[i]
    if (quote) {
      if (c === '\\' && quote === '"') i++
      else if (c === quote) quote = ''
    } else if (c === '"' || c === "'") {
      quote = c
    } else if (c === '[' || c === '{') {
      depth++
    } else if (c === ']' || c === '}') {
      depth--
    }
  }
  return depth
}

/**
 * A Cargo manifest's entries: `section\0key` → {section, key, value}, a
 * multi-line array or inline table read as one value with its whitespace
 * collapsed, comments and blank lines dropped (cargo reads neither). The n-th
 * `[[bin]]` is the section `bin#n`. Throws on what it does not read: a
 * multi-line string, a line that is neither a table nor a key, a value that
 * never closes.
 */
function manifestEntries(text) {
  const out = new Map()
  const counts = new Map()
  let section = ''
  let cur = null
  const put = (entry) => {
    out.set(`${entry.section}\u0000${entry.key}`, { section: entry.section, key: entry.key, value: entry.value.replace(/\s+/g, ' ').trim() })
  }
  for (const raw of String(text ?? '').replace(/\r\n/g, '\n').split('\n')) {
    if (raw.includes('"""') || raw.includes("'''")) throw new Error('a multi-line string')
    const line = stripTomlComment(raw).trim()
    if (cur) {
      if (line) {
        cur.value += ` ${line}`
        cur.depth += tomlDepth(line)
      }
      if (cur.depth <= 0) {
        put(cur)
        cur = null
      }
      continue
    }
    if (!line) continue
    const array = /^\[\[\s*([^\]]+?)\s*\]\]$/.exec(line)
    if (array) {
      const n = counts.get(array[1]) ?? 0
      counts.set(array[1], n + 1)
      section = `${array[1]}#${n}`
      continue
    }
    const table = /^\[\s*([^\]]+?)\s*\]$/.exec(line)
    if (table) {
      section = table[1].replace(/\s+/g, '')
      continue
    }
    const eq = line.indexOf('=')
    if (eq <= 0) throw new Error(`a line that is neither a table nor a key: ${line}`)
    cur = { section, key: line.slice(0, eq).replace(/["'\s]/g, ''), value: line.slice(eq + 1), depth: tomlDepth(line.slice(eq + 1)) }
    if (cur.depth <= 0) {
      put(cur)
      cur = null
    }
  }
  if (cur) throw new Error(`the value of ${cur.key} never closes`)
  return out
}

/** The quoted strings of a TOML array value. */
function tomlStrings(value) {
  return new Set([...String(value ?? '').matchAll(/"([^"]*)"|'([^']*)'/g)].map((m) => m[1] ?? m[2]))
}

/**
 * What an api/Cargo.toml edit changes, entry by entry (the workspace root
 * manifest; a member's own manifest is that member's file). Every changed
 * entry, on either side, is classified by its section:
 *   [workspace] members      the package each added member dir names
 *   [workspace.dependencies] an entry with `path =`: that crate
 *   [dependencies], [dev-dependencies], [build-dependencies] (and their
 *     target.… forms) an entry with `path =` or `workspace = true`, and
 *     [package] and the root's target tables: the root package
 *   anything else — a registry entry, a `features` list, the profiles, the
 *     lints, [workspace.package], [patch]: the workspace.
 * A comment or a blank line is no change: cargo reads neither.
 *
 * @param {string} baseText
 * @param {string} headText
 * @param {{root?: string, rootPackage?: string}} [opts]
 * @returns {{workspace: true, why: string} | {workspace: false, packages: string[]}}
 */
export function manifestDelta(baseText, headText, { root = process.cwd(), rootPackage = 'talaria-api' } = {}) {
  const wide = (why) => ({ workspace: true, why })
  let before
  let after
  try {
    before = manifestEntries(baseText)
    after = manifestEntries(headText)
  } catch (e) {
    return wide(`api/Cargo.toml could not be read entry by entry (${oneLine(e)})`)
  }
  const packages = new Set()
  for (const key of new Set([...before.keys(), ...after.keys()])) {
    const a = before.get(key)
    const b = after.get(key)
    if (a && b && a.value === b.value) continue
    const { section, key: name } = a ?? b
    const values = [a?.value, b?.value].filter((v) => v !== undefined)
    const has = (re) => values.some((v) => re.test(v))
    const where = `[${section || 'top level'}] ${name}`
    const features = has(/\b(?:default-)?features\s*=/)
    if (section === 'workspace' && name === 'members') {
      const was = tomlStrings(a?.value)
      const now = tomlStrings(b?.value)
      for (const member of was) if (!now.has(member)) return wide(`${member} left the workspace`)
      for (const member of now) {
        if (was.has(member)) continue
        if (/[*?[]/.test(member)) return wide(`the workspace members gained the glob ${member}`)
        const pkg = readPackageName(`api/${member}/Cargo.toml`, root)
        if (!pkg) return wide(`the new member ${member} has no manifest`)
        packages.add(pkg)
      }
    } else if (section === 'workspace.dependencies') {
      if (!has(/\bpath\s*=/) || features) return wide(`${where} is not a plain path entry`)
      const renamed = values.map((v) => /\bpackage\s*=\s*"([^"]+)"/.exec(v)?.[1]).find(Boolean)
      packages.add(renamed ?? name)
    } else if (/^(?:target\..+\.)?(?:dev-|build-)?dependencies$/.test(section)) {
      const internal = has(/\bpath\s*=/) || has(/\bworkspace\s*=\s*true\b/) || (/\.workspace$/.test(name) && has(/^true$/))
      if (!internal || features) return wide(`${where} is not a plain workspace or path entry`)
      packages.add(rootPackage)
    } else if (section === 'package' || /^(?:lib|bin|test|bench|example)(?:#\d+)?$/.test(section)) {
      packages.add(rootPackage)
    } else {
      return wide(`${where} changes how the workspace builds`)
    }
  }
  return { workspace: false, packages: [...packages].sort() }
}

/**
 * The leaves of the Rust use tree (or path) that starts at text[at], each
 * with its `as` alias and whether it was a glob: `a::{b, c::d as e, f::*}`
 * is [a b], [a c d] as e, and [a f] (glob). Parsing stops at anything a path
 * cannot hold, so `talaria_api::runs::store::claim(&pool)` is
 * [runs store claim].
 */
function useLeaves(text, at) {
  let i = at
  const leaves = []
  const ws = () => {
    while (i < text.length && /\s/.test(text[i])) i++
  }
  const name = () => {
    const m = /^(?:r#)?[A-Za-z_][A-Za-z0-9_]*/.exec(text.slice(i, i + 128))
    if (!m) return null
    i += m[0].length
    return m[0].replace(/^r#/, '')
  }
  const tree = (prefix, depth) => {
    const segs = [...prefix]
    for (;;) {
      ws()
      if (text[i] === '{') {
        i++
        if (depth > 8) return
        for (;;) {
          ws()
          if (i >= text.length || text[i] === '}') {
            i++
            return
          }
          const from = i
          tree(segs, depth + 1)
          ws()
          if (text[i] === ',') {
            i++
            continue
          }
          if (text[i] === '}') {
            i++
            return
          }
          if (i === from) return
          return
        }
      }
      if (text[i] === '*') {
        i++
        leaves.push({ segs, alias: null, glob: true })
        return
      }
      const id = name()
      if (id === null) {
        if (segs.length) leaves.push({ segs, alias: null, glob: false })
        return
      }
      if (id !== 'self') segs.push(id)
      ws()
      if (text.startsWith('::', i)) {
        i += 2
        continue
      }
      let alias = null
      if (/^as\b/.test(text.slice(i, i + 3))) {
        i += 2
        ws()
        alias = name()
      }
      leaves.push({ segs, alias, glob: false })
      return
    }
  }
  tree([], 0)
  return leaves
}

/** a is b, or b's ancestor or descendant: either can reach the other. */
function related(a, b) {
  const n = Math.min(a.length, b.length)
  for (let i = 0; i < n; i++) if (a[i] !== b[i]) return false
  return true
}

/** The module path a source file declares, from its place under src/. */
function modulePath(srcRel) {
  const parts = srcRel.replace(/\.rs$/, '').split('/')
  const last = parts.at(-1)
  if (parts.length === 1 && (last === 'lib' || last === 'main')) return []
  if (last === 'mod') parts.pop()
  return parts
}

/** @type {Map<string, Array<{path: string[], target: string, targetPath: string[]}>>} */
const exportCache = new Map()

/**
 * The workspace crates a crate re-exports, with where: each `pub use
 * talaria_x…` in its library source is {path inside this crate, the target
 * crate, the path inside the target}. `pub use talaria_runs_store as store`
 * in runs/mod.rs is [runs store] → talaria_runs_store []; `pub use
 * talaria_kb::*` in kb/mod.rs is [kb] → talaria_kb [].
 */
function reexports(crate, root) {
  const key = `${root}\u0000${crate}`
  if (exportCache.has(key)) return exportCache.get(key)
  const layout = apiLayout(root)
  const name = layout.byIdent.get(crate)
  const out = []
  if (name) {
    const src = `${layout.dirs.get(name)}/src`
    for (const rel of rustFiles(root, src)) {
      const inSrc = rel.slice(src.length + 1)
      // The binary (main.rs, src/bin/) is no part of the library's paths.
      if (inSrc === 'main.rs' || inSrc.startsWith('bin/')) continue
      const text = readText(root, rel) ?? ''
      const at = modulePath(inSrc)
      for (const m of text.matchAll(/\bpub\s+use\s+/g)) {
        for (const leaf of useLeaves(text, m.index + m[0].length)) {
          const [target, ...targetPath] = leaf.segs
          if (!target || !layout.byIdent.has(target)) continue
          const path = leaf.glob ? at : [...at, leaf.alias ?? leaf.segs.at(-1)]
          out.push({ path, target, targetPath })
        }
      }
    }
  }
  exportCache.set(key, out)
  return out
}

/**
 * The paths through which a crate is reachable from the root package's
 * library, and from every crate on the way: Map<crate, paths inside it>.
 * talaria_runs_store is talaria_api::runs::store, because the root re-exports
 * talaria_api_facades::runs and the facades' runs/mod.rs re-exports the store
 * crate as `store`.
 */
function reachingPaths(target, root) {
  const layout = apiLayout(root)
  const memo = new Map()
  const visit = (crate, depth) => {
    if (memo.has(crate)) return memo.get(crate)
    memo.set(crate, [])
    const paths = []
    if (depth <= 4) {
      for (const e of reexports(crate, root)) {
        if (e.target === target) {
          paths.push(e.path)
          continue
        }
        for (const inner of visit(e.target, depth + 1)) {
          if (related(inner, e.targetPath)) {
            paths.push(inner.length >= e.targetPath.length ? [...e.path, ...inner.slice(e.targetPath.length)] : e.path)
          }
        }
      }
    }
    memo.set(crate, paths)
    return paths
  }
  visit(ident(layout.root), 0)
  return new Map([...memo].filter(([, paths]) => paths.length))
}

/** Does `text` reach crate `target`: by name, or by a path `reach` says leads to it? */
function reaches(text, target, reach) {
  if (new RegExp(`\\b${target}\\b`).test(text)) return true
  for (const [via, paths] of reach) {
    for (const m of text.matchAll(new RegExp(`\\b${via}::`, 'g'))) {
      for (const leaf of useLeaves(text, m.index + m[0].length)) {
        if (paths.some((path) => related(leaf.segs, path))) return true
      }
    }
  }
  return false
}

/**
 * The api/tests/it modules that reach any of `packages`: the stems of the
 * files that name a package's crate, or use a talaria_api path that leads to
 * it (a re-export in api/src or in a crate it re-exports, such as
 * talaria-api-facades), or an ancestor of one. The root package is every
 * module (['*']), and so is a crate the shared support module reaches: every
 * module runs through it.
 *
 * @param {string[]} packages
 * @param {string} [root]
 * @returns {string[]}
 */
export function itModulesFor(packages, root = process.cwd()) {
  const layout = apiLayout(root)
  if (packages.includes(layout.root)) return ['*']
  const tests = rustFiles(root, 'api/tests/it')
    .filter((rel) => /^api\/tests\/it\/[A-Za-z0-9_]+\.rs$/.test(rel) && rel !== 'api/tests/it/main.rs')
    .map((rel) => ({ stem: rel.slice('api/tests/it/'.length, -3), text: readText(root, rel) ?? '' }))
  const support = rustFiles(root, 'api/tests/it/support').map((rel) => readText(root, rel) ?? '').join('\n')
  const out = new Set()
  for (const pkg of packages) {
    const target = ident(pkg)
    const reach = reachingPaths(target, root)
    if (reaches(support, target, reach)) return ['*']
    for (const { stem, text } of tests) if (reaches(text, target, reach)) out.add(stem)
  }
  return [...out].sort()
}

/** The Rust files a package compiles: its dir, or for the root api/src, api/tests and api/build.rs. */
function packageRustFiles(name, root) {
  const layout = apiLayout(root)
  const dir = layout.dirs.get(name)
  if (!dir) return []
  if (dir !== 'api') return rustFiles(root, dir)
  return [...rustFiles(root, 'api/src'), ...rustFiles(root, 'api/tests'), ...(existsSync(join(root, 'api/build.rs')) ? ['api/build.rs'] : [])]
}

/**
 * The packages whose Rust sources name `needle` (a file outside api/ that
 * they include_str!, read at test time, or hold a table to).
 * @param {string} needle
 * @param {string} [root]
 */
export function packagesReading(needle, root = process.cwd()) {
  const out = []
  for (const name of apiLayout(root).dirs.keys()) {
    if (packageRustFiles(name, root).some((rel) => (readText(root, rel) ?? '').includes(needle))) out.push(name)
  }
  return out.sort()
}

/** Has a library target: src/lib.rs, or a [lib] table. */
function hasLib(name, root) {
  const dir = apiLayout(root).dirs.get(name)
  if (!dir) return false
  return existsSync(join(root, dir, 'src/lib.rs')) || /^\[lib\]/m.test(readText(root, `${dir}/Cargo.toml`) ?? '')
}

/** Has an #[ignore]d test in its library source: the live in-crate tests. */
function hasIgnoredTests(name, root) {
  const dir = apiLayout(root).dirs.get(name)
  if (!dir) return false
  return rustFiles(root, `${dir}/src`).some((rel) => (readText(root, rel) ?? '').includes('#[ignore'))
}

/**
 * The api job's test plan for one run.
 *   workspace  every package's tests, every ignored in-crate test, every live
 *              module: integration, full, or a light run the plan could not
 *              narrow.
 *   packages   light: `cargo test -p` for each of `packages`; the ignored
 *              in-crate tests of `libPackages`; `itModules` from
 *              api/tests/it. An empty plan tests nothing — the api surface
 *              did not move, or only a db file did and the live modules are
 *              the whole test.
 * `live` says whether postgres and redis are needed at all.
 *
 * @param {{mode: string, all: boolean, files: string[], root?: string,
 *   baseText: (rel: string) => string, headText?: (rel: string) => string}} o
 *   baseText reads a file at the diff's base and headText in the checkout;
 *   each throws when it cannot
 */
export function planApi({ mode, all, files, root = process.cwd(), baseText, headText = (rel) => readFileSync(join(root, rel), 'utf8') }) {
  const wide = (why) => ({ scope: 'workspace', packages: [], libPackages: [], itModules: ['*'], live: true, why })
  if (mode !== 'light') return wide(`${mode || 'an unknown'} mode tests the workspace`)
  if (all) return wide('every surface runs, so the workspace is tested')
  const apiFiles = (files ?? []).filter((file) => SURFACES.api.some((glob) => globMatch(glob, file)))
  if (apiFiles.length === 0) {
    return { scope: 'packages', packages: [], libPackages: [], itModules: [], live: false, why: 'the api surface did not move' }
  }
  const widener = apiFiles.find((file) => API_WORKSPACE_INPUTS.some((glob) => globMatch(glob, file)))
  if (widener) return wide(`${widener} changes how every crate builds or what the job is`)

  const layout = apiLayout(root)
  const pkgs = new Set()
  const itStems = new Set()
  let everyModule = ''
  let rust = false
  for (const file of apiFiles) {
    if (API_DB_INPUTS.some((glob) => globMatch(glob, file))) {
      everyModule ||= `${file} is the schema every live module runs on`
      continue
    }
    if (file.startsWith('api/')) rust = true
    if (file.startsWith('api/crates/')) {
      // Any file in a crate's dir, not only .rs: include_str! reads the rest.
      const dir = file.split('/')[2]
      const name = dir ? readPackageName(`api/crates/${dir}/Cargo.toml`, root) : null
      if (!name) return wide(`${file} maps to no package`)
      pkgs.add(name)
      // Another package that reads this file by its path tests too:
      // talaria-harness-defs holds a table to talaria-fitness-toolbox's
      // sandbox.rs (`include_str!("../../talaria-fitness-toolbox/src/…")`).
      for (const reader of packagesReading(file.slice('api/crates/'.length), root)) pkgs.add(reader)
    } else if (file.startsWith('api/tests/')) {
      // One live module's own file runs that module; the shared support,
      // the module list (main.rs) and the fixtures run them all.
      const m = /^api\/tests\/it\/([A-Za-z0-9_]+)\.rs$/.exec(file)
      if (m && m[1] !== 'main') itStems.add(m[1])
      else everyModule ||= `${file} is shared by every live module`
      // A crate whose own tests read the file tests too: the shared fixtures
      // (app-grants.json, collation.json, the github pem…) are include_str!'d
      // by crates as `../../../tests/it/fixtures/…`. The root package is the
      // module rule above.
      for (const reader of packagesReading(file.slice('api/'.length), root)) {
        if (reader !== layout.root) pkgs.add(reader)
      }
    } else if (file.startsWith('api/src/') || file === 'api/build.rs') {
      pkgs.add(layout.root)
    } else if (file === 'api/Cargo.lock' || file === 'api/Cargo.toml') {
      let delta
      try {
        const head = headText(file)
        delta = file === 'api/Cargo.lock' ? lockDelta(baseText(file), head) : manifestDelta(baseText(file), head, { root, rootPackage: layout.root })
      } catch (e) {
        return wide(`${file} could not be compared with the base (${oneLine(e)})`)
      }
      if (delta.workspace) return wide(`${file}: ${delta.why}`)
      for (const name of delta.packages) {
        if (!layout.dirs.has(name)) return wide(`${file} names ${name}, which is no package here`)
        pkgs.add(name)
      }
    } else if (!file.startsWith('api/')) {
      const input = API_EXTERNAL_INPUTS.find((p) => (p.endsWith('/') ? file.startsWith(p) : file === p))
      if (!input) return wide(`${file} is on the api surface with no rule for it`)
      const needle = input.replace(/\/$/, '')
      const readers = packagesReading(needle, root)
      if (readers.length === 0) return wide(`no api source names ${needle}, so what reads ${file} is unknown`)
      for (const name of readers) pkgs.add(name)
    }
    // Anything else under api/ (the package Dockerfile, a readme) compiles
    // into nothing a test runs.
  }

  let itModules
  if (everyModule || pkgs.has(layout.root)) {
    itModules = ['*']
  } else {
    // `*` already holds every stem: a list beside it (`* task_move_live`)
    // is no plan ci.yml's normalize step accepts, so it would widen the run.
    const reached = itModulesFor([...pkgs], root)
    itModules = reached.includes('*') ? ['*'] : [...new Set([...reached, ...itStems])].sort()
  }
  // The live modules are one test binary in the root package, so a Rust
  // change that runs any of them tests that package too. A schema change
  // alone compiles nothing new: the live modules are its whole test.
  if (itModules.length && (rust || pkgs.size)) pkgs.add(layout.root)
  const packages = [...pkgs].sort()
  const libPackages = packages.filter((name) => hasLib(name, root))
  const live = itModules.length > 0 || libPackages.some((name) => hasIgnoredTests(name, root))
  const why = everyModule || `${packages.length} package(s) the diff touches`
  return { scope: 'packages', packages, libPackages, itModules, live, why }
}

/**
 * Does this run upload the talaria-api binary (`talaria dev --api-from-ci`)?
 * A pull request from this repository, a push to testing, and an offload run:
 * the runs whose binary someone exercises. Never a fork's pull request.
 * @param {{event?: string, ref?: string, repo?: string, headRepo?: string, calledRef?: string, calledMode?: string}} ctx
 */
export function binQualifies(ctx) {
  if (ctx.calledRef) return (ctx.calledMode === 'light' || ctx.calledMode === 'full') && offloadBranch(ctx.ref) !== null
  if (ctx.event === 'pull_request') return Boolean(ctx.headRepo) && ctx.headRepo === ctx.repo
  if (ctx.event === 'push') return ctx.ref === 'refs/heads/testing'
  return false
}

/**
 * The array a change's migrations must extend (ci.yml's `migrations` job
 * replays it, then this tree's array on top): the branch the change lands on.
 *   pull request   origin/<its base>: testing for a feature, rc for a
 *                  promotion or a hotfix.
 *   push testing   origin/rc;  push rc  origin/main;  push main  origin/main.
 *   offload        the nearer of origin/testing and origin/rc (pickBase).
 *   a release call, or anything else: origin/main.
 * @param {{event?: string, ref?: string, baseRef?: string, calledRef?: string, calledMode?: string}} ctx
 * @param {(args: string[]) => string} git
 */
export function migrationBaseline(ctx, git) {
  const safe = (branch) => /^[A-Za-z0-9._/-]+$/.test(branch) && !branch.includes('..')
  if (ctx.calledRef) {
    if (MODES.includes(ctx.calledMode) && offloadBranch(ctx.ref) !== null) return pickBase(git)?.ref ?? 'origin/main'
    return 'origin/main'
  }
  if (ctx.event === 'pull_request') return ctx.baseRef && safe(ctx.baseRef) ? `origin/${ctx.baseRef}` : 'origin/main'
  if (ctx.event === 'push') {
    if (ctx.ref === 'refs/heads/testing') return 'origin/rc'
    if (ctx.ref === 'refs/heads/rc' || ctx.ref === 'refs/heads/main') return 'origin/main'
    return pickBase(git)?.ref ?? 'origin/main'
  }
  return 'origin/main'
}

/** One line for the log and the summary. */
export function describeApi(api) {
  if (api.scope === 'workspace') return `the workspace, every live module (${api.why})`
  if (!api.packages.length && !api.itModules.length) return `nothing (${api.why})`
  const tests = api.packages.length ? api.packages.map((p) => `-p ${p}`).join(' ') : 'no package'
  const modules = api.itModules.length ? `live modules ${api.itModules.join(' ')}` : 'no live module'
  return `${tests}; ${modules}; ${api.live ? 'postgres + redis' : 'no database'} (${api.why})`
}

/**
 * The CI plan for one run.
 *
 * ctx: {event, ref, baseRef, headRef, headRepo, calledRef, calledMode,
 * before, baseSha, deleted, repo, token, root} — ci.yml's env, see the `ci`
 * CLI below; root is the checkout (default: the working directory).
 * fetchImpl: a fetch-compatible function for lastGreen (globalThis.fetch in CI).
 *
 * @returns {Promise<{mode: string, base: string, range: string, all: boolean,
 *   reason: string, surfaces: Record<string, boolean>, files: string[],
 *   api: {scope: string, packages: string[], libPackages: string[],
 *   itModules: string[], live: boolean, bin: boolean, why: string},
 *   migrationBaseline: string}>}
 */
export async function planCi(ctx, git, fetchImpl = globalThis.fetch) {
  if (String(ctx.deleted) === 'true') {
    return {
      mode: 'none', base: '', range: '', all: false,
      reason: 'the push deleted the branch: nothing to check',
      surfaces: everySurface(false), files: [],
      api: { scope: 'packages', packages: [], libPackages: [], itModules: [], live: false, bin: false, why: 'nothing to check' },
      migrationBaseline: 'origin/main',
    }
  }

  let mode = ''
  let all = false
  let base = ''
  let range = ''
  let reason = ''
  const since = (sha, why) => {
    base = sha
    range = `${sha}..HEAD`
    reason = why
  }
  const widen = (why) => {
    all = true
    reason = why
  }
  const green = (branch) => lastGreen({ repo: ctx.repo, branch, token: ctx.token, fetchImpl, git })

  try {
    if (ctx.calledRef) {
      const called = ctx.calledMode || ''
      if (called === '') {
        mode = 'full'
        widen('called with a ref and no mode (release.yml): every surface, full')
      } else if (!MODES.includes(called)) {
        mode = 'full'
        widen(`called with unknown mode '${called}': every surface, full`)
      } else {
        mode = called
        let lane = ''
        const branch = offloadBranch(ctx.ref)
        if (branch !== null && needsFull(branch) && mode !== 'full') {
          mode = 'full'
          lane = `${branch} goes straight into rc, so its offload runs full; `
        }
        if (mode === 'light') {
          const picked = pickBase(git)
          if (picked) {
            since(picked.sha, `light call: since the merge-base with ${picked.ref} (${picked.ahead} commit(s) ahead)`)
          } else {
            widen('light call: neither origin/testing nor origin/rc resolves, so every surface runs')
          }
        } else {
          widen(`${lane}called ${mode}: every surface`)
        }
      }
    } else if (ctx.event === 'pull_request') {
      const into = ctx.baseRef || '(unknown base)'
      mode = ctx.baseRef === 'rc' || ctx.baseRef === 'main' ? 'full' : 'light'
      let merge = false
      try {
        git(['rev-parse', '--verify', '--quiet', 'HEAD^2'])
        merge = true
      } catch {
        merge = false
      }
      if (merge) {
        base = git(['rev-parse', 'HEAD^1'])
        range = 'HEAD^1..HEAD'
        reason = `pull request into ${into}: the merge ref against the base tip it was built on`
      } else if (ctx.baseSha) {
        base = ctx.baseSha
        range = `${ctx.baseSha}...HEAD`
        reason = `pull request into ${into}: no merge ref, so the PR's own changes since the base sha`
      } else {
        widen(`pull request into ${into} with no merge ref and no base sha: every surface runs`)
      }
    } else if (ctx.event === 'push') {
      const branch = (ctx.ref || '').startsWith('refs/heads/') ? ctx.ref.slice('refs/heads/'.length) : ''
      if (branch === 'main') {
        mode = 'full'
        const sha = await green('main')
        if (sha) since(sha, "push to main: everything since main's last green CI run")
        else if (ctx.before && ctx.before !== ZERO_SHA) since(ctx.before, 'push to main: no green run found, so since the push\'s before sha')
        else widen('push to main with no green run and no before sha: every surface runs')
      } else if (branch === 'rc') {
        mode = 'full'
        const sha = await green('rc')
        if (sha) since(sha, "push to rc: everything since rc's last green CI run")
        else since(git(['merge-base', 'origin/main', 'HEAD']), 'push to rc: no green run found, so since the merge-base with origin/main')
      } else if (branch === 'testing') {
        mode = 'integration'
        const sha = await green('testing')
        if (sha) since(sha, "push to testing: everything since testing's last green CI run")
        else since(git(['merge-base', 'origin/rc', 'HEAD']), 'push to testing: no green run found, so since the merge-base with origin/rc')
      } else {
        mode = 'light'
        const picked = pickBase(git)
        const what = branch || ctx.ref || '(unknown ref)'
        if (picked) since(picked.sha, `push to ${what}: since the merge-base with ${picked.ref} (${picked.ahead} commit(s) ahead)`)
        else widen(`push to ${what}: neither origin/testing nor origin/rc resolves, so every surface runs`)
      }
    } else {
      mode = 'full'
      widen(`event '${ctx.event || ''}' has no rule: every surface, full`)
    }
  } catch (e) {
    if (!mode) mode = 'full'
    widen(`git could not compute a range (${oneLine(e)}): every surface runs`)
  }

  let files = []
  if (!all) {
    try {
      files = changedFiles(git, range)
    } catch (e) {
      widen(`${reason}; but git could not diff ${range} (${oneLine(e)}), so every surface runs`)
    }
  }

  // The lockfile and the root manifest are compared with the diff's base: the
  // merge-base for a three-dot range, the base sha otherwise.
  let diffBase = null
  const baseText = (rel) => {
    diffBase ??= range.includes('...') ? git(['merge-base', base, 'HEAD']) : base
    return git(['show', `${diffBase}:${rel}`])
  }
  const root = ctx.root || process.cwd()
  const surfaces = all ? everySurface(true) : ciSurfaces(files)
  const api = planApi({ mode, all, files, root, baseText })
  // The binary is the root package's: built when its tests are.
  const bin = surfaces.api && (api.scope === 'workspace' || api.packages.includes(apiLayout(root).root)) && binQualifies(ctx)
  return {
    mode, base, range, all, reason, surfaces, files,
    api: { ...api, bin },
    migrationBaseline: migrationBaseline(ctx, git),
  }
}

// ── The CLI ──────────────────────────────────────────────────────────────────

/** @param {string} value  one $GITHUB_OUTPUT line can carry no newline */
function flat(value) {
  return String(value ?? '').replace(/[\r\n]+/g, ' ').trim()
}

/** @param {string} value */
function cell(value) {
  return flat(value).replace(/\|/g, '\\|')
}

/**
 * `ci`, minus the process: env in; the $GITHUB_OUTPUT lines, the log and the
 * step summary out. EVERY key in OUTPUT_KEYS is emitted, whatever happened.
 * @param {Record<string, string | undefined>} env
 * @param {(args: string[]) => string} git
 * @param {Function} [fetchImpl]
 */
export async function ciOutputs(env, git, fetchImpl = globalThis.fetch) {
  const ctx = {
    event: env.EVENT || '',
    ref: env.REF || '',
    baseRef: env.BASE_REF || '',
    headRef: env.HEAD_REF || '',
    headRepo: env.HEAD_REPO || '',
    calledRef: env.CALLED_REF || '',
    calledMode: env.CALLED_MODE || '',
    before: env.BEFORE || '',
    baseSha: env.BASE_SHA || '',
    deleted: env.DELETED || '',
    repo: env.REPO || '',
    token: env.GH_TOKEN || env.GITHUB_TOKEN || '',
  }
  const plan = await planCi(ctx, git, fetchImpl)
  const values = {
    mode: plan.mode || 'full',
    base: plan.base,
    range: plan.range,
    reason: plan.reason,
    // Anything but an explicit false runs: the fail-closed reading.
    ...Object.fromEntries(SURFACE_NAMES.map((name) => [name, plan.surfaces[name] === false ? 'false' : 'true'])),
    api_scope: plan.api.scope,
    api_packages: plan.api.packages.join(' '),
    api_lib_packages: plan.api.libPackages.join(' '),
    api_it: plan.api.itModules.length ? 'true' : 'false',
    api_it_modules: plan.api.itModules.join(' '),
    api_live: plan.api.live ? 'true' : 'false',
    api_bin: plan.api.bin ? 'true' : 'false',
    migration_baseline: plan.migrationBaseline,
  }
  const output = OUTPUT_KEYS.map((key) => `${key}=${flat(values[key])}`)

  const on = SURFACE_NAMES.filter((name) => values[name] === 'true')
  const log = [
    `plan: mode=${values.mode} range=${values.range || '(none)'}${plan.all ? ' (every surface)' : ''}`,
    `plan: ${values.reason}`,
    `changed files: ${plan.files.length}${plan.files.length > 50 ? ' (first 50)' : ''}`,
    ...plan.files.slice(0, 50).map((file) => `  ${file}`),
    ...SURFACE_NAMES.map((name) => `  ${name}: ${values[name]}`),
    `api tests: ${describeApi(plan.api)}${plan.api.bin ? '; uploads the talaria-api binary' : ''}`,
    `migration baseline: ${values.migration_baseline}`,
  ]
  const summary = [
    '### What changed',
    '',
    '| | |',
    '|---|---|',
    `| mode | \`${cell(values.mode)}\` |`,
    `| range | ${values.range ? `\`${cell(values.range)}\`` : '—'} |`,
    `| reason | ${cell(values.reason)} |`,
    `| changed files | ${plan.all ? 'not counted (every surface runs)' : plan.files.length} |`,
    `| surfaces | ${on.length ? on.join(', ') : 'none'} (${on.length} of ${SURFACE_NAMES.length}) |`,
    `| api tests | ${values.api === 'true' ? cell(describeApi(plan.api)) : '—'} |`,
    `| migration baseline | ${values.db === 'true' ? `\`${cell(values.migration_baseline)}\`` : '—'} |`,
    '',
  ].join('\n')
  return { plan, output, log, summary }
}

function realGit(args) {
  return execFileSync('git', args, {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
    maxBuffer: 64 * 1024 * 1024,
  }).trim()
}

async function cliCi() {
  const { output, log, summary } = await ciOutputs(process.env, realGit)
  const target = process.env.GITHUB_OUTPUT
  // With no $GITHUB_OUTPUT the outputs are stdout, so the log moves to stderr.
  const say = target ? (line) => console.log(line) : (line) => console.error(line)
  for (const line of log) say(line)
  if (target) appendFileSync(target, `${output.join('\n')}\n`)
  else process.stdout.write(`${output.join('\n')}\n`)
  if (process.env.GITHUB_STEP_SUMMARY) appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${summary}\n`)
}

/**
 * This branch, as an offload run of it would see it: the base, the files, the
 * CI surfaces and the local gate's finer plan. `--full` previews ci/full/.
 * @param {string[]} args
 */
function cliPlan(args) {
  for (const a of args) {
    if (a !== '--full' && a !== '--json') usage(`unknown argument ${a}`)
  }
  const json = args.includes('--json')
  const picked = pickBase(realGit)
  if (!picked) {
    console.error('surfaces: base')
    console.error('neither origin/testing nor origin/rc is in this clone, so the branch has no base. `git fetch origin testing rc` and run again.')
    process.exit(2)
  }
  let branch = ''
  try {
    branch = realGit(['symbolic-ref', '--short', '-q', 'HEAD'])
  } catch {
    branch = ''
  }
  const lane = branch.startsWith('ci/') ? (offloadBranch(`refs/heads/${branch}`) ?? '') : branch
  const asked = args.includes('--full') || branch.startsWith('ci/full/')
  const full = asked || needsFull(lane)
  const mode = full ? 'full' : 'light'
  const range = `${picked.sha}..HEAD`
  const files = changedFiles(realGit, range)
  const reason = full
    ? `${asked ? 'asked for full' : `${lane} goes straight into rc`}: an offload runs every surface, macOS, Windows, iOS and e2e included`
    : `light: the surfaces since the merge-base with ${picked.ref}; macOS, Windows, iOS and e2e are skipped`
  const surfaces = full ? everySurface(true) : ciSurfaces(files)
  const local = classify(files, (rel) => readPackageName(rel))
  const api = planApi({ mode, all: full, files, baseText: (rel) => realGit(['show', `${picked.sha}:${rel}`]) })
  if (json) {
    console.log(JSON.stringify({ mode, all: full, reason, branch, base: picked, range, files, surfaces, local, api, migrationBaseline: picked.ref }, null, 2))
    return
  }
  // e2e is a full-mode job in ci.yml, like the macOS, Windows and iOS legs: a
  // light run skips it even when its surface moved.
  const runs = (name) => surfaces[name] && (full || name !== 'e2e')
  const on = SURFACE_NAMES.filter(runs)
  const off = SURFACE_NAMES.filter((name) => !runs(name))
  console.log(`surfaces: ${files.length} file(s) since ${picked.sha.slice(0, 12)} (merge-base with ${picked.ref}, ${picked.ahead} commit(s) ahead)`)
  console.log(`surfaces: mode ${mode} — ${reason}`)
  console.log(`surfaces: CI runs ${on.length ? on.join(', ') : 'only the invariants job'}${off.length ? `; skips ${off.join(', ')}` : ''}`)
  if (surfaces.api) console.log(`surfaces: api tests ${describeApi(api)}`)
  if (surfaces.db) console.log(`surfaces: migrations upgrade from ${picked.ref}`)
  if (local.apiPackages.length) console.log(`surfaces: api packages touched: ${local.apiPackages.join(' ')}`)
  if (local.apiGraph.length) console.log(`surfaces: api graph files touched: ${local.apiGraph.join(' ')}`)
  if (local.desktopCargo) console.log('surfaces: desktop rust touched')
}

function usage(problem) {
  console.error(`surfaces: ${problem}`)
  console.error('usage: node scripts/surfaces.mjs ci | plan [--full] [--json]')
  process.exit(1)
}

function invokedDirectly() {
  const file = process.argv[1]
  if (!file) return false
  try {
    return realpathSync(file) === realpathSync(fileURLToPath(import.meta.url))
  } catch {
    return false
  }
}

if (invokedDirectly()) {
  const [command, ...rest] = process.argv.slice(2)
  try {
    if (command === 'ci') {
      if (rest.length) usage(`ci takes no arguments (got ${rest.join(' ')})`)
      await cliCi()
    } else if (command === 'plan') {
      cliPlan(rest)
    } else {
      usage(command ? `unknown command ${command}` : 'a command is required')
    }
  } catch (e) {
    // A programming error. A computed plan, however wide, always exits 0.
    console.error(`surfaces: could not plan — ${e instanceof Error ? e.stack : e}`)
    process.exit(1)
  }
}
