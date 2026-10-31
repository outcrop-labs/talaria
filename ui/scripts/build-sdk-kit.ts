// Build the SDK kit the instance serves at GET /api/sdk/kit.tgz.
//
//   cd ui && bun scripts/build-sdk-kit.ts        (the last step of `bun run build`)
//
// Writes dist/sdk-kit/kit.tgz and dist/sdk-kit/kit.json. Runs after the client
// build, whose dist/client/runtime/build-id is the stamp the kit carries: the
// kit is versioned with the host build it came from. What goes in, and why, is
// ui/src/sdk-kit/kit.ts. Bun, not node: the runtime is bundled with Bun.build.
//
// The dev server runs this on demand when a kit is asked for and none matches
// the current sources (ui/src/server/sdk-kit.ts).
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { tmpdir } from 'node:os'
import { dirname, join, relative, resolve } from 'node:path'
import { createRequire } from 'node:module'
import { clientSpecifiers, hostBuildStamp, serverSpecifiers } from '../src/app-runtime/specifiers'
import { SDK_VERSION } from '../src/server/app-build/manifest'
import { KIT_FORMAT, KIT_PACKAGES, KIT_PATHS, kitVersionOk, type KitManifest } from '../src/sdk-kit/kit'
import { kitSourceDigest, rewriteHostAliases, sdkSourceClosure, tarGz, type TarEntry } from '../src/sdk-kit/pack'

const UI = resolve(import.meta.dir, '..')
const SRC = join(UI, 'src')
const OUT = join(UI, 'dist/sdk-kit')
const require = createRequire(import.meta.url)
const t0 = Date.now()

/** Stand-ins for the host's service clients. The kit runs the host's build,
 *  probe and runner, which read settings through them; with no database
 *  behind them every read is empty — every setting its default, as on a
 *  fresh instance — and every write goes nowhere. */
const STUBS: Record<string, string> = {
  postgres: `
const empty = () => Object.assign([], { count: 0, columns: [] })
const pending = () => {
  const p = Promise.resolve(empty())
  return Object.assign(p, { values: () => p, raw: () => p, execute: () => p, simple: () => p, cancel: () => {} })
}
export default function postgres() {
  const sql = (..._args) => pending()
  sql.json = (v) => v
  sql.array = (v) => v
  sql.unsafe = () => pending()
  sql.file = () => pending()
  sql.begin = async (...args) => { const fn = args.find((a) => typeof a === 'function'); return fn ? fn(sql) : undefined }
  sql.reserve = async () => Object.assign(sql, { release: () => {} })
  sql.end = async () => {}
  sql.listen = async () => ({ unlisten: async () => {} })
  sql.notify = async () => {}
  return sql
}
`,
  ioredis: `
const nothing = async () => null
export default class Redis {
  constructor() {
    return new Proxy(this, {
      get(target, key) {
        if (key === 'then') return undefined
        if (key in target) return target[key]
        if (key === 'on' || key === 'once' || key === 'off') return () => target
        if (key === 'status') return 'ready'
        return nothing
      },
    })
  }
}
export { Redis }
`,
  nodemailer: `
export default { createTransport: () => ({ sendMail: async () => { throw new Error('the SDK kit sends no mail') }, verify: async () => false }) }
export const createTransport = () => ({ sendMail: async () => { throw new Error('the SDK kit sends no mail') }, verify: async () => false })
`,
}

const fail = (msg: string): never => {
  console.error(`[sdk-kit] ${msg}`)
  process.exit(1)
}

// The stamp: the one the client build wrote (so the kit says the same version
// the host's own app builds are keyed on), else what that build would write.
process.chdir(UI)
const stampFile = join(UI, KIT_PATHS.buildId)
const hostBuild = existsSync(stampFile) ? readFileSync(stampFile, 'utf8').trim() || hostBuildStamp() : hostBuildStamp()

const scratch = mkdtempSync(join(tmpdir(), 'talaria-sdk-kit-'))
const stage = join(scratch, 'kit')
mkdirSync(stage, { recursive: true })
const files: TarEntry[] = []
const add = (path: string, data: string | Uint8Array, mode = 0o644) => {
  files.push({ path, data, mode })
}

