// talaria-app — the SDK kit's command line. Bundled into the kit as
// bin/talaria-app (ui/scripts/build-sdk-kit.ts); runs on bun, in the sandbox
// that fetched the kit, never on the host.
import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { captureConsole, formatBuildReport, kitBuild } from './build'
import { tests, typecheck, type CheckStep } from './check'
import { findEvalSession, formatEvalReport, gatewayModels, runEvals, type GatewayTarget } from './eval'
import type { KitManifest } from './kit'
import { scaffoldApp } from './scaffold'
import { enterWorkspace, findAppDir, readAppManifest, readKitManifest, slugFor, workDirFor } from './workspace'

export const USAGE = `talaria-app — build a Talaria app outside the host, with the host's own recipe

usage: talaria-app <command> [dir] [flags]

commands
  scaffold [dir]  write a new app: the skeleton \`talaria app new\` writes, plus a
                  .gitignore and a README          --name <n> --icon <i> --harness
  check [dir]     talaria.json against the host's rules, a typecheck against the
                  kit's declarations, and the app's own *.test.ts
                                                   --no-typecheck --no-tests
  build [dir]     the host's build recipe and load probe, reported per surface
                  and per harness as Manage → Apps would
  eval [dir]      every harness fixture, against harnesses/recorded/ replies, or
                  through the Talaria gateway      --gateway --model <id> --record
                                                   --harness <id|file> --case <name>
                                                   --url <origin> --key <key>
  version         the kit's version and the SDK version it serves

flags for every command
  --slug <slug>   build as this slug (default: talaria.json's, else the directory's)
  --json          the report as JSON
  --verbose       also print what the host's code logged, and every tool's output
  --kit <dir>     use the kit at <dir> (default: the one this CLI came in)

[dir] defaults to the nearest directory at or above the current one that holds a
talaria.json (scaffold: the current directory).

exit status: 0 passed, 1 failed, 2 a usage error.
`

const BOOLS = new Set(['gateway', 'record', 'json', 'verbose', 'no-typecheck', 'no-tests', 'help'])
const VALUES = new Set(['name', 'icon', 'model', 'case', 'url', 'key', 'slug', 'kit'])

export interface Parsed {
  command: string | null
  positionals: string[]
  flags: Record<string, string | true>
}

/** The command comes first; `--harness` is a switch on scaffold (add the
 *  starter harness) and a filter on eval (which harness to run). */
export function parseArgs(argv: readonly string[]): Parsed | string {
  const first = argv[0]
  const command = first !== undefined && !first.startsWith('-') ? first : null
  const rest = command === null ? argv : argv.slice(1)
  const positionals: string[] = []
  const flags: Record<string, string | true> = {}
  const isBool = (name: string) => BOOLS.has(name) || (name === 'harness' && command !== 'eval')
  const isValue = (name: string) => VALUES.has(name) || (name === 'harness' && command === 'eval')
  for (let i = 0; i < rest.length; i++) {
    const a = rest[i]!
    if (a === '-h') {
      flags.help = true
      continue
    }
    if (!a.startsWith('--')) {
      positionals.push(a)
      continue
    }
    const eq = a.indexOf('=')
    const name = a.slice(2, eq === -1 ? undefined : eq)
    if (isBool(name)) {
      if (eq !== -1) return `--${name} takes no value`
      flags[name] = true
    } else if (isValue(name)) {
      const value = eq !== -1 ? a.slice(eq + 1) : rest[++i]
      if (value === undefined || value === '' || (eq === -1 && value.startsWith('--'))) return `--${name} needs a value`
      flags[name] = value
    } else return `unknown flag --${name}`
  }
  return { command, positionals, flags }
}

/** The kit this CLI came in: lib/cli.js sits one level under its root. */
export function bundledKitRoot(moduleUrl: string): string {
  return resolve(dirname(fileURLToPath(moduleUrl)), '..')
}

interface Io {
  out: (s: string) => void
  err: (s: string) => void
  cwd: string
  env: NodeJS.ProcessEnv
  kitRoot: string
  fetchImpl?: typeof fetch
}

const str = (v: string | true | undefined): string | undefined => (typeof v === 'string' ? v : undefined)

