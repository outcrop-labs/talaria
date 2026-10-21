// `talaria setup`: the PATH shim (where it lands, what it says, that it stays
// executable, which checkout it names), and the re-run contract — on a
// checkout that is already set up, nothing is regenerated, pulled or rebuilt.

import { describe, expect, test } from 'bun:test'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, statSync, utimesSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { installShim, missingImageServices, runSetup, shimDir, shimTarget } from './setup'
import { SIDECARS_COMPOSE } from '../compose'
import { fakeCtx } from '../testing'
import { CliError } from '../ui'

const homeCtx = (over: Record<string, string> = {}) => {
  const ctx = fakeCtx({ env: { HOME: mkdtempSync(join(tmpdir(), 'talaria-shim-')), ...over } })
  ctx.root = mkdtempSync(join(tmpdir(), 'talaria-root-'))
  return ctx
}

describe('shimDir', () => {
  test('TALARIA_BIN_DIR wins outright', () => {
    const ctx = homeCtx({ TALARIA_BIN_DIR: '/opt/bin' })
    expect(shimDir(ctx)).toBe('/opt/bin')
  })

  test('no bun bin dir → ~/.local/bin (created on install)', () => {
    const ctx = homeCtx()
    expect(shimDir(ctx)).toBe(join(ctx.env.HOME!, '.local/bin'))
  })

  test('an existing ~/.bun/bin wins over ~/.local/bin', () => {
    const ctx = homeCtx()
    mkdirSync(join(ctx.env.HOME!, '.bun/bin'), { recursive: true })
    expect(shimDir(ctx)).toBe(join(ctx.env.HOME!, '.bun/bin'))
  })

  test('BUN_INSTALL relocates the bun bin dir', () => {
    const ctx = homeCtx({ BUN_INSTALL: '/custom/bun' })
    expect(shimDir(ctx)).toBe('/custom/bun/bin')
  })
})

describe('installShim', () => {
  test('writes an executable sh shim that execs bun against this checkout', () => {
    const ctx = homeCtx()
    const path = installShim(ctx)
    expect(dirname(path)).toBe(shimDir(ctx))
    expect(path.endsWith('/talaria')).toBe(true)
    expect(statSync(path).mode & 0o777).toBe(0o755)
    const text = readFileSync(path, 'utf8')
    expect(text.startsWith('#!/bin/sh\n')).toBe(true)
    expect(text).toContain(`exec bun '${ctx.root}/cli/bin/talaria.ts' "$@"`)
    // honest about repointing, for whoever finds it first
    expect(text).toContain('re-run setup')
  })

  test('re-running overwrites — the shim follows the last checkout that ran setup', () => {
    const ctx = homeCtx()
    installShim(ctx)
    const second = homeCtx()
    second.env.HOME = ctx.env.HOME
    const path = installShim(second)
    expect(readFileSync(path, 'utf8')).toContain(`exec bun '${second.root}/cli/bin/talaria.ts'`)
  })

  test('a repo path containing a quote refuses rather than writing a broken shim', () => {
    const ctx = homeCtx()
    ctx.root = `/mnt/data/jon's/talaria`
    try {
      installShim(ctx)
      throw new Error('expected a die')
    } catch (e) {
      expect(e).toBeInstanceOf(CliError)
      expect((e as CliError).message).toContain('single quote')
    }
  })
})

describe('shimTarget', () => {
  test('a plain checkout is its own target', () => {
    const ctx = homeCtx()
    mkdirSync(join(ctx.root, '.git'))
    expect(shimTarget(ctx)).toBe(ctx.root)
  })

  test('a linked worktree names the primary checkout', () => {
    const ctx = homeCtx()
    const primary = mkdtempSync(join(tmpdir(), 'talaria-primary-'))
    mkdirSync(join(primary, 'cli/bin'), { recursive: true })
    writeFileSync(join(primary, 'cli/bin/talaria.ts'), '')
    writeFileSync(join(ctx.root, '.git'), `gitdir: ${primary}/.git/worktrees/talaria-x\n`)
    expect(shimTarget(ctx)).toBe(primary)
    // and the shim it writes says so
    expect(readFileSync(installShim(ctx, shimTarget(ctx)), 'utf8')).toContain(`exec bun '${primary}/cli/bin/talaria.ts'`)
  })

  test('a gitdir that is not a worktree shape, or a primary with no CLI, falls back to this checkout', () => {
    const ctx = homeCtx()
    writeFileSync(join(ctx.root, '.git'), 'gitdir: /somewhere/else/.git\n')
    expect(shimTarget(ctx)).toBe(ctx.root)
    const gone = join(tmpdir(), 'talaria-no-such-primary')
    writeFileSync(join(ctx.root, '.git'), `gitdir: ${gone}/.git/worktrees/x\n`)
    expect(shimTarget(ctx)).toBe(ctx.root)
  })
})

