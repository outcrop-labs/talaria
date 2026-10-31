// `talaria-app check`: the app's manifest held to the host's rules, a
// typecheck against the kit's declarations, and the app's own tests.
//
// The typecheck resolves exactly what an app may import: @talaria/sdk and its
// /server and /testing entries (the kit's declarations), and the shared
// client packages (svelte, @lucide/svelte, @tanstack/svelte-query). Anything
// else — zod, a package the app's author has locally — does not resolve,
// because it does not resolve on the host either: an app has no node_modules
// of its own there. (`z` comes from '@talaria/sdk/server'.)
import { spawn, spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, writeFileSync } from 'node:fs'
import { delimiter, join } from 'node:path'
import { KIT_PATHS, SHARED_TYPE_PACKAGES } from './kit'

export interface CheckStep {
  name: 'manifest' | 'typecheck' | 'tests'
  status: 'ok' | 'failed' | 'skipped'
  summary: string
  /** The tool's own output, shown when the step failed (or with --verbose). */
  output: string
}

/** The compiler options the host typechecks its own apps with
 *  (ui/tsconfig.json), with the module map pointed at the kit. */
export function kitTsconfig(kit: string, app: string): Record<string, unknown> {
  const nm = join(kit, 'node_modules')
  const paths: Record<string, string[]> = {
    '@talaria/sdk': [join(kit, KIT_PATHS.types.client)],
    '@talaria/sdk/server': [join(kit, KIT_PATHS.types.server)],
    '@talaria/sdk/testing': [join(kit, KIT_PATHS.types.testing)],
    vitest: [join(nm, 'vitest')],
  }
  for (const pkg of SHARED_TYPE_PACKAGES) {
    paths[pkg] = [join(nm, pkg)]
    paths[`${pkg}/*`] = [join(nm, pkg, '*')]
  }
  return {
    compilerOptions: {
      target: 'ES2022',
      lib: ['ES2023', 'DOM', 'DOM.Iterable'],
      module: 'ESNext',
      moduleResolution: 'bundler',
      strict: true,
      noUnusedLocals: true,
      noUnusedParameters: true,
      noUncheckedIndexedAccess: true,
      esModuleInterop: true,
      skipLibCheck: true,
      verbatimModuleSyntax: true,
      allowImportingTsExtensions: true,
      noEmit: true,
      types: ['node'],
      typeRoots: [join(nm, '@types')],
      paths,
    },
    include: [join(app, '**/*.ts'), join(app, '**/*.svelte')],
    exclude: [join(app, 'node_modules'), join(app, '.talaria'), join(app, '**/dist')],
  }
}

/** The vitest config an app's tests run under: the app's own `*.test.ts`,
 *  with the SDK entries a test may import mapped to the kit's runtime — the
 *  same `@talaria/sdk/testing` the host's own app tests use. */
export function kitVitestConfig(kit: string, app: string, work: string): string {
  const config = {
    root: app,
    cacheDir: join(work, '.vitest'),
    resolve: {
      alias: [
        { find: '@talaria/sdk/server', replacement: join(kit, KIT_PATHS.runtime.server) },
        { find: '@talaria/sdk/testing', replacement: join(kit, KIT_PATHS.runtime.testing) },
        { find: '@talaria/sdk', replacement: join(kit, KIT_PATHS.runtime.client) },
        { find: 'vitest', replacement: join(kit, 'node_modules', 'vitest', 'dist', 'index.js') },
      ],
    },
    server: { fs: { allow: [app, kit, work] } },
    test: {
      include: ['**/*.test.ts'],
      exclude: ['**/node_modules/**', '**/.talaria/**', '**/.git/**', '**/dist/**'],
      environment: 'node',
      passWithNoTests: true,
    },
  }
  // Exact-match aliases: an `alias` entry with a string `find` is a prefix
  // match in Vite, and '@talaria/sdk' would take '@talaria/sdk/server'.
  const aliases = config.resolve.alias
    .map((a) => `{ find: /^${a.find.replace(/[/.@]/g, (c) => `\\${c}`)}$/, replacement: ${JSON.stringify(a.replacement)} }`)
    .join(',\n      ')
  return `// Written by talaria-app check. Regenerated on every run.
export default {
  root: ${JSON.stringify(config.root)},
  cacheDir: ${JSON.stringify(config.cacheDir)},
  resolve: {
    alias: [
      ${aliases}
    ],
  },
  server: ${JSON.stringify(config.server)},
  test: ${JSON.stringify(config.test)},
}
`
}