export async function main(argv: readonly string[], io: Io): Promise<number> {
  const parsed = parseArgs(argv)
  if (typeof parsed === 'string') {
    io.err(`talaria-app: ${parsed}\n\n${USAGE}`)
    return 2
  }
  const p = parsed
  if (!p.command || p.flags.help || p.command === 'help') {
    io.out(USAGE)
    return p.command || p.flags.help ? 0 : 2
  }
  const kitRoot = resolve(io.cwd, str(p.flags.kit) ?? io.env.TALARIA_SDK_KIT ?? io.kitRoot)
  const kit = readKitManifest(kitRoot)
  if (typeof kit === 'string') {
    io.err(`talaria-app: ${kit}`)
    return 2
  }
  if (p.positionals.length > 1) {
    io.err(`talaria-app: unexpected argument ${p.positionals[1]}`)
    return 2
  }
  const json = p.flags.json === true

  if (p.command === 'version') {
    io.out(json ? JSON.stringify({ kit: kit.version, sdk: kit.sdk, hostBuild: kit.hostBuild }) : `kit ${kit.version} · @talaria/sdk ${kit.sdk} · host build ${kit.hostBuild}`)
    return 0
  }

  if (p.command === 'scaffold') {
    const dir = resolve(io.cwd, p.positionals[0] ?? '.')
    const done = scaffoldApp(dir, { name: str(p.flags.name), icon: str(p.flags.icon), harness: p.flags.harness === true })
    if (typeof done === 'string') {
      io.err(`talaria-app scaffold: ${done}`)
      return 1
    }
    if (json) io.out(JSON.stringify(done))
    else {
      io.out(`wrote ${done.written.length} files into ${done.dir}`)
      for (const f of done.written) io.out(`  ${f}`)
      for (const f of done.kept) io.out(`  ${f} (kept the repo's own)`)
      io.out('\nnext: talaria-app check && talaria-app build')
    }
    return 0
  }

  if (!['check', 'build', 'eval'].includes(p.command)) {
    io.err(`talaria-app: unknown command ${p.command}\n\n${USAGE}`)
    return 2
  }

  const app = findAppDir(resolve(io.cwd, p.positionals[0] ?? '.'))
  if (!app) {
    io.err(`talaria-app ${p.command}: no talaria.json at or above ${resolve(io.cwd, p.positionals[0] ?? '.')} — scaffold one with talaria-app scaffold`)
    return 1
  }
  const manifest = readAppManifest(app, kit.sdk)
  const slug = slugFor(app, manifest.ok ? manifest.manifest : null, str(p.flags.slug))

  if (p.command === 'check') return check(io, kit, kitRoot, app, manifest, slug, p)

  if (!manifest.ok) {
    io.err(`talaria-app ${p.command}: ${manifest.error}`)
    return 1
  }
  if (!slug) {
    io.err(`talaria-app ${p.command}: "${app}" does not name a usable app slug — declare "slug" in talaria.json or pass --slug`)
    return 1
  }
  // eval's own questions come before any build: a missing credential or
  // model is a usage error, not a reason to compile.
  let gateway: (GatewayTarget & { model: string }) | undefined
  if (p.command === 'eval' && p.flags.gateway) {
    const url = str(p.flags.url) ?? io.env.TALARIA_API_URL ?? io.env.TALARIA_URL
    const key = str(p.flags.key) ?? io.env.TALARIA_WORKBENCH_API_KEY ?? io.env.TALARIA_LLM_KEY
    if (!url || !key) {
      io.err(
        'talaria-app eval --gateway: no gateway to call. In a workbench job TALARIA_API_URL and TALARIA_WORKBENCH_API_KEY are set; anywhere else pass --url <instance origin> and --key <a gateway key> (or TALARIA_URL / TALARIA_LLM_KEY).',
      )
      return 2
    }
    const session = findEvalSession(app)
    const model = str(p.flags.model)
    if (!model) {
      const models = await gatewayModels({ url, key }, io.fetchImpl)
      io.err(`talaria-app eval --gateway: name the model to run the fixtures on with --model <id>${models.length ? `. The gateway serves: ${models.join(', ')}` : ''}`)
      return 2
    }
    gateway = { url, key, model, ...(session ? { session: session.key } : {}) }
    if (!json) io.out(session ? `gateway ${url} · ${model} · booked to workbench job ${session.jobId ?? '(this job)'}` : `gateway ${url} · ${model}`)
  } else if (p.command === 'eval' && p.flags.record) {
    io.err('talaria-app eval: --record writes what the gateway answered, so it needs --gateway')
    return 2
  }
  const ws = enterWorkspace(kitRoot, app, slug, io.env)
  const built = await captureConsole(() => kitBuild(ws, manifest.manifest.name, kit.version))
  if (p.command === 'build' || !built.value.ok) {
    if (json) io.out(JSON.stringify(built.value, null, 2))
    else io.out(formatBuildReport(built.value, io.cwd))
    if (p.flags.verbose && built.lines.length) io.err(`\n── what the host's code logged ──\n${built.lines.join('\n')}`)
    if (p.command === 'eval') io.err('\ntalaria-app eval: the app does not build, so its harnesses cannot run')
    return built.value.ok ? 0 : 1
  }

  const evaluated = await captureConsole(() =>
    runEvals({
      slug,
      appName: manifest.manifest.name,
      appDir: app,
      ...(str(p.flags.harness) ? { harness: str(p.flags.harness) } : {}),
      ...(str(p.flags.case) ? { case: str(p.flags.case) } : {}),
      ...(gateway ? { gateway, record: p.flags.record === true } : {}),
      ...(io.fetchImpl ? { fetchImpl: io.fetchImpl } : {}),
    }),
  )
  if (json) io.out(JSON.stringify(evaluated.value, null, 2))
  else io.out(formatEvalReport(evaluated.value, app))
  if (p.flags.verbose && evaluated.lines.length) io.err(`\n── what the host's code logged ──\n${evaluated.lines.join('\n')}`)
  if (evaluated.value.cases.length === 0 && (str(p.flags.harness) || str(p.flags.case))) {
    io.err('talaria-app eval: nothing matched --harness / --case')
    return 1
  }
  return evaluated.value.ok || evaluated.value.cases.length === 0 ? 0 : 1
}