try {
  // ── 1. Declarations ──────────────────────────────────────────────────────
  // The files the three entries reach, copied apart so svelte2tsx emits for
  // them and nothing else, then every `@/` import made relative: an app gets
  // no `@/` path, so it cannot typecheck against a host internal its build
  // could never import.
  const lib = join(scratch, 'types-src')
  const closure = sdkSourceClosure(SRC)
  for (const rel of closure) {
    mkdirSync(dirname(join(lib, 'src', rel)), { recursive: true })
    cpSync(join(SRC, rel), join(lib, 'src', rel))
  }
  writeFileSync(
    join(lib, 'tsconfig.json'),
    JSON.stringify({
      compilerOptions: {
        target: 'ES2022',
        lib: ['ES2023', 'DOM', 'DOM.Iterable'],
        module: 'ESNext',
        moduleResolution: 'bundler',
        strict: true,
        skipLibCheck: true,
        verbatimModuleSyntax: true,
        allowImportingTsExtensions: true,
        types: ['node', 'svelte'],
        baseUrl: '.',
        paths: { '@/*': ['./src/*'] },
      },
      include: ['src'],
    }),
  )
  // The copy resolves packages from the ui's own node_modules.
  const { symlinkSync } = await import('node:fs')
  symlinkSync(join(UI, 'node_modules'), join(lib, 'node_modules'))
  const declared = join(scratch, 'types-out')
  const { emitDts } = (await import('svelte2tsx')) as typeof import('svelte2tsx')
  await emitDts({
    libRoot: lib,
    declarationDir: declared,
    svelteShimsPath: require.resolve('svelte2tsx/svelte-shims-v4.d.ts'),
    tsconfig: join(lib, 'tsconfig.json'),
  })
  const walk = (dir: string): string[] =>
    readdirSync(dir).flatMap((n) => {
      const p = join(dir, n)
      return statSync(p).isDirectory() ? walk(p) : [p]
    })
  let declarations = 0
  for (const file of walk(join(declared, 'src'))) {
    const rel = relative(join(declared, 'src'), file)
    add(`types/src/${rel}`, rewriteHostAliases(readFileSync(file, 'utf8'), rel))
    declarations++
  }
  for (const entry of Object.values(KIT_PATHS.types)) {
    if (!files.some((f) => f.path === entry)) fail(`svelte2tsx emitted no ${entry}`)
  }

  // ── 2. The runtime ───────────────────────────────────────────────────────
  // The CLI, the @talaria/sdk/server shim an app's server, mcp and harness
  // chunks link to (where the build recipe looks for it), and the testing
  // entry — one Bun.build with shared chunks, so the shim the app's code
  // imports and the runner the CLI drives are the same modules (one zod, one
  // defineHarness). The packages the kit installs stay external and resolve
  // from its node_modules; the host's database and Redis clients are
  // replaced by stand-ins with no server behind them.
  // At a fixed path: Bun names each module's source in a comment, and a
  // temporary directory's name would make every build's bytes differ.
  const entries = join(UI, 'dist/.sdk-kit-entries')
  rmSync(entries, { recursive: true, force: true })
  const entry = (rel: string, code: string) => {
    mkdirSync(dirname(join(entries, rel)), { recursive: true })
    writeFileSync(join(entries, rel), code)
    return join(entries, rel)
  }
  const ui = (rel: string) => JSON.stringify(join(SRC, rel))
  const built = await Bun.build({
    entrypoints: [
      entry('lib/cli.ts', `import { run } from ${ui('sdk-kit/cli.ts')}\nawait run(import.meta.url)\n`),
      entry('dist/server/runtime/rt-talaria-sdk-server.ts', `export * from ${ui('sdk/server.ts')}\n`),
      entry('lib/sdk-testing.ts', `export * from ${ui('sdk/testing.ts')}\n`),
      entry(
        'lib/sdk-client.ts',
        `throw new Error("@talaria/sdk (the client entry) does not run under talaria-app check: the app's tests run server.ts, mcp.ts and harnesses/ under node. A surface is checked by talaria-app build, which compiles it with the host's recipe.")\nexport {}\n`,
      ),
    ],
    root: entries,
    outdir: stage,
    target: 'node',
    format: 'esm',
    splitting: true,
    naming: { entry: '[dir]/[name].[ext]', chunk: 'lib/[name]-[hash].[ext]', asset: 'lib/[name]-[hash].[ext]' },
    external: KIT_PACKAGES.flatMap((p) => [p, `${p}/*`]),
    tsconfig: join(UI, 'tsconfig.json'),
    plugins: [
      {
        name: 'talaria-kit-no-services',
        setup(b) {
          b.onResolve({ filter: /^(postgres|ioredis|nodemailer)$/ }, (args) => ({ path: args.path, namespace: 'kit-stub' }))
          b.onLoad({ filter: /.*/, namespace: 'kit-stub' }, (args) => ({
            loader: 'js',
            contents: STUBS[args.path] ?? fail(`no stub for ${args.path}`),
          }))
        },
      },
    ],
  })
  rmSync(entries, { recursive: true, force: true })
  if (!built.success) fail(`the runtime did not bundle:\n${built.logs.map(String).join('\n')}`)
  for (const file of walk(stage)) add(relative(stage, file), readFileSync(file))
  add(
    KIT_PATHS.cli,
    `#!/bin/sh
# talaria-app — the Talaria SDK kit's CLI (\`talaria-app --help\`). Runs on bun.
here=$(dirname "$(readlink -f "$0")")
exec bun "$here/../lib/cli.js" "$@"
`,
    0o755,
  )

  // ── 3. What the build recipe reads from its root ──────────────────────────
  add('src/styles.css', readFileSync(join(SRC, 'styles.css')))
  add('svelte.config.ts', readFileSync(join(UI, 'svelte.config.ts')))
  add(KIT_PATHS.buildId, `${hostBuild}\n`)
  add(KIT_PATHS.schema, readFileSync(join(UI, '..', 'docs/sdk/talaria.schema.json')))

  // ── 4. The packages, pinned to the host's own ────────────────────────────
  const packages: Record<string, string> = {}
  for (const name of KIT_PACKAGES) {
    const pj = join(UI, 'node_modules', name, 'package.json')
    if (!existsSync(pj)) fail(`${name} is not installed in ui/node_modules`)
    packages[name] = (JSON.parse(readFileSync(pj, 'utf8')) as { version: string }).version
  }
  // The version: the stamp, and a digest of everything else the kit holds —
  // the stamp alone does not move when the SDK does.
  files.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0))
  const contents = createHash('sha256')
  for (const f of files) {
    contents.update(f.path)
    contents.update('\0')
    contents.update(typeof f.data === 'string' ? f.data : Buffer.from(f.data))
    contents.update('\0')
  }
  for (const [name, v] of Object.entries(packages)) contents.update(`${name}@${v}\0`)
  const version = `${hostBuild}-${contents.digest('hex').slice(0, 8)}`
  if (!kitVersionOk(version)) fail(`"${version}" is not a usable kit version`)
  const manifest: KitManifest = {
    format: KIT_FORMAT,
    version,
    hostBuild,
    sdk: SDK_VERSION,
    specifiers: { client: clientSpecifiers(), server: serverSpecifiers() },
    packages,
  }
  add(KIT_PATHS.manifest, `${JSON.stringify(manifest, null, 2)}\n`)
  add(
    'package.json',
    `${JSON.stringify(
      {
        name: '@talaria/sdk-kit',
        private: true,
        description: `The Talaria SDK kit ${version}: @talaria/sdk ${SDK_VERSION} declarations, the host's app build recipe, and the talaria-app CLI.`,
        type: 'module',
        bin: { 'talaria-app': KIT_PATHS.cli },
        dependencies: packages,
      },
      null,
      2,
    )}\n`,
  )
  add(
    'README.md',
    `# Talaria SDK kit ${version}

