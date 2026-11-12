// scripts/surfaces.mjs — the one planner CI and the local gate share. Run by
// `bun run check` (scripts/run-self-tests.mjs finds it).
//
// PURE: it reads the real tree (the workflows, a real Cargo manifest) but makes
// no network call and spawns no git. planCi gets a fake git, which answers
// only the exact commands it was given and throws on anything else, and a fake
// fetch.

import { readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  API_DB_INPUTS,
  API_EXTERNAL_INPUTS,
  API_KEYS,
  JOB_FILES,
  MODES,
  OUTPUT_KEYS,
  SURFACES,
  SURFACE_NAMES,
  UI_SURFACE,
  binQualifies,
  bunPackages,
  ciOutputs,
  ciSurfaces,
  classify,
  globMatch,
  installLines,
  isPromotion,
  itModulesFor,
  lastGreen,
  lockDelta,
  manifestDelta,
  migrationBaseline,
  missingInstalls,
  needsFull,
  offloadBranch,
  packagesReading,
  pickBase,
  planApi,
  planCi,
  readPackageName,
} from './surfaces.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
let failed = 0
let ran = 0

async function check(name, fn) {
  ran++
  try {
    await fn()
  } catch (e) {
    failed++
    console.error(`FAIL ${name}: ${e instanceof Error ? e.message : e}`)
  }
}

function eq(actual, expected, what = 'value') {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) throw new Error(`${what}: expected ${b}, got ${a}`)
}

function ok(cond, what) {
  if (!cond) throw new Error(what)
}

/** Every surface on, or off, by name. */
const every = (on) => Object.fromEntries(SURFACE_NAMES.map((n) => [n, on]))
const only = (...names) => Object.fromEntries(SURFACE_NAMES.map((n) => [n, names.includes(n)]))

/**
 * A git that knows only the commands in `table` (args joined by spaces). A
 * value that is an Error is thrown; anything not in the table throws, as a
 * failed git would.
 */
function fakeGit(table = {}) {
  const git = (args) => {
    const key = args.join(' ')
    git.calls.push(key)
    if (!Object.hasOwn(table, key)) throw new Error(`fatal: fake git has no answer for \`git ${key}\``)
    const v = table[key]
    if (v instanceof Error) throw v
    return v
  }
  git.calls = []
  return git
}

/** A fetch answering the runs listing with these head shas; it records the URL. */
function runsFetch(shas) {
  const f = async (url, init) => {
    f.urls.push(url)
    f.inits.push(init)
    return { ok: true, json: async () => ({ workflow_runs: shas.map((head_sha) => ({ head_sha })) }) }
  }
  f.urls = []
  f.inits = []
  return f
}

const fetchThrows = async () => {
  throw new Error('getaddrinfo ENOTFOUND api.github.com')
}

const DIFF = (range) => `diff --name-only --no-renames ${range}`
const resolves = (ref) => `rev-parse --verify --quiet ${ref}^{commit}`

// ── globMatch: bash `case` semantics ────────────────────────────────────────
await check('`*` spans `/`', () => {
  ok(globMatch('ui/*', 'ui/src/lib/a.ts'), 'ui/* matches a nested file')
  ok(globMatch('api/*.rs', 'api/crates/talaria-db/src/lib.rs'), 'api/*.rs matches a crate source')
  ok(globMatch('.github/actions/**', '.github/actions/mold/action.yml'), '** is just two stars')
  ok(globMatch('ui/*', 'ui/'), '* matches the empty string')
})
await check('every other character is literal', () => {
  ok(!globMatch('api/*.rs', 'api/Cargo.toml'), 'the suffix still has to match')
  ok(!globMatch('docs/CONTAINER.md', 'docs/CONTAINERxmd'), '. is not a wildcard')
  ok(!globMatch('a?', 'ab'), '? is literal here')
  ok(globMatch('a?', 'a?'), '? matches itself')
  ok(!globMatch('[ab]', 'a'), '[ is literal here')
  ok(!globMatch('ui/*', 'xui/a'), 'anchored at the start')
  ok(!globMatch('.gitignore', 'ui/.gitignore'), 'an exact path is exact')
})

// ── ciSurfaces ──────────────────────────────────────────────────────────────
await check('docs-only and changelog-only move nothing', () =>
  eq(ciSurfaces(['docs/FOO.md', 'changelog/2026-11-10-x.md', 'README.md']), every(false)))
await check('an empty diff moves nothing', () => eq(ciSurfaces([]), every(false)))
await check('every job-definition file moves every surface', () => {
  for (const file of ['.github/workflows/ci.yml', '.github/workflows/ci-offload.yml', 'scripts/surfaces.mjs', '.github/actions/setup-runtime/action.yml']) {
    eq(ciSurfaces([file]), every(true), file)
  }
})
await check('the job-definition files are in every list', () => {
  for (const [name, globs] of Object.entries(SURFACES)) {
    for (const file of JOB_FILES) ok(globs.includes(file), `${name} lacks ${file}`)
  }
})
await check('another workflow moves nothing', () => eq(ciSurfaces(['.github/workflows/release.yml']), every(false)))
await check('a ui file moves ui, e2e and the image', () => eq(ciSurfaces(['ui/src/routes/+page.svelte']), only('ui', 'e2e', 'image')))
await check('the dither engine moves the desktop too', () => eq(ciSurfaces(['ui/src/lib/dither-engine.ts']), only('ui', 'desktop', 'e2e', 'image')))
await check('a crate source moves api, cli (box.test reads every .rs) and e2e', () =>
  eq(ciSurfaces(['api/crates/talaria-db/src/lib.rs']), only('cli', 'api', 'e2e')))
await check('a ui-read crate file moves ui as well', () =>
  eq(ciSurfaces(['api/crates/talaria-gateway/src/guard.rs']), only('ui', 'cli', 'api', 'e2e')))
await check('api/tests moves api and cli, not e2e (not in the binary)', () =>
  eq(ciSurfaces(['api/tests/it/secretbox.rs']), only('cli', 'api')))
await check('the shared fixtures move ui, cli and api', () =>
  eq(ciSurfaces(['api/tests/it/fixtures/app-grants.json']), only('ui', 'cli', 'api')))
await check('the lockfile moves api and e2e', () => eq(ciSurfaces(['api/Cargo.lock']), only('api', 'e2e')))
await check('the migration array moves ui, api (its live suite), db, e2e and the image', () =>
  eq(ciSurfaces(['ui/src/server/db/pg.ts']), only('ui', 'api', 'db', 'e2e', 'image')))
await check('the migration scripts move db; only the replay the api job runs moves api too', () => {
  eq(ciSurfaces(['ui/scripts/check-migrations.ts']), only('ui', 'api', 'db', 'e2e', 'image'))
  eq(ciSurfaces(['ui/scripts/migrate-file.ts']), only('ui', 'db', 'e2e', 'image'))
  eq(ciSurfaces(['ui/scripts/check-git-hosts-mirror.sql']), only('ui', 'db', 'e2e', 'image'))
  eq(ciSurfaces(['ui/package.json']), only('ui', 'db', 'e2e', 'image'))
})
await check('mcp/src/index.ts moves mcp, api (the fitness catalog sync test) and the image', () =>
  eq(ciSurfaces(['mcp/src/index.ts']), only('mcp', 'api', 'image')))
await check('the chassis template moves ui, api, e2e and the image', () =>
  eq(ciSurfaces(['scripts/chassis.template.yml']), only('ui', 'api', 'e2e', 'image')))