/** The JavaScript runtime the checkers run on: a node that answers from the
 *  app's directory when the sandbox has one (svelte-check and vitest are
 *  node programs; in a workbench job `node` is a mise shim, which answers
 *  only where the repo pins it), else the bun running this. */
export function nodeRuntime(cwd: string, env: NodeJS.ProcessEnv = process.env): string {
  for (const dir of (env.PATH ?? '').split(delimiter)) {
    const node = dir ? join(dir, 'node') : ''
    if (node && existsSync(node) && spawnSync(node, ['--version'], { cwd, env, stdio: 'ignore', timeout: 20_000 }).status === 0) return node
  }
  return process.execPath
}

export function run(cmd: string, args: string[], cwd: string): Promise<{ code: number; output: string }> {
  return new Promise((resolveRun) => {
    const child = spawn(cmd, args, { cwd, env: { ...process.env, FORCE_COLOR: '0', NO_COLOR: '1' }, stdio: ['ignore', 'pipe', 'pipe'] })
    let output = ''
    child.stdout.on('data', (d: Buffer) => (output += d.toString()))
    child.stderr.on('data', (d: Buffer) => (output += d.toString()))
    child.on('error', (e) => resolveRun({ code: 127, output: `${output}${e.message}\n` }))
    child.on('close', (code) => resolveRun({ code: code ?? 1, output }))
  })
}

/** svelte-check over the app with the kit's tsconfig. */
export async function typecheck(kit: string, app: string, work: string): Promise<CheckStep> {
  mkdirSync(work, { recursive: true })
  const tsconfig = join(work, 'tsconfig.json')
  writeFileSync(tsconfig, `${JSON.stringify(kitTsconfig(kit, app), null, 2)}\n`)
  const bin = join(kit, 'node_modules', 'svelte-check', 'bin', 'svelte-check')
  const { code, output } = await run(nodeRuntime(app), [bin, '--workspace', app, '--tsconfig', tsconfig, '--threshold', 'error', '--output', 'human'], app)
  const tail = output.trim().split('\n').reverse().find((l) => /svelte-check found|COMPLETED|error/i.test(l)) ?? ''
  return { name: 'typecheck', status: code === 0 ? 'ok' : 'failed', summary: tail.replace(/^.*svelte-check /, 'svelte-check ') || (code === 0 ? 'no errors' : `svelte-check exited ${code}`), output }
}

/** The app's own `*.test.ts`, under vitest. */
export async function tests(kit: string, app: string, work: string): Promise<CheckStep> {
  mkdirSync(work, { recursive: true })
  const config = join(work, 'vitest.config.mjs')
  writeFileSync(config, kitVitestConfig(kit, app, work))
  const bin = join(kit, 'node_modules', 'vitest', 'vitest.mjs')
  const { code, output } = await run(nodeRuntime(app), [bin, 'run', '--config', config], app)
  const tally = output.split('\n').map((l) => l.trim()).filter((l) => /^Tests\s/.test(l) || /No test files found/.test(l))
  return {
    name: 'tests',
    status: code === 0 ? 'ok' : 'failed',
    summary: tally.length ? tally.join(' · ').replace(/\s+/g, ' ') : code === 0 ? 'no test files' : `vitest exited ${code}`,
    output,
  }
}