@talaria/sdk ${SDK_VERSION}, from host build ${hostBuild}. Served by the instance at
\`GET /api/sdk/kit.tgz\`; see docs/sdk/outside-the-host.md in the Talaria repository.

    bun install                   # once, in this directory
    bin/talaria-app --help

Nothing in here is a secret, and nothing in here calls a model except
\`talaria-app eval --gateway\`, which goes through the instance's gateway.
`,
  )

  // ── 5. The archive ───────────────────────────────────────────────────────
  files.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0))
  const tgz = tarGz(files)
  const digest = createHash('sha256').update(tgz).digest('hex')
  rmSync(OUT, { recursive: true, force: true })
  mkdirSync(OUT, { recursive: true })
  writeFileSync(join(OUT, 'kit.tgz'), tgz)
  writeFileSync(join(OUT, 'kit.json'), `${JSON.stringify({ ...manifest, digest, bytes: tgz.length, sources: kitSourceDigest(UI) }, null, 2)}\n`)
  console.log(
    `[sdk-kit] ${version}: ${files.length} files (${declarations} declarations), ${(tgz.length / 1024).toFixed(0)} KB → dist/sdk-kit/kit.tgz in ${Date.now() - t0} ms`,
  )
} finally {
  rmSync(scratch, { recursive: true, force: true })
}