await check('a shipped skill moves api and the image', () => eq(ciSurfaces(['scripts/skills/talaria-apps/SKILL.md']), only('api', 'image')))
await check('the cli reads CONTAINER.md, the devbox compose and .gitignore', () => {
  for (const f of ['docs/CONTAINER.md', '.gitignore']) eq(ciSurfaces([f]), only('cli'), f)
  eq(ciSurfaces(['docker/devbox.compose.yml']), only('cli', 'image'), 'docker/ is the image surface too')
})
await check('the SDK docs and the scaffolder move ui', () => {
  eq(ciSurfaces(['docs/sdk/client.md']), only('ui'))
  eq(ciSurfaces(['cli/src/cmd/app/skeleton.ts']), only('ui', 'cli', 'image'))
})
await check('desktop, mobile and the e2e stubs move their own', () => {
  eq(ciSurfaces(['desktop/src-tauri/src/main.rs']), only('desktop'))
  eq(ciSurfaces(['mobile/shared/build.gradle.kts']), only('mobile'))
  eq(ciSurfaces(['scripts/e2e/mcp-stubs.mjs']), only('e2e'))
})
await check('the build context moves the image, and only the image', () => {
  for (const f of ['.dockerignore', 'Dockerfile', 'docker/entrypoint.sh', 'docker/await-deps.mjs', 'omp-auth/src/server.ts']) {
    eq(ciSurfaces([f]), only('image'), f)
  }
})
await check('what the image builds or copies moves it: ui, cli, mcp, apps, the schema, the skills', () => {
  for (const f of ['ui/src/routes/+page.svelte', 'cli/src/ports.ts', 'cli/src/cmd/app/skeleton.ts', 'mcp/src/a.ts', 'apps/x/y.ts',
    'docs/sdk/talaria.schema.json', 'scripts/skills/talaria-apps/SKILL.md', 'scripts/chassis.template.yml']) {
    ok(ciSurfaces([f]).image, `${f} moves the image`)
  }
})
await check('what the image never reads leaves it alone: the api, the package Dockerfile, desktop, mobile, docs', () => {
  for (const f of ['api/crates/talaria-db/src/lib.rs', 'api/package.Dockerfile', 'api/Cargo.lock', 'desktop/src/App.svelte',
    'mobile/shared/build.gradle.kts', 'docs/sdk/client.md', 'docs/CONTAINER.md', 'README.md', '.github/workflows/release.yml']) {
    ok(!ciSurfaces([f]).image, `${f} does not move the image`)
  }
})
await check('every file the ui build reads from outside ui/ moves the image (check-invariants.mjs, image-build-input-ignored)', () => {
  // The re-admitted lines of .dockerignore are the ui build's inputs from
  // outside ui/; the image has to rebuild when one changes.
  const ignore = readFileSync(join(ROOT, '.dockerignore'), 'utf8')
  const kept = ignore.split('\n').map((l) => l.trim()).filter((l) => l.startsWith('!') && !l.includes('*')).map((l) => l.slice(1))
  ok(kept.length >= 2, `the re-admitted files: ${JSON.stringify(kept)}`)
  for (const f of kept) ok(ciSurfaces([f]).image, `${f} is re-admitted to the context but does not move the image`)
})

// ── UI_SURFACE is derived from SURFACES.ui ──────────────────────────────────
await check('UI_SURFACE is the ui list in the gate\'s prefix form', () =>
  eq([...UI_SURFACE].sort(), [
    'api/crates/talaria-gateway/src/guard.rs',
    'api/crates/talaria-harness-defs/src/registry.rs',
    'api/crates/talaria-platform-agents/src/lib.rs',
    'api/tests/it/fixtures/',
    'apps/',
    'cli/src/cmd/app/',
    'docs/sdk/',
    'scripts/build-workbench-image.sh',
    'scripts/chassis.template.yml',
    'scripts/check-invariants.mjs',
    'scripts/gen-secretbox-vectors.mjs',
    'scripts/workbench.Dockerfile',
    'ui/',
  ]))
await check('UI_SURFACE has no glob the prefix form cannot say', () => {
  for (const p of UI_SURFACE) ok(!p.includes('*'), `${p} is not a prefix or an exact path`)
})
await check('API_EXTERNAL_INPUTS is the api list outside api/', () =>
  eq(API_EXTERNAL_INPUTS, ['scripts/chassis.template.yml', 'scripts/hermes-toolsets.json', 'scripts/hermes-skill-authority.json', 'scripts/skills/', 'mcp/src/index.ts']))

// ── classify (the local gate's finer plan; moved from gate.mjs) ─────────────
const stub = (rel) => {
  if (rel === 'api/Cargo.toml') return 'talaria-api'
  if (rel === 'api/crates/talaria-error/Cargo.toml') return 'talaria-error'
  if (rel === 'api/crates/talaria-db/Cargo.toml') return 'talaria-db'
  return null
}
await check('docs-only compiles nothing', () => {
  const docs = classify(['docs/FOO.md', 'changelog/x.md', '.github/workflows/ci.yml'], stub)
  ok(!docs.ui && !docs.mcp && !docs.cli && docs.apiPackages.length === 0 && !docs.desktopCargo && !docs.mobile, 'docs-only compiles nothing')
  ok(docs.apiGraph.length === 0, 'a workflow edit is not a local api compile')
  ok(!classify(['scripts/surfaces.mjs', '.github/workflows/ci-offload.yml'], stub).ui, 'the planner and the offload workflow are not a local ui run')
})
await check('a crate file maps to its package', () => {
  const one = classify(['api/crates/talaria-error/src/lib.rs'], stub)
  eq(one.apiPackages, ['talaria-error'])
  eq(one.apiGraph, [], 'graph')
})
await check('graph files do not select packages', () => {
  const graph = classify(['api/Cargo.lock', 'api/Cargo.toml', 'api/.cargo/config.toml'], stub)
  ok(graph.apiPackages.length === 0 && graph.apiGraph.length === 3, 'three graph files, no packages')
})
await check('api/src and api/tests are the root package', () =>
  eq(classify(['api/src/db.rs', 'api/tests/it/secretbox.rs'], stub).apiPackages, ['talaria-api']))
await check('packages are deduped and sorted', () =>
  eq(classify(['api/crates/talaria-db/src/lib.rs', 'api/crates/talaria-error/Cargo.toml'], stub).apiPackages, ['talaria-db', 'talaria-error']))