// The dev stack as compose would describe it — what `config --format json`
// answers for the sidecar plane + dev-compose.
const STACK = {
  services: {
    postgres: { image: 'docker.io/library/postgres:16-alpine' },
    redis: { image: 'docker.io/library/redis:7-alpine' },
    qdrant: { image: 'docker.io/qdrant/qdrant:latest' },
    embeddings: { image: 'ghcr.io/huggingface/text-embeddings-inference:cpu-latest' },
    storage: { image: 'ghcr.io/versity/versitygw:v1.8.0' },
  },
}

const devFiles = (root: string) => [join(root, SIDECARS_COMPOSE), join(root, 'docker/dev-compose.yml')]
const configArgs = (root: string) => ['compose', '-f', devFiles(root)[0]!, '-f', devFiles(root)[1]!, 'config', '--format', 'json']

describe('missingImageServices', () => {
  test('names only the services whose image the daemon lacks', async () => {
    const ctx = homeCtx()
    ctx.plant(['docker', configArgs(ctx.root)], JSON.stringify(STACK))
    ctx.plant(['docker', ['image', 'inspect', 'docker.io/qdrant/qdrant:latest']], new Error('No such image'))
    ctx.plant(['docker', ['image', 'inspect', 'ghcr.io/versity/versitygw:v1.8.0']], new Error('No such image'))
    expect(await missingImageServices(ctx, devFiles(ctx.root))).toEqual(['qdrant', 'storage'])
  })

  test('everything present → an empty list; an unreadable config → null (pull everything)', async () => {
    const ctx = homeCtx()
    ctx.plant(['docker', configArgs(ctx.root)], JSON.stringify(STACK))
    expect(await missingImageServices(ctx, devFiles(ctx.root))).toEqual([])
    const old = homeCtx()
    old.plant(['docker', configArgs(old.root)], 'unknown flag: --format')
    expect(await missingImageServices(old, devFiles(old.root))).toBeNull()
  })
})

// A root in the shape setup leaves behind. Every piece is optional so a test
// can remove exactly one and watch setup add exactly that one back.
function seedRoot(ctx: ReturnType<typeof homeCtx>, parts: { uiEnv?: boolean; fleet?: boolean; deps?: boolean; mcpDist?: boolean; ompDist?: boolean }) {
  const root = ctx.root
  const past = new Date(Date.now() - 60_000)
  mkdirSync(join(root, 'scripts'), { recursive: true })
  writeFileSync(join(root, 'scripts/chassis.template.yml'), 'service:\n  image: x\n')
  mkdirSync(join(root, 'ui'), { recursive: true })
  if (parts.uiEnv) {
    writeFileSync(
      join(root, 'ui/.env'),
      `DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\nREDIS_URL=redis://127.0.0.1:6399\nAUTH_SECRET=${'a'.repeat(64)}\nTALARIA_SECRET_KEY=${'b'.repeat(64)}\n`,
    )
  }
  if (parts.fleet) {
    mkdirSync(join(root, 'fleet'), { recursive: true })
    writeFileSync(join(root, 'fleet/chassis.yml'), 'service:\n  image: mine\n  cap_add: [CHOWN]\n')
    writeFileSync(join(root, 'fleet/.env'), 'LLM_BASE_URL=\n')
  }
  for (const pkg of ['mcp', 'omp-auth']) {
    mkdirSync(join(root, pkg, 'src'), { recursive: true })
    writeFileSync(join(root, pkg, 'src/index.ts'), '')
    utimesSync(join(root, pkg, 'src/index.ts'), past, past)
    if (parts.deps) mkdirSync(join(root, pkg, 'node_modules'), { recursive: true })
  }
  if (parts.deps) mkdirSync(join(root, 'ui/node_modules'), { recursive: true })
  if (parts.mcpDist) {
    mkdirSync(join(root, 'mcp/dist'), { recursive: true })
    writeFileSync(join(root, 'mcp/dist/index.js'), '')
  }
  if (parts.ompDist) {
    mkdirSync(join(root, 'omp-auth/dist'), { recursive: true })
    writeFileSync(join(root, 'omp-auth/dist/server.js'), '')
  }
  ctx.plant(['docker', configArgs(root)], JSON.stringify(STACK))
}

const complete = { uiEnv: true, fleet: true, deps: true, mcpDist: true, ompDist: true }

const setupCtx = () => {
  const ctx = homeCtx()
  ctx.env.TALARIA_BIN_DIR = join(ctx.env.HOME!, 'bin')
  ctx.env.PATH = `${ctx.env.TALARIA_BIN_DIR}:/usr/bin`
  return ctx
}