async function check(
  io: Io,
  kit: KitManifest,
  kitRoot: string,
  app: string,
  manifest: ReturnType<typeof readAppManifest>,
  slug: string | null,
  p: Parsed,
): Promise<number> {
  const steps: CheckStep[] = []
  steps.push(
    manifest.ok
      ? {
          name: 'manifest',
          status: slug ? 'ok' : 'failed',
          summary: slug
            ? `${manifest.manifest.name} · installs as "${slug}"${manifest.manifest.sdk ? ` · sdk ${manifest.manifest.sdk} (this host serves ${kit.sdk})` : ''}`
            : 'no usable slug: declare "slug" in talaria.json, or name the repository talaria-app-<slug>',
          output: '',
        }
      : { name: 'manifest', status: 'failed', summary: manifest.error, output: '' },
  )
  const work = workDirFor(app, io.env)
  if (!existsSync(join(kitRoot, 'node_modules'))) {
    io.err(`talaria-app check: the kit at ${kitRoot} has no node_modules — run \`bun install\` there (prepare_env does it in a workbench job)`)
    return 1
  }
  steps.push(p.flags['no-typecheck'] ? { name: 'typecheck', status: 'skipped', summary: '--no-typecheck', output: '' } : await typecheck(kitRoot, app, work))
  steps.push(p.flags['no-tests'] ? { name: 'tests', status: 'skipped', summary: '--no-tests', output: '' } : await tests(kitRoot, app, work))
  const ok = steps.every((s) => s.status !== 'failed')
  if (p.flags.json) io.out(JSON.stringify({ app, kit: kit.version, ok, steps }, null, 2))
  else {
    io.out(`${manifest.ok ? manifest.manifest.name : app} · kit ${kit.version}`)
    for (const s of steps) {
      io.out(`  ${s.name.padEnd(10)} ${s.status === 'ok' ? 'ok' : s.status === 'skipped' ? 'skipped' : 'FAILED'} — ${s.summary}`)
      if ((s.status === 'failed' || p.flags.verbose) && s.output.trim()) io.out(indent(s.output.trim()))
    }
  }
  return ok ? 0 : 1
}

const indent = (s: string) =>
  s
    .split('\n')
    .map((l) => `      ${l}`)
    .join('\n')

/** The bundled entry: bin/talaria-app imports this module and runs it. */
export async function run(moduleUrl: string): Promise<void> {
  const code = await main(process.argv.slice(2), {
    out: (s) => process.stdout.write(`${s}\n`),
    err: (s) => process.stderr.write(`${s}\n`),
    cwd: process.cwd(),
    env: process.env,
    kitRoot: bundledKitRoot(moduleUrl),
  })
  process.exitCode = code
}