await check('a missing manifest is unmapped, not a workspace compile', () => {
  const gone = classify(['api/crates/does-not-exist/src/lib.rs'], stub)
  ok(gone.apiPackages.length === 0 && gone.apiUnmapped.length === 1, 'unmapped')
})
await check('a dockerfile is not a compile', () => {
  const d = classify(['api/package.Dockerfile'], stub)
  ok(d.apiPackages.length === 0 && d.apiGraph.length === 0, 'no compile')
})
await check('each surface selects its own', () => {
  const s = classify([
    'ui/src/a.ts',
    'apps/x/y.ts',
    'mcp/src/index.ts',
    'cli/src/ports.ts',
    'desktop/src/App.svelte',
    'desktop/src-tauri/src/main.rs',
    'mobile/shared/src/commonMain/kotlin/app/talaria/mobile/App.kt',
  ], stub)
  ok(s.ui && s.mcp && s.cli, 'ui, apps, mcp, cli each select their surface')
  ok(s.desktopTypecheck && s.desktopCargo, 'desktop svelte and rust select separately')
  ok(s.mobile, 'a kotlin source file selects the mobile surface')
})
await check('an SDK doc runs the ui suite, which compiles its examples', () => {
  const s = classify(['docs/sdk/client.md'], stub)
  ok(s.ui && !s.cli, 'ui, not cli')
})
await check('the app scaffolder runs the ui suite and the cli; other cli source only the cli', () => {
  const s = classify(['cli/src/cmd/app/skeleton.ts'], stub)
  ok(s.ui && s.cli, 'scaffolder: ui and cli')
  const c = classify(['cli/src/ports.ts'], stub)
  ok(c.cli && !c.ui, 'other cli source: cli only')
})
await check("what the ui suite reads from outside ui/ runs it, as SURFACES.ui says", () => {
  for (const f of [
    'scripts/check-invariants.mjs',
    'scripts/gen-secretbox-vectors.mjs',
    'scripts/chassis.template.yml',
    'api/tests/it/fixtures/app-grants.json',
    'api/crates/talaria-gateway/src/guard.rs',
  ]) {
    ok(classify([f], stub).ui, `${f} runs the ui suite`)
  }
  ok(!classify(['api/crates/talaria-error/src/lib.rs', 'scripts/check-docs.mjs'], stub).ui, 'a crate or script the ui suite does not read leaves it alone')
  const dither = classify(['ui/src/lib/dither-engine.ts'], stub)
  ok(dither.ui && dither.desktopTypecheck, 'the dither engine runs the ui suite and the desktop typecheck')
})
await check('the local ui plan and the CI ui surface agree file by file', () => {
  for (const f of ['ui/a.ts', 'apps/x', 'docs/sdk/a.md', 'cli/src/cmd/app/x.ts', 'api/tests/it/fixtures/f.json',
    'scripts/workbench.Dockerfile', 'scripts/build-workbench-image.sh', 'docs/FOO.md', 'cli/src/ports.ts', 'scripts/check-docs.mjs']) {
    eq(classify([f], stub).ui, ciSurfaces([f]).ui, f)
  }
})
await check('a mobile readme or provisioner script is not a gradle build; the version catalog is', () => {
  ok(!classify(['mobile/README.md', 'mobile/tools/devbox-install.sh'], stub).mobile, 'docs and provisioner')
  ok(classify(['mobile/gradle/libs.versions.toml'], stub).mobile, 'version catalog')
})
await check('the install plan: a fresh worktree installs exactly what it typechecks', () => {
  const fresh = (rel) => rel === 'ui/node_modules'
  const cliInstalls = missingInstalls(bunPackages(classify(['cli/src/ports.ts'], stub)), fresh)
  eq(cliInstalls, ['cli'])
  eq(installLines(cliInstalls), ['gate: run bun install --frozen-lockfile in cli/ (no cli/node_modules)'])
})
await check('the real talaria-error manifest names talaria-error', () =>
  eq(classify(['api/crates/talaria-error/src/lib.rs'], (rel) => readPackageName(rel, ROOT)).apiPackages, ['talaria-error']))

// ── pickBase ────────────────────────────────────────────────────────────────
const twoBases = (testingAhead, rcAhead) => fakeGit({
  [resolves('origin/testing')]: 'tsha',
  'merge-base HEAD origin/testing': 'mbt',
  'rev-list --count mbt..HEAD': String(testingAhead),
  [resolves('origin/rc')]: 'rsha',
  'merge-base HEAD origin/rc': 'mbr',
  'rev-list --count mbr..HEAD': String(rcAhead),
})
await check('pickBase takes the base with the fewest commits on HEAD', () => {
  eq(pickBase(twoBases(2, 9)), { ref: 'origin/testing', sha: 'mbt', ahead: 2 })
  eq(pickBase(twoBases(4000, 3)), { ref: 'origin/rc', sha: 'mbr', ahead: 3 }, 'a stale testing loses')
})
await check('pickBase breaks a tie by list order', () => eq(pickBase(twoBases(5, 5)).ref, 'origin/testing'))
await check('pickBase skips a base that does not resolve, and is null when none does', () => {
  const rcOnly = fakeGit({ [resolves('origin/rc')]: 'r', 'merge-base HEAD origin/rc': 'mbr', 'rev-list --count mbr..HEAD': '7' })
  eq(pickBase(rcOnly), { ref: 'origin/rc', sha: 'mbr', ahead: 7 })
  eq(pickBase(fakeGit({})), null)
})

// ── lastGreen ───────────────────────────────────────────────────────────────
await check('lastGreen asks for successful push runs of ci.yml on the branch, with the token', async () => {
  const f = runsFetch(['c2'])
  const git = fakeGit({ 'rev-list --first-parent --max-count=500 HEAD^': 'c1\nc2\nc3', 'cat-file -e c2:scripts/surfaces.mjs': '' })
  eq(await lastGreen({ repo: 'o/r', branch: 'rc', token: 't0k', fetchImpl: f, git }), 'c2')
  eq(f.urls[0], 'https://api.github.com/repos/o/r/actions/workflows/ci.yml/runs?branch=rc&event=push&status=success&per_page=100', 'url')
  eq(f.inits[0].headers.Authorization, 'Bearer t0k', 'auth header')
})
await check('lastGreen ignores a green run whose tree lacks surfaces.mjs', async () => {
  const git = fakeGit({
    'rev-list --first-parent --max-count=500 HEAD^': 'c1\nc2\nc3',
    'cat-file -e c1:scripts/surfaces.mjs': new Error('fatal: path does not exist'),
    'cat-file -e c3:scripts/surfaces.mjs': '',
  })
  eq(await lastGreen({ repo: 'o/r', branch: 'rc', fetchImpl: runsFetch(['c1', 'c3']), git }), 'c3')
  const older = fakeGit({
    'rev-list --first-parent --max-count=500 HEAD^': 'c1\nc2',
    'cat-file -e c1:scripts/surfaces.mjs': new Error('fatal: path does not exist'),
  })
  eq(await lastGreen({ repo: 'o/r', branch: 'rc', fetchImpl: runsFetch(['c1']), git: older }), null)
})
await check('lastGreen is null on an API error, a non-200, or no repo', async () => {
  const git = fakeGit({ 'rev-list --first-parent --max-count=500 HEAD^': 'c1' })
  eq(await lastGreen({ repo: 'o/r', branch: 'rc', fetchImpl: fetchThrows, git }), null)
  eq(await lastGreen({ repo: 'o/r', branch: 'rc', fetchImpl: async () => ({ ok: false, status: 403 }), git }), null)
  eq(await lastGreen({ repo: '', branch: 'rc', fetchImpl: runsFetch(['c1']), git }), null)
})