const argsOf = (ctx: ReturnType<typeof homeCtx>) => ctx.calls.map((c) => [c.cmd, ...c.args].join(' '))
const cwdsOf = (ctx: ReturnType<typeof homeCtx>, ...args: string[]) =>
  ctx.calls.filter((c) => c.args.join(' ') === args.join(' ')).map((c) => c.opts?.cwd)

describe('runSetup on an existing install', () => {
  test('a complete install: nothing pulled, installed or built, ui/.env byte-identical, the shim refreshed', async () => {
    const ctx = setupCtx()
    seedRoot(ctx, complete)
    const before = readFileSync(join(ctx.root, 'ui/.env'), 'utf8')
    expect(await runSetup(ctx)).toBe(0)
    expect(readFileSync(join(ctx.root, 'ui/.env'), 'utf8')).toBe(before)
    const calls = argsOf(ctx)
    expect(calls.some((c) => c.includes(' pull'))).toBe(false)
    expect(calls.some((c) => c === 'bun install')).toBe(false)
    expect(calls.some((c) => c === 'bun run build')).toBe(false)
    // what a re-run is for
    const shim = readFileSync(join(ctx.env.TALARIA_BIN_DIR!, 'talaria'), 'utf8')
    expect(shim).toContain(`exec bun '${ctx.root}/cli/bin/talaria.ts'`)
    expect(ctx.logLines.some((l) => l.msg.includes('Existing install'))).toBe(true)
    expect(ctx.logLines.some((l) => l.msg.includes('nothing regenerated'))).toBe(true)
  })

  test('the shim lands before anything that can fail — a broken toolkit build still leaves `talaria` on PATH', async () => {
    const ctx = setupCtx()
    seedRoot(ctx, { ...complete, mcpDist: false })
    ctx.plant(['bun', ['run', 'build']], new Error('tsc: boom'))
    await expect(runSetup(ctx)).rejects.toThrow(/mcp\/ failed to build/)
    expect(existsSync(join(ctx.env.TALARIA_BIN_DIR!, 'talaria'))).toBe(true)
  })

  test('a root missing only mcp/dist builds mcp — and nothing else', async () => {
    const ctx = setupCtx()
    seedRoot(ctx, { ...complete, mcpDist: false })
    const before = readFileSync(join(ctx.root, 'ui/.env'), 'utf8')
    expect(await runSetup(ctx)).toBe(0)
    expect(readFileSync(join(ctx.root, 'ui/.env'), 'utf8')).toBe(before)
    expect(cwdsOf(ctx, 'run', 'build')).toEqual([join(ctx.root, 'mcp')])
    expect(argsOf(ctx).some((c) => c === 'bun install' || c.includes(' pull'))).toBe(false)
  })

  test('only the images the daemon lacks are pulled', async () => {
    const ctx = setupCtx()
    seedRoot(ctx, complete)
    ctx.plant(['docker', ['image', 'inspect', 'docker.io/qdrant/qdrant:latest']], new Error('No such image'))
    expect(await runSetup(ctx)).toBe(0)
    const pulls = ctx.calls.filter((c) => c.cmd === 'docker' && c.args.includes('pull'))
    expect(pulls).toHaveLength(1)
    expect(pulls[0]!.args.slice(-3)).toEqual(['pull', '-q', 'qdrant'])
  })
})

describe('runSetup on a fresh clone', () => {
  test('generates ui/.env and the fleet plane, pulls every image, installs and builds', async () => {
    const ctx = setupCtx()
    seedRoot(ctx, {})
    for (const svc of Object.values(STACK.services)) {
      ctx.plant(['docker', ['image', 'inspect', svc.image]], new Error('No such image'))
    }
    expect(await runSetup(ctx)).toBe(0)
    const env = readFileSync(join(ctx.root, 'ui/.env'), 'utf8')
    expect(env).toMatch(/^TALARIA_SECRET_KEY=[0-9a-f]{64}$/m)
    expect(existsSync(join(ctx.root, 'fleet/chassis.yml'))).toBe(true)
    expect(existsSync(join(ctx.root, 'fleet/.env'))).toBe(true)
    const pulls = ctx.calls.filter((c) => c.cmd === 'docker' && c.args.includes('pull'))
    expect(pulls).toHaveLength(1)
    expect(pulls[0]!.args.slice(-5)).toEqual(['postgres', 'redis', 'qdrant', 'embeddings', 'storage'])
    expect(cwdsOf(ctx, 'install').sort()).toEqual([join(ctx.root, 'mcp'), join(ctx.root, 'omp-auth'), join(ctx.root, 'ui')].sort())
    expect(cwdsOf(ctx, 'run', 'build').sort()).toEqual([join(ctx.root, 'mcp'), join(ctx.root, 'omp-auth')].sort())
    expect(ctx.logLines.some((l) => l.msg.includes('Existing install'))).toBe(false)
  })
})
