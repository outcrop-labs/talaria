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
// USAGE
//   node scripts/surfaces.mjs ci                 # ci.yml: env in, $GITHUB_OUTPUT out
//   node scripts/surfaces.mjs plan [--full] [--json]
//                                                # this branch, as an offload run would see it
//
// The tests are scripts/surfaces.test.mjs (run by `bun run check`).

import { execFileSync } from 'node:child_process'
import { appendFileSync, existsSync, readFileSync, realpathSync } from 'node:fs'
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
//         (include_str!), the shipped skills tree, and mcp/src/index.ts (the
//         fitness catalog's sync test).
//   desktop  ui/src/lib/dither-engine.ts, which the launcher imports as
//         `@dither` (desktop/vite.config.ts, tsconfig.json).
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
    ...JOB_FILES,
  ],
  desktop: ['desktop/*', 'ui/src/lib/dither-engine.ts', ...JOB_FILES],
  mobile: ['mobile/*', ...JOB_FILES],
  e2e: [
    'ui/*', 'scripts/e2e/*', 'api/crates/*', 'api/src/*', 'api/Cargo.toml', 'api/Cargo.lock',
    'api/.cargo/*', 'api/rust-toolchain.toml', 'scripts/chassis.template.yml',
    ...JOB_FILES,
  ],
}

export const SURFACE_NAMES = Object.keys(SURFACES)

/** Every key `ci` writes to $GITHUB_OUTPUT, always, in this order. */
export const OUTPUT_KEYS = ['mode', 'base', 'range', 'reason', ...SURFACE_NAMES]

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
 * The files outside api/ the api job reads, in the same prefix form: what a
 * scoped api run has to map to the crates that read them.
 */
export const API_EXTERNAL_INPUTS = SURFACES.api
  .filter((glob) => !glob.startsWith('api/') && !JOB_FILES.includes(glob))
  .map((glob) => (glob.endsWith('/*') ? glob.slice(0, -1) : glob))

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

/**
 * The CI plan for one run.
 *
 * ctx: {event, ref, baseRef, headRef, calledRef, calledMode, before, baseSha,
 * deleted, repo, token} — ci.yml's env, see the `ci` CLI below.
 * fetchImpl: a fetch-compatible function for lastGreen (globalThis.fetch in CI).
 *
 * @returns {Promise<{mode: string, base: string, range: string, all: boolean,
 *   reason: string, surfaces: Record<string, boolean>, files: string[]}>}
 */
export async function planCi(ctx, git, fetchImpl = globalThis.fetch) {
  if (String(ctx.deleted) === 'true') {
    return {
      mode: 'none', base: '', range: '', all: false,
      reason: 'the push deleted the branch: nothing to check',
      surfaces: everySurface(false), files: [],
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
  return { mode, base, range, all, reason, surfaces: all ? everySurface(true) : ciSurfaces(files), files }
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
  }
  const output = OUTPUT_KEYS.map((key) => `${key}=${flat(values[key])}`)

  const on = SURFACE_NAMES.filter((name) => values[name] === 'true')
  const log = [
    `plan: mode=${values.mode} range=${values.range || '(none)'}${plan.all ? ' (every surface)' : ''}`,
    `plan: ${values.reason}`,
    `changed files: ${plan.files.length}${plan.files.length > 50 ? ' (first 50)' : ''}`,
    ...plan.files.slice(0, 50).map((file) => `  ${file}`),
    ...SURFACE_NAMES.map((name) => `  ${name}: ${values[name]}`),
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
  if (json) {
    console.log(JSON.stringify({ mode, all: full, reason, branch, base: picked, range, files, surfaces, local }, null, 2))
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