// ── planCi ──────────────────────────────────────────────────────────────────
const prMerge = (files) => fakeGit({
  'rev-parse --verify --quiet HEAD^2': 'headsha',
  'rev-parse HEAD^1': 'basetip',
  [DIFF('HEAD^1..HEAD')]: files.join('\n'),
})
await check('a PR into testing is light, on the merge ref\'s first parent', async () => {
  const p = await planCi({ event: 'pull_request', baseRef: 'testing', baseSha: 'old' }, prMerge(['ui/src/a.ts']), fetchThrows)
  eq([p.mode, p.range, p.base, p.all], ['light', 'HEAD^1..HEAD', 'basetip', false])
  eq(p.surfaces, only('ui', 'e2e', 'image'))
})
await check('a PR into rc or main is full; into a stacked branch light', async () => {
  for (const [into, mode] of [['rc', 'full'], ['main', 'full'], ['ckpt/x', 'light'], ['hotfix/a', 'light']]) {
    eq((await planCi({ event: 'pull_request', baseRef: into }, prMerge(['docs/a.md']), fetchThrows)).mode, mode, into)
  }
})
await check('a PR with no merge parent diffs its own changes since the base sha', async () => {
  const git = fakeGit({ [DIFF('b45e...HEAD')]: 'mcp/src/a.ts' })
  const p = await planCi({ event: 'pull_request', baseRef: 'testing', baseSha: 'b45e' }, git, fetchThrows)
  eq([p.mode, p.range, p.base], ['light', 'b45e...HEAD', 'b45e'])
  eq(p.surfaces, only('mcp', 'image'))
})
await check('a push to rc diffs from rc\'s last green run', async () => {
  const f = runsFetch(['g1'])
  const git = fakeGit({
    'rev-list --first-parent --max-count=500 HEAD^': 'p1\ng1',
    'cat-file -e g1:scripts/surfaces.mjs': '',
    [DIFF('g1..HEAD')]: 'api/crates/talaria-db/src/lib.rs',
  })
  const p = await planCi({ event: 'push', ref: 'refs/heads/rc', repo: 'o/r', token: 't' }, git, f)
  eq([p.mode, p.range, p.base, p.all], ['full', 'g1..HEAD', 'g1', false])
  eq(p.surfaces, only('cli', 'api', 'e2e'))
  ok(f.urls[0].includes('branch=rc'), 'asked about rc')
})
await check('a push to rc falls back to the merge-base with origin/main when the API throws', async () => {
  const git = fakeGit({ 'merge-base origin/main HEAD': 'mbm', [DIFF('mbm..HEAD')]: 'desktop/src/App.svelte' })
  const p = await planCi({ event: 'push', ref: 'refs/heads/rc', repo: 'o/r' }, git, fetchThrows)
  eq([p.mode, p.range, p.base], ['full', 'mbm..HEAD', 'mbm'])
  eq(p.surfaces, only('desktop'))
})
await check('a push to testing is integration, falling back to the merge-base with origin/rc', async () => {
  const git = fakeGit({ 'merge-base origin/rc HEAD': 'mbr', [DIFF('mbr..HEAD')]: 'ui/a.ts' })
  const p = await planCi({ event: 'push', ref: 'refs/heads/testing', repo: 'o/r' }, git, runsFetch([]))
  eq([p.mode, p.range], ['integration', 'mbr..HEAD'])
})
await check('a push to main: last green, else before, else everything', async () => {
  const before = fakeGit({ [DIFF('bef0..HEAD')]: 'mobile/a.kt' })
  const p = await planCi({ event: 'push', ref: 'refs/heads/main', before: 'bef0', repo: 'o/r' }, before, fetchThrows)
  eq([p.mode, p.range, p.all], ['full', 'bef0..HEAD', false])
  eq(p.surfaces, only('mobile'))
  for (const b of ['', '0000000000000000000000000000000000000000']) {
    const q = await planCi({ event: 'push', ref: 'refs/heads/main', before: b, repo: 'o/r' }, fakeGit({}), fetchThrows)
    eq([q.mode, q.all, q.surfaces], ['full', true, every(true)], `before '${b}'`)
  }
})
await check('any other push is light, against the nearer base', async () => {
  const git = fakeGit({
    [resolves('origin/rc')]: 'r', 'merge-base HEAD origin/rc': 'mbr', 'rev-list --count mbr..HEAD': '2',
    [DIFF('mbr..HEAD')]: 'cli/src/ports.ts',
  })
  const p = await planCi({ event: 'push', ref: 'refs/heads/wt/x' }, git, fetchThrows)
  eq([p.mode, p.range], ['light', 'mbr..HEAD'])
  eq(p.surfaces, only('cli', 'image'))
})
const lightBase = (files) => fakeGit({
  [resolves('origin/testing')]: 't', 'merge-base HEAD origin/testing': 'mbt', 'rev-list --count mbt..HEAD': '900',
  [resolves('origin/rc')]: 'r', 'merge-base HEAD origin/rc': 'mbr', 'rev-list --count mbr..HEAD': '1',
  [DIFF('mbr..HEAD')]: files.join('\n'),
})
await check('called light diffs against the nearer of testing and rc', async () => {
  const p = await planCi({ event: 'push', ref: 'refs/heads/ci/wt/x', calledRef: 'abc', calledMode: 'light' }, lightBase(['mcp/a.ts']), fetchThrows)
  eq([p.mode, p.range, p.base, p.all], ['light', 'mbr..HEAD', 'mbr', false])
  eq(p.surfaces, only('mcp', 'image'))
})
await check('called full or integration runs every surface', async () => {
  for (const mode of ['full', 'integration']) {
    const p = await planCi({ event: 'push', ref: 'refs/heads/ci/full/wt/x', calledRef: 'abc', calledMode: mode }, fakeGit({}), fetchThrows)
    eq([p.mode, p.all, p.surfaces], [mode, true, every(true)], mode)
  }
})
await check('called with no mode is release.yml: full, every surface', async () => {
  const p = await planCi({ event: 'schedule', ref: 'refs/heads/main', calledRef: 'rc', calledMode: '' }, fakeGit({}), fetchThrows)
  eq([p.mode, p.all, p.surfaces], ['full', true, every(true)])
})
await check('called with an unknown mode is full, every surface', async () => {
  const p = await planCi({ event: 'push', ref: 'refs/heads/ci/x', calledRef: 'abc', calledMode: 'promotion' }, fakeGit({}), fetchThrows)
  eq([p.mode, p.all], ['full', true])
})
await check('an offload of a lane into rc runs full', async () => {
  for (const ref of ['refs/heads/ci/hotfix/x', 'refs/heads/ci/release/roll', 'refs/heads/ci/sync/a', 'refs/heads/ci/promote/testing-abc', 'refs/heads/ci/backflow/b', 'refs/heads/ci/full/hotfix/x']) {
    const p = await planCi({ event: 'push', ref, calledRef: 'abc', calledMode: 'light' }, fakeGit({}), fetchThrows)
    eq([p.mode, p.all], ['full', true], ref)
  }
  const plain = await planCi({ event: 'push', ref: 'refs/heads/ci/wt/hotfix-notes', calledRef: 'abc', calledMode: 'light' }, lightBase([]), fetchThrows)
  eq(plain.mode, 'light', 'only a prefix makes a lane')
})
await check('offloadBranch and needsFull', () => {
  eq(offloadBranch('refs/heads/ci/full/wt/a'), 'wt/a')
  eq(offloadBranch('refs/heads/ci/wt/a'), 'wt/a')
  eq(offloadBranch('refs/heads/wt/a'), null)
  ok(needsFull('hotfix/a') && needsFull('backflow/x') && !needsFull('wt/hotfix'), 'prefixes only')
})
await check('a deleted branch plans nothing', async () => {
  const p = await planCi({ event: 'push', ref: 'refs/heads/rc', deleted: 'true' }, fakeGit({}), fetchThrows)
  eq([p.mode, p.surfaces], ['none', every(false)])
})
await check('a git that throws runs every surface, and keeps the mode', async () => {
  const pr = await planCi({ event: 'pull_request', baseRef: 'rc', baseSha: 'b' }, fakeGit({}), fetchThrows)
  eq([pr.mode, pr.all, pr.surfaces], ['full', true, every(true)], 'pr')
  ok(/could not diff/.test(pr.reason), `the reason says so: ${pr.reason}`)
  const rc = await planCi({ event: 'push', ref: 'refs/heads/rc', repo: 'o/r' }, fakeGit({}), runsFetch(['x']))
  eq([rc.mode, rc.all], ['full', true], 'rc')
  const light = await planCi({ event: 'push', ref: 'refs/heads/wt/x' }, fakeGit({}), fetchThrows)
  eq([light.mode, light.all], ['light', true], 'light with no base')
})
await check('an event with no rule is full, every surface', async () => {
  const p = await planCi({ event: 'merge_group', ref: 'refs/heads/gh-readonly-queue/x' }, fakeGit({}), fetchThrows)
  eq([p.mode, p.all], ['full', true])
})

// ── Promotion mode: the rc → main pull request ──────────────────────────────
const PROMO = { event: 'pull_request', baseRef: 'main', headRef: 'rc', repo: 'o/r', headRepo: 'o/r', baseSha: 'm0', root: ROOT }
/** The merge ref of rc into main: HEAD's tree, and rc's tip tree. */
const promoGit = (mergeTree, rcTree, files = ['ui/src/a.ts', 'api/crates/talaria-db/src/lib.rs', 'desktop/src/App.svelte']) => fakeGit({
  'rev-parse --verify --quiet HEAD^2': 'rctip',
  'rev-parse HEAD^1': 'maintip',
  'rev-parse HEAD^{tree}': mergeTree,
  'rev-parse HEAD^2^{tree}': rcTree,
  [DIFF('HEAD^1..HEAD')]: files.join('\n'),
})
await check("promotion: this repository's rc into main, merge tree == rc's tree, runs no surface", async () => {
  const git = promoGit('t1', 't1')
  const p = await planCi(PROMO, git, fetchThrows)
  eq([p.mode, p.all, p.range, p.base], ['promotion', false, 'HEAD^1..HEAD', 'maintip'])
  eq(p.surfaces, every(false), 'every surface off, whatever the diff touched')
  eq(p.reason, "the merge tree is rc's tip; its evidence is rc's push CI and rc-deploy, which the promotion gate requires")
  eq([p.api.scope, p.api.packages, p.api.itModules, p.api.live, p.api.bin], ['packages', [], [], false, false], 'no api test, no binary')
  eq(p.files.length, 3, 'the files are listed for the log')
  eq(p.migrationBaseline, 'origin/main')
  ok(git.calls.includes('rev-parse HEAD^{tree}') && git.calls.includes('rev-parse HEAD^2^{tree}'), `compared the trees: ${git.calls.join(', ')}`)
})
await check('promotion: a diff that will not resolve still promotes; the trees decided', async () => {
  const git = fakeGit({
    'rev-parse --verify --quiet HEAD^2': 'rctip', 'rev-parse HEAD^1': 'maintip',
    'rev-parse HEAD^{tree}': 't1', 'rev-parse HEAD^2^{tree}': 't1',
  })
  const p = await planCi(PROMO, git, fetchThrows)
  eq([p.mode, p.files, p.surfaces], ['promotion', [], every(false)])
})
await check("promotion: unequal trees are full, every surface the diff moved, and the reason says so", async () => {
  const p = await planCi(PROMO, promoGit('t1', 't2', ['ui/src/a.ts']), fetchThrows)
  eq([p.mode, p.all], ['full', false])
  eq(p.surfaces, only('ui', 'e2e', 'image'))
  ok(/not promotion mode/.test(p.reason) && /is not rc's tip tree/.test(p.reason), p.reason)
})
await check("promotion: a fork's rc is full", async () => {
  const p = await planCi({ ...PROMO, headRepo: 'fork/r' }, promoGit('t1', 't1', ['ui/src/a.ts']), fetchThrows)
  eq([p.mode, p.surfaces], ['full', only('ui', 'e2e', 'image')])
  ok(/fork\/r's rc/.test(p.reason), p.reason)
})
await check('promotion: an unknown head repository is full', async () => {
  for (const ctx of [{ ...PROMO, headRepo: '' }, { ...PROMO, repo: '' }]) {
    const p = await planCi(ctx, promoGit('t1', 't1', ['ui/src/a.ts']), fetchThrows)
    eq(p.mode, 'full', JSON.stringify(ctx))
    ok(/head repository is unknown/.test(p.reason), p.reason)
  }
})
await check('promotion: no merge ref, or trees git cannot read, is full', async () => {
  const noMerge = fakeGit({ [DIFF('m0...HEAD')]: 'ui/src/a.ts' })
  const p = await planCi(PROMO, noMerge, fetchThrows)
  eq([p.mode, p.range], ['full', 'm0...HEAD'])
  ok(/not the merge ref/.test(p.reason), p.reason)
  const unreadable = fakeGit({
    'rev-parse --verify --quiet HEAD^2': 'rctip', 'rev-parse HEAD^1': 'maintip',
    'rev-parse HEAD^{tree}': 't1', 'rev-parse HEAD^2^{tree}': new Error('fatal: bad revision'),
    [DIFF('HEAD^1..HEAD')]: 'ui/src/a.ts',
  })
  const q = await planCi(PROMO, unreadable, fetchThrows)
  eq(q.mode, 'full')
  ok(/could not read the trees/.test(q.reason), q.reason)
})
await check('promotion is only rc into main: another head into main, rc into testing, a push of rc are not', async () => {
  const other = await planCi({ ...PROMO, headRef: 'sync/x' }, promoGit('t1', 't1', ['ui/src/a.ts']), fetchThrows)
  eq(other.mode, 'full', 'another head into main')
  ok(!/promotion/.test(other.reason), `no promotion talk for another head: ${other.reason}`)
  const intoTesting = await planCi({ ...PROMO, baseRef: 'testing' }, promoGit('t1', 't1', ['ui/src/a.ts']), fetchThrows)
  eq(intoTesting.mode, 'light', 'rc into testing')
  ok(!isPromotion({ ...PROMO, event: 'push' }, fakeGit({}), true).ok, 'a push')
})
await check('a call can never ask for promotion mode', async () => {
  ok(!MODES.includes('promotion'), 'promotion is not a callable mode')
  const p = await planCi({ event: 'push', ref: 'refs/heads/ci/x', calledRef: 'abc', calledMode: 'promotion' }, fakeGit({}), fetchThrows)
  eq([p.mode, p.all, p.surfaces], ['full', true, every(true)])
})

// ── The api job's scope ─────────────────────────────────────────────────────
// Two real rc lockfiles: the parent of 57ef46ff (#708) and 57ef46ff, which
// added the talaria-work-pulse crate. Every changed block and four unchanged
// ones are kept verbatim; talaria-api's dependency list (every crate) is cut
// to the entries around the change, the same way on both sides.
const LOCK_BASE = readFileSync(join(ROOT, 'scripts/surfaces.fixtures/Cargo.lock.57ef46ff-parent'), 'utf8')
const LOCK_HEAD = readFileSync(join(ROOT, 'scripts/surfaces.fixtures/Cargo.lock.57ef46ff'), 'utf8')
const MANIFEST = readFileSync(join(ROOT, 'api/Cargo.toml'), 'utf8')
const noBase = () => {
  throw new Error('fatal: no base in this test')
}

await check('lockDelta: a workspace-member-only diff from two real rc lockfiles is its packages', () => {
  const d = lockDelta(LOCK_BASE, LOCK_HEAD)
  eq(d.workspace, false, 'workspace')
  eq(d.packages, ['talaria-api', 'talaria-harness', 'talaria-routes-boards', 'talaria-routes-fleet',
    'talaria-runs-work-session', 'talaria-work-pulse', 'talaria-work-wait', 'talaria-workbench-mcp'])
  eq(lockDelta(LOCK_HEAD, LOCK_HEAD), { workspace: false, packages: [] }, 'no change')
})
await check('lockDelta: a source, checksum or registry change is the workspace', () => {
  const checksum = LOCK_HEAD.replace(/(name = "aead"[\s\S]*?checksum = ")([0-9a-f])/, (m, pre, c) => `${pre}${c === '0' ? '1' : '0'}`)
  ok(checksum !== LOCK_HEAD, 'the fixture has aead with a checksum')
  ok(lockDelta(LOCK_HEAD, checksum).workspace, 'a checksum change')
  const added = `${LOCK_HEAD}\n[[package]]\nname = "left-pad"\nversion = "1.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "00"\n`
  ok(lockDelta(LOCK_HEAD, added).workspace, 'an added registry crate')
  ok(lockDelta(added, LOCK_HEAD).workspace, 'a removed registry crate')
  ok(lockDelta(LOCK_HEAD, LOCK_HEAD.replace('version = 4', 'version = 3')).workspace, 'the lockfile format')
})
await check('lockDelta: a removed member leaves nothing to test; the members that named it are in', () => {
  const d = lockDelta(LOCK_HEAD, LOCK_BASE)
  ok(!d.workspace && !d.packages.includes('talaria-work-pulse') && d.packages.includes('talaria-work-wait'), JSON.stringify(d))
})

const member = (list) => MANIFEST.replace('members = ["crates/*"]', `members = [${list}]`)
await check('manifestDelta: members, path dependencies, the root package', () => {
  ok(MANIFEST.includes('members = ["crates/*"]'), 'api/Cargo.toml still says members = ["crates/*"]')
  const opts = { root: ROOT }
  eq(manifestDelta(member('"crates/talaria-db"'), member('"crates/talaria-db",\n  "crates/talaria-error",'), opts),
    { workspace: false, packages: ['talaria-error'] }, 'an added member')
  ok(manifestDelta(member('"crates/talaria-db", "crates/talaria-error"'), member('"crates/talaria-db"'), opts).workspace, 'a removed member')
  ok(manifestDelta(member('"crates/talaria-db"'), member('"crates/talaria-db", "more/*"'), opts).workspace, 'a glob member')
  const pathDep = MANIFEST.replace(/^talaria-yaml11 = \{ path = "crates\/talaria-yaml11" \}$/m, '$&\n\ntalaria-zzz = { path = "crates/talaria-zzz" }')
  ok(pathDep !== MANIFEST, 'the fixture line exists')
  eq(manifestDelta(MANIFEST, pathDep, opts), { workspace: false, packages: ['talaria-zzz'] }, 'a workspace path dependency')
  const rootDep = MANIFEST.replace(/^\[dependencies\]$/m, '$&\ntalaria-zzz = { workspace = true }')
  eq(manifestDelta(MANIFEST, rootDep, opts), { workspace: false, packages: ['talaria-api'] }, 'a root dependency on a member')
  eq(manifestDelta(MANIFEST, MANIFEST.replace(/^\[package\]$/m, '# a comment\n$&\n\n# another'), opts),
    { workspace: false, packages: [] }, 'comments and blank lines change nothing')
})
await check('manifestDelta: profiles, registry entries and features are the workspace', () => {
  const opts = { root: ROOT }
  ok(manifestDelta(MANIFEST, MANIFEST.replace(/^opt-level = 1$/m, 'opt-level = 2'), opts).workspace, 'a profile')
  ok(manifestDelta(MANIFEST, MANIFEST.replace(/^libc = "0\.2"$/m, 'libc = "0.3"'), opts).workspace, 'a registry version')
  const feat = MANIFEST.replace(/^\[dependencies\]$/m, '$&\nleft-pad = { workspace = true, features = ["x"] }')
  ok(manifestDelta(MANIFEST, feat, opts).workspace, 'a features list on a root dependency')
  ok(manifestDelta(MANIFEST, MANIFEST.replace('resolver = "3"', 'resolver = "2"'), opts).workspace, '[workspace] resolver')
  const quote3 = '"'.repeat(3)
  ok(manifestDelta(MANIFEST, MANIFEST.replace('[profile.dev]', `[profile.dev]\nx = ${quote3}\na\n${quote3}`), opts).workspace, 'what it cannot read')
})

await check('itModulesFor: the root package is every module; a crate the support module reaches is too', () => {
  eq(itModulesFor(['talaria-api'], ROOT), ['*'])
  eq(itModulesFor(['talaria-db'], ROOT), ['*'], 'support builds its pool through talaria_api::db')
})
await check('itModulesFor: talaria-runs-store reaches runs_store through the facades; talaria-error reaches none', () => {
  const mods = itModulesFor(['talaria-runs-store'], ROOT)
  ok(mods.includes('runs_store'), `runs_store is in ${JSON.stringify(mods)}`)
  for (const m of mods) {
    const text = readFileSync(join(ROOT, `api/tests/it/${m}.rs`), 'utf8')
    ok(/\btalaria_runs_store\b|runs::store\b|runs::\{/.test(text), `${m} names neither talaria_runs_store nor runs::store`)
  }
  ok(!mods.includes('secretbox') && !mods.includes('yaml_stringify'), 'unrelated modules stay out')
  eq(itModulesFor(['talaria-error'], ROOT), [])
})
await check('packagesReading: the crates that name a file outside api/', () => {
  eq(packagesReading('scripts/hermes-toolsets.json', ROOT), ['talaria-agent-defs'])
  eq(packagesReading('scripts/no-such-input.json', ROOT), [])
})
await check('API_DB_INPUTS is the db files the api job reads', () =>
  eq(API_DB_INPUTS, ['ui/src/server/db/*', 'ui/scripts/check-migrations.ts']))

const light = (files, extra = {}) => planApi({ mode: 'light', all: false, files, root: ROOT, baseText: noBase, ...extra })
await check('light: one crate tests that crate, with no database and no live module', () => {
  eq(light(['api/crates/talaria-error/src/lib.rs']), {
    scope: 'packages', packages: ['talaria-error'], libPackages: ['talaria-error'], itModules: [], live: false,
    why: '1 package(s) the diff touches',
  })
})
await check('light: a db file alone compiles nothing and runs every live module', () => {
  const p = light(['ui/src/server/db/pg.ts'])
  eq([p.scope, p.packages, p.itModules, p.live], ['packages', [], ['*'], true])
})
await check("light: a live module's own file runs that module, in the root package", () => {
  const p = light(['api/tests/it/runs_store.rs'])
  eq([p.scope, p.packages, p.itModules, p.live], ['packages', ['talaria-api'], ['runs_store'], true])
  eq(light(['api/tests/it/support/mod.rs']).itModules, ['*'], 'the support module runs them all')
})
await check('light: a crate the live modules reach runs them, and its ignored tests', () => {
  const p = light(['api/crates/talaria-runs-store/src/lib.rs'])
  ok(p.packages.includes('talaria-runs-store') && p.packages.includes('talaria-api'), JSON.stringify(p.packages))
  ok(p.itModules.includes('runs_store') && p.live, JSON.stringify(p))
})
await check('light: api/src is the root package and every module', () => {
  const p = light(['api/src/main.rs'])
  eq([p.packages, p.itModules], [['talaria-api'], ['*']])
})
await check('light: the lockfile maps to the members it changed', () => {
  const p = light(['api/Cargo.lock'], { baseText: () => LOCK_BASE, headText: () => LOCK_HEAD })
  eq(p.scope, 'packages')
  ok(p.packages.includes('talaria-work-pulse') && p.itModules[0] === '*', JSON.stringify(p))
  eq(light(['api/Cargo.lock']).scope, 'workspace', 'a lockfile with no base to compare widens')
})
await check("light: every module is `*` alone, never `*` beside a module's own stem", () => {
  // talaria-db is reached through the shared support module (every module);
  // the live module's own file must not add its stem beside the `*`, which
  // ci.yml's normalize step would refuse and widen to the workspace.
  const p = light(['api/crates/talaria-db/src/lib.rs', 'api/tests/it/runs_store.rs'])
  eq([p.scope, p.itModules], ['packages', ['*']])
})
await check('light: a file another package reads by its path tests that reader too', () => {
  const sandbox = light(['api/crates/talaria-fitness-toolbox/src/sandbox.rs'])
  ok(sandbox.packages.includes('talaria-fitness-toolbox') && sandbox.packages.includes('talaria-harness-defs'),
    `talaria-harness-defs include_str!s the toolbox's sandbox.rs: ${JSON.stringify(sandbox.packages)}`)
  const grants = light(['api/tests/it/fixtures/app-grants.json'])
  ok(grants.packages.includes('talaria-users') && grants.packages.includes('talaria-api'),
    `talaria-users include_str!s the grants fixture: ${JSON.stringify(grants.packages)}`)
  eq(grants.itModules, ['*'], 'a fixture still runs every live module')
  eq(light(['api/tests/it/runs_store.rs']).packages, ['talaria-api'], 'a live module file names no other reader')
})
await check('light: an input outside api/ maps to the crates that read it', () =>
  ok(light(['scripts/hermes-toolsets.json']).packages.includes('talaria-agent-defs'), 'talaria-agent-defs reads it'))
await check('light widens on the toolchain, the cargo config, the job files and an unmapped crate', () => {
  for (const f of ['api/rust-toolchain.toml', 'api/.cargo/config.toml', '.github/workflows/ci.yml', 'scripts/surfaces.mjs', '.github/actions/mold/action.yml', 'api/crates/no-such-crate/src/lib.rs']) {
    eq(light([f]).scope, 'workspace', f)
  }
})
await check('integration, full, and a light run on every surface test the workspace', () => {
  for (const mode of ['integration', 'full']) {
    eq(planApi({ mode, all: false, files: ['api/crates/talaria-error/src/lib.rs'], root: ROOT, baseText: noBase }).scope, 'workspace', mode)
  }
  eq(planApi({ mode: 'light', all: true, files: [], root: ROOT, baseText: noBase }).scope, 'workspace', 'all')
})
await check('no api file plans nothing', () => eq(light(['docs/a.md']).packages, []))

await check('binQualifies: a same-repo PR, a push to testing, an offload; never a fork or a release', () => {
  ok(binQualifies({ event: 'pull_request', repo: 'o/r', headRepo: 'o/r' }), 'same-repo PR')
  ok(!binQualifies({ event: 'pull_request', repo: 'o/r', headRepo: 'fork/r' }), 'fork PR')
  ok(!binQualifies({ event: 'pull_request', repo: 'o/r' }), 'a PR with no head repo')
  ok(binQualifies({ event: 'push', ref: 'refs/heads/testing' }), 'push to testing')
  ok(!binQualifies({ event: 'push', ref: 'refs/heads/rc' }), 'push to rc')
  ok(binQualifies({ event: 'push', ref: 'refs/heads/ci/wt/x', calledRef: 'abc', calledMode: 'light' }), 'offload light')
  ok(binQualifies({ event: 'push', ref: 'refs/heads/ci/full/wt/x', calledRef: 'abc', calledMode: 'full' }), 'offload full')
  ok(!binQualifies({ event: 'schedule', ref: 'refs/heads/main', calledRef: 'rc', calledMode: '' }), 'release')
})
await check('migrationBaseline: the branch the change lands on', () => {
  const none = fakeGit({})
  eq(migrationBaseline({ event: 'pull_request', baseRef: 'testing' }, none), 'origin/testing')
  eq(migrationBaseline({ event: 'pull_request', baseRef: 'rc' }, none), 'origin/rc')
  eq(migrationBaseline({ event: 'pull_request', baseRef: 'bad..ref' }, none), 'origin/main')
  eq(migrationBaseline({ event: 'push', ref: 'refs/heads/testing' }, none), 'origin/rc')
  eq(migrationBaseline({ event: 'push', ref: 'refs/heads/rc' }, none), 'origin/main')
  eq(migrationBaseline({ event: 'push', ref: 'refs/heads/main' }, none), 'origin/main')
  eq(migrationBaseline({ event: 'schedule', ref: 'refs/heads/main', calledRef: 'rc', calledMode: '' }, none), 'origin/main', 'release')
  eq(migrationBaseline({ event: 'push', ref: 'refs/heads/ci/wt/x', calledRef: 'abc', calledMode: 'light' }, twoBases(3, 9)), 'origin/testing', 'offload')
  eq(migrationBaseline({ event: 'push', ref: 'refs/heads/ci/full/hotfix/x', calledRef: 'abc', calledMode: 'full' }, twoBases(40, 2)), 'origin/rc', 'a hotfix offload')
  eq(migrationBaseline({ event: 'push', ref: 'refs/heads/ci/wt/x', calledRef: 'abc', calledMode: 'light' }, none), 'origin/main', 'no base resolves')
})
await check('planCi carries the api plan: a light same-repo PR on one crate builds no binary', async () => {
  const p = await planCi({ event: 'pull_request', baseRef: 'testing', repo: 'o/r', headRepo: 'o/r', root: ROOT }, prMerge(['api/crates/talaria-error/src/lib.rs']), fetchThrows)
  eq([p.api.scope, p.api.packages, p.api.live, p.api.bin, p.migrationBaseline], ['packages', ['talaria-error'], false, false, 'origin/testing'])
})
await check('planCi: a light same-repo PR on a live module uploads the binary; a fork does not', async () => {
  const p = await planCi({ event: 'pull_request', baseRef: 'testing', repo: 'o/r', headRepo: 'o/r', root: ROOT }, prMerge(['api/tests/it/runs_store.rs']), fetchThrows)
  eq([p.api.packages, p.api.itModules, p.api.bin], [['talaria-api'], ['runs_store'], true])
  const fork = await planCi({ event: 'pull_request', baseRef: 'testing', repo: 'o/r', headRepo: 'f/r', root: ROOT }, prMerge(['api/tests/it/runs_store.rs']), fetchThrows)
  eq(fork.api.bin, false, 'a fork')
})
await check('planCi: a full PR into rc tests the workspace and builds the binary', async () => {
  const p = await planCi({ event: 'pull_request', baseRef: 'rc', repo: 'o/r', headRepo: 'o/r', root: ROOT }, prMerge(['api/crates/talaria-error/src/lib.rs']), fetchThrows)
  eq([p.mode, p.api.scope, p.api.itModules, p.api.live, p.api.bin, p.migrationBaseline], ['full', 'workspace', ['*'], true, true, 'origin/rc'])
})
await check('planCi: no api file is no binary, whatever the event', async () => {
  const p = await planCi({ event: 'pull_request', baseRef: 'rc', repo: 'o/r', headRepo: 'o/r', root: ROOT }, prMerge(['docs/a.md']), fetchThrows)
  eq(p.api.bin, false)
})
await check('planCi: a lockfile edit reads the base from git, the merge-base for a three-dot range', async () => {
  const git = fakeGit({
    [DIFF('b45e...HEAD')]: 'api/Cargo.lock',
    'merge-base b45e HEAD': 'mb0',
    'show mb0:api/Cargo.lock': new Error('fatal: path does not exist'),
  })
  const p = await planCi({ event: 'pull_request', baseRef: 'testing', baseSha: 'b45e', root: ROOT }, git, fetchThrows)
  ok(git.calls.includes('show mb0:api/Cargo.lock'), `asked git for the merge-base's lockfile: ${git.calls.join(', ')}`)
  eq(p.api.scope, 'workspace', 'an unreadable base widens')
})

// ── The `ci` CLI's outputs ──────────────────────────────────────────────────
await check('the CLI emits every key, even with an empty diff', async () => {
  const { output, summary } = await ciOutputs(
    { EVENT: 'pull_request', BASE_REF: 'testing', BASE_SHA: 'b' },
    fakeGit({ 'rev-parse --verify --quiet HEAD^2': 'h', 'rev-parse HEAD^1': 'b1', [DIFF('HEAD^1..HEAD')]: '' }),
    fetchThrows,
  )
  eq(output.map((l) => l.split('=')[0]), OUTPUT_KEYS, 'keys, in order')
  const kv = Object.fromEntries(output.map((l) => [l.slice(0, l.indexOf('=')), l.slice(l.indexOf('=') + 1)]))
  eq([kv.mode, kv.range, kv.base], ['light', 'HEAD^1..HEAD', 'b1'])
  for (const n of SURFACE_NAMES) eq(kv[n], 'false', n)
  eq(API_KEYS.map((k) => kv[k]), ['packages', '', '', 'false', '', 'false', 'false'], 'the api plan of an empty diff')
  eq(kv.migration_baseline, 'origin/testing', 'the baseline')
  ok(kv.reason.length > 0, 'a reason')
  ok(summary.includes('| mode | `light` |') && summary.includes('`HEAD^1..HEAD`'), 'the summary table names the mode and range')
})
await check('the CLI never emits an empty mode or a newline in a value', async () => {
  const { output } = await ciOutputs({ EVENT: 'push', REF: 'refs/heads/rc', REPO: 'o/r' }, fakeGit({}), fetchThrows)
  ok(/^mode=\S+$/.test(output[0]), output[0])
  for (const l of output) ok(!l.includes('\n'), `one line: ${l}`)
  eq(output.filter((l) => new RegExp(`^(${SURFACE_NAMES.join('|')})=true$`).test(l)).length, SURFACE_NAMES.length, 'all on')
})
await check('the CLI reads HEAD_REF, HEAD_REPO and REPO: a promotion emits mode=promotion, image=false and every surface off', async () => {
  const env = { EVENT: 'pull_request', BASE_REF: 'main', HEAD_REF: 'rc', HEAD_REPO: 'o/r', REPO: 'o/r', BASE_SHA: 'm0' }
  const { output, summary } = await ciOutputs(env, promoGit('t1', 't1'), fetchThrows)
  const kv = Object.fromEntries(output.map((l) => [l.slice(0, l.indexOf('=')), l.slice(l.indexOf('=') + 1)]))
  eq(output.map((l) => l.split('=')[0]), OUTPUT_KEYS, 'keys, in order')
  eq([kv.mode, kv.image, kv.api_bin, kv.api_it], ['promotion', 'false', 'false', 'false'])
  for (const n of SURFACE_NAMES) eq(kv[n], 'false', n)
  ok(summary.includes('| mode | `promotion` |') && summary.includes("the merge tree is rc's tip"), summary)
  const fork = await ciOutputs({ ...env, HEAD_REPO: 'fork/r' }, promoGit('t1', 't1'), fetchThrows)
  ok(fork.output.includes('mode=full') && fork.output.includes('image=true'), `a fork's rc: ${fork.output.join(' ')}`)
})

// ── Drift guards: the workflows run this file and read every key ────────────
const ciYml = readFileSync(join(ROOT, '.github/workflows/ci.yml'), 'utf8')
await check('ci.yml materializes this planner and runs `ci`', () => {
  ok(ciYml.includes('scripts/surfaces.mjs'), 'ci.yml no longer names scripts/surfaces.mjs')
  ok(/surfaces\.mjs"? ci\b/.test(ciYml), 'ci.yml no longer runs `surfaces.mjs ci`')
})
await check("ci.yml's changes job exposes the mode, the range and every surface", () => {
  for (const key of ['mode', 'base', 'range', ...SURFACE_NAMES]) {
    ok(ciYml.includes(`${key}: \${{ steps.plan.outputs.${key} }}`), `changes.outputs lacks ${key}`)
  }
  const loop = /for key in ([a-z0-9 ]+); do/.exec(ciYml)
  ok(loop, 'the normalize step has no `for key in …` loop')
  eq(loop[1].trim().split(/\s+/), SURFACE_NAMES, 'the keys the normalize step defaults to true')
})
await check("ci.yml's changes job exposes and normalizes the api plan and the migration baseline", () => {
  for (const key of [...API_KEYS, 'migration_baseline']) {
    ok(ciYml.includes(`${key}: \${{ steps.plan.outputs.${key} }}`), `changes.outputs lacks ${key}`)
    ok(ciYml.includes(`$(get ${key})`), `the normalize step does not read ${key}`)
  }
  for (const key of ['api_scope', 'api_packages', 'api_lib_packages', 'api_it_modules', 'api_it', 'api_live', 'api_bin', 'migration_baseline', 'db']) {
    ok(ciYml.includes(`needs.changes.outputs.${key}`), `no job reads ${key}`)
  }
  ok(ciYml.includes('HEAD_REPO: ${{ github.event.pull_request.head.repo.full_name }}'), 'the plan step does not pass HEAD_REPO')
})
await check("ci.yml's image job builds the app image when its surface moved, and pushes nothing", () => {
  const job = /\n {2}image:\n([\s\S]*?)(?=\n {2}[a-z][a-z0-9_-]*:\n|$)/.exec(ciYml)
  ok(job, 'ci.yml has no `image` job')
  const body = job[1]
  ok(/^ {4}name: app image builds \(docker\)$/m.test(body), 'the image job is not named `app image builds (docker)`')
  ok(/^ {4}if: needs\.changes\.outputs\.image == 'true'$/m.test(body), 'the image job does not run on the image surface')
  ok(/docker build\b/.test(body) && !/docker (push|login)\b/.test(body), 'the image job must build and never log in or push')
  ok(/persist-credentials: false/.test(body), 'the image job keeps no credential in the checkout')
})
await check("ci.yml's header lists the contexts docs/BRANCHES.md requires per branch, and each is a job", () => {
  // The header's REQUIRED CONTEXTS table: `<context>  <branches…>` rows.
  const block = /^# REQUIRED CONTEXTS\.[\s\S]*?\n#\s*\n([\s\S]*?)\n#\s*\n/m.exec(ciYml)
  ok(block, 'the header has no REQUIRED CONTEXTS table')
  const header = new Map(['testing', 'rc', 'main'].map((b) => [b, new Set()]))
  for (const raw of block[1].split('\n')) {
    const row = /^#\s{3,}(.+?)\s{2,}((?:testing|rc|main)(?: (?:testing|rc|main))*)\s*$/.exec(raw)
    ok(row, `not a table row: ${raw}`)
    for (const b of row[2].split(' ')) header.get(b).add(row[1])
  }
  const flowYml = readFileSync(join(ROOT, '.github/workflows/flow.yml'), 'utf8')
  const names = new Set([...ciYml.matchAll(/^ {4}name: (.+)$/gm), ...flowYml.matchAll(/^ {4}name: (.+)$/gm)].map((m) => m[1].trim()))
  const doc = readFileSync(join(ROOT, 'docs/BRANCHES.md'), 'utf8')
  // testing: the testing-checks ruleset. rc and main: the classic protection
  // table's "Required checks" row — rc's cell, then main's (`none` is empty).
  const ruleset = /^\{"name":"testing-checks".*$/m.exec(doc)
  ok(ruleset, 'docs/BRANCHES.md has no testing-checks ruleset')
  const classic = /^\| Required checks[^|]*\|([^|]*)\|([^|]*)\|\s*$/m.exec(doc)
  ok(classic, "docs/BRANCHES.md's protection table has no Required checks row")
  const cell = (c) => [...c.matchAll(/`([^`]+)`/g)].map((m) => m[1])
  const required = new Map([
    ['testing', JSON.parse(ruleset[0]).rules[0].parameters.required_status_checks.map((c) => c.context)],
    ['rc', cell(classic[1])],
    ['main', cell(classic[2])],
  ])
  for (const [branch, set] of header) {
    for (const ctx of set) ok(names.has(ctx), `${ctx} is in the header but is no job name in ci.yml or flow.yml`)
    eq([...set].sort(), required.get(branch).sort(), `${branch}'s required checks`)
  }
})
await check('ci-offload.yml calls ci.yml with a mode, under a name that is not CI', () => {
  const off = readFileSync(join(ROOT, '.github/workflows/ci-offload.yml'), 'utf8')
  ok(/^name: CI offload$/m.test(off), 'the workflow is named `CI offload`')
  ok(off.includes('uses: ./.github/workflows/ci.yml'), 'it calls ci.yml')
  ok(/branches: \['ci\/\*\*'\]/.test(off), 'it runs on ci/** pushes')
  ok(/mode: .*ci\/full\//.test(off), 'ci/full/ picks full')
})

if (failed) {
  console.error(`surfaces: ${failed} of ${ran} checks failed`)
  process.exit(1)
}
console.log(`surfaces: ${ran} checks, all clean`)
