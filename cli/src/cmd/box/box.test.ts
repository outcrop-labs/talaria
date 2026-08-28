// The box command's load-bearing shapes: the ui/.env strip-list (sealed-secret
// compat), the override file's single-environment merge (the bash version's
// duplicate-key bug dropped the token), the chassis network repoint's scope,
// rm's work guards, and the seed pipe. Filesystem state is real (tmp trees);
// process behavior is planted.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, readFileSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { boxUiEnv, overrideYaml, parseEnvFlags } from './new'
import { repointChassis, seedFleetEnv, runSeed } from './seed'
import { runEnter } from './enter'
import { runRm, runStart } from './lifecycle'
import { boxComposeSpec } from './shared'
import { fakeCtx, type FakeCtx } from '../../testing'
import { CliError } from '../../ui'

const attempt = async (fn: () => Promise<unknown>): Promise<string> => {
  try {
    await fn()
    return ''
  } catch (e) {
    return e instanceof CliError ? e.message : `<unexpected throw: ${String(e)}>`
  }
}

const makeBox = (name: string): { root: string; box: string } => {
  const root = mkdtempSync(join(tmpdir(), 'talaria-box-'))
  const box = join(root, '..', 'devboxes', name)
  mkdirSync(join(box, 'talaria/.git'), { recursive: true })
  writeFileSync(
    join(box, 'box.env'),
    `BOX_NAME=${name}\nBRANCH=agent/${name}\nAPP_PORT=5301\nPROJECT=devbox-${name}\n`,
  )
  return { root, box }
}

describe('box new — generated files', () => {
  test('boxUiEnv strips primary-service lines and keeps sealed-secret keys verbatim', () => {
    const primary = `# comment
DATABASE_URL=postgres://primary:5544/talaria
REDIS_URL=redis://primary:6399
PORT=5173
SEARXNG_URL=http://127.0.0.1:8888
TALARIA_QDRANT_URL=http://127.0.0.1:6333
TALARIA_EMBED_URL=http://127.0.0.1:8080
TALARIA_S3_BUCKET=primary-bucket
TALARIA_S3_ACCESS_KEY=primarykey
TALARIA_S3_SECRET_KEY=primarysecret
TALARIA_S3_URL=http://127.0.0.1:9000
TALARIA_SECRET_KEY=keep-exactly-this-root
AUTH_SECRET=keep-this-too
AUTH_USERS=admin@talaria.local:pass
AUTH_ADMIN_EMAILS=admin@talaria.local
TALARIA_AGENT_KEY=also-kept
`
    const out = boxUiEnv(primary, { name: 'demo', state: '/phys/state', s3: { bucket: 'b', key: 'k', secret: 's' } })
    const lines = out.split('\n')
    // primary service URLs gone; the box's own block re-adds box-side values
    expect(lines.filter((l) => l.startsWith('DATABASE_URL='))).toEqual(['DATABASE_URL=postgres://talaria:talaria@postgres:5432/talaria'])
    expect(lines.filter((l) => l.startsWith('TALARIA_S3_ACCESS_KEY='))).toEqual(['TALARIA_S3_ACCESS_KEY=k'])
    expect(lines.filter((l) => l.startsWith('TALARIA_S3_BUCKET='))).toEqual(['TALARIA_S3_BUCKET=b'])
    expect(lines.filter((l) => l.startsWith('PORT='))).toEqual(['PORT=5273'])
    // sealed-secret compat: the worktree rule
    expect(lines).toContain('TALARIA_SECRET_KEY=keep-exactly-this-root')
    expect(lines).toContain('AUTH_SECRET=keep-this-too')
    expect(lines).toContain('AUTH_USERS=admin@talaria.local:pass')
    // the marker and the fleet/app dirs
    expect(lines).toContain('TALARIA_DEVBOX=demo')
    expect(lines).toContain('TALARIA_FLEET_PROJECT=devbox-demo-fleet')
    expect(lines).toContain('TALARIA_FLEET_DIR=/phys/state/fleet')
    expect(lines).toContain('TALARIA_AGENT_DIAL=container')
  })

  test('overrideYaml merges token + SSH into ONE environment block', () => {
    const y = overrideYaml({ token: 'tok', sshSock: '/run/sock' })!
    // the bash version appended a second `environment:` key — docker's
    // duplicate-key merge then DROPPED the token
    expect(y.split('\n').filter((l) => l.trim() === 'environment:')).toHaveLength(1)
    expect(y).toContain('CLAUDE_CODE_OAUTH_TOKEN: \'tok\'')
    expect(y).toContain('SSH_AUTH_SOCK: /ssh-agent')
    expect(y).toContain('- /run/sock:/ssh-agent')
    expect(y).toContain('NEVER set CLAUDE_CODE_OAUTH_TOKEN and ANTHROPIC_AUTH_TOKEN together')
  })

  test('overrideYaml: token alone → no volumes; nothing → no file', () => {
    expect(overrideYaml({ token: 'tok' })).not.toContain('volumes:')
    expect(overrideYaml({ sshSock: '/s' })).not.toContain("CLAUDE_CODE_OAUTH_TOKEN: '")
    expect(overrideYaml({})).toBeNull()
  })

  test('overrideYaml: inherited Anthropic-compatible endpoint joins the same environment block', () => {
    const y = overrideYaml({
      sshSock: '/s',
      anthropic: { baseUrl: 'https://api.z.ai/api/anthropic', authToken: 'tok' },
    })!
    expect(y.split('\n').filter((l) => l.trim() === 'environment:')).toHaveLength(1)
    expect(y).toContain("ANTHROPIC_BASE_URL: 'https://api.z.ai/api/anthropic'")
    expect(y).toContain("ANTHROPIC_AUTH_TOKEN: 'tok'")
    expect(y).not.toContain('ANTHROPIC_MODEL') // unset model must be TRULY ABSENT
    expect(y).toContain('SSH_AUTH_SOCK: /ssh-agent')
  })

  test('overrideYaml: endpoint alone writes the file; model rides when set', () => {
    const y = overrideYaml({ anthropic: { baseUrl: 'https://p', authToken: 't', model: 'glm-4.7' } })!
    expect(y).toContain("ANTHROPIC_MODEL: 'glm-4.7'")
    expect(y).not.toContain('volumes:')
  })

  test('overrideYaml: an --env key wins over the inherited trio — no key emitted twice', () => {
    const y = overrideYaml({
      anthropic: { baseUrl: 'https://p', authToken: 'inherited', model: 'm' },
      extra: { ANTHROPIC_AUTH_TOKEN: 'explicit', OTHER_TOOL_TOKEN: "va'lue" },
    })!
    expect(y.match(/^ {6}ANTHROPIC_AUTH_TOKEN:/gm)).toHaveLength(1) // env entries, not the header's rule
    expect(y).toContain("ANTHROPIC_AUTH_TOKEN: 'explicit'")
    expect(y).toContain("OTHER_TOOL_TOKEN: 'va''lue'") // '' is the YAML single-quote escape
  })

  test('parseEnvFlags: KEY=VALUE list → ordered map; malformed dies loudly', () => {
    expect(parseEnvFlags(['A=1', 'B=2'])).toEqual({ A: '1', B: '2' })
    expect(parseEnvFlags(['EMPTY='])).toEqual({ EMPTY: '' })
    expect(() => parseEnvFlags(['nope'])).toThrow('--env wants KEY=VALUE')
    expect(() => parseEnvFlags(['1BAD=x'])).toThrow('--env wants KEY=VALUE')
    expect(() => parseEnvFlags(['A B=x'])).toThrow('--env wants KEY=VALUE')
  })
})

describe('box seed — pure transforms', () => {
  test('repointChassis moves only the network block name', () => {
    const y = `service:
  image: x
network:
  name: talaria
  driver: bridge
volumes:
  name: keepme
`
    const out = repointChassis(y, 'devbox-demo-fleet')
    expect(out).toContain('  name: devbox-demo-fleet')
    expect(out).toContain('  name: keepme')
    expect(out).toContain('  driver: bridge')
  })

  test('seedFleetEnv keeps only the endpoint lines agents interpolate', () => {
    const primary = `# header noise
LLM_BASE_URL=http://primary:5273/api/llm/v1
LLM_API_KEY=sekrit
LLM_MODEL=gpt-x
#HERMES_IMAGE=commented
HERMES_IMAGE=repo/img:2
TALARIA_AGENT_KEY=not-a-fleet-var
OTHER=no
`
    const out = seedFleetEnv(primary)
    const keys = out.split('\n').filter((l) => /^[A-Z]/.test(l)).map((l) => l.split('=')[0])
    expect(keys.sort()).toEqual(['HERMES_IMAGE', 'LLM_API_KEY', 'LLM_BASE_URL', 'LLM_MODEL'])
  })
})

describe('box seed — run', () => {
  const makeSeedTree = (): string => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-seed-'))
    mkdirSync(join(root, 'ui'), { recursive: true })
    writeFileSync(join(root, 'ui/.env'), 'TALARIA_S3_ACCESS_KEY=k\nTALARIA_S3_SECRET_KEY=s\n')
    mkdirSync(join(root, 'scripts'), { recursive: true })
    writeFileSync(join(root, 'scripts/chassis.template.yml'), 'service:\n  image: x\nnetwork:\n  name: talaria\n')
    mkdirSync(join(root, 'fleet'), { recursive: true })
    writeFileSync(join(root, 'fleet/.env'), 'LLM_BASE_URL=http://p\nLLM_API_KEY=sekrit\nNOISE=1\n')
    return root
  }

  test('empty box DB → streaming pg_dump | psql, chassis repointed, fleet/.env 0600', async () => {
    const root = makeSeedTree()
    const ctx = fakeCtx()
    ctx.root = root
    await runSeed(ctx, 'demo')
    const piped = ctx.calls.filter((c) => c.cmd === 'docker' && c.args[0] === 'exec' && c.args[1] === '-i')
    expect(piped).toHaveLength(1) // the psql side of the pipe
    expect(ctx.calls.some((c) => c.cmd === 'docker' && c.args.includes('pg_dump') && c.args.includes('--clean'))).toBe(true)
    const state = join(root, '..', 'devboxes', 'demo', 'state')
    expect(readFileSync(join(state, 'fleet/chassis.yml'), 'utf8')).toContain('name: devbox-demo-fleet')
    const envPath = join(state, 'fleet/.env')
    expect(readFileSync(envPath, 'utf8')).toContain('LLM_API_KEY=sekrit')
    expect((statSync(envPath).mode & 0o777)).toBe(0o600)
    // the throwaway mc container is removed even on the happy path
    expect(ctx.calls.some((c) => c.cmd === 'docker' && c.args[0] === 'rm' && c.args.includes('devbox-demo-seed-mc')))
  })

  test('box DB with tables and no --force → no pipe, keeps existing chassis/.env', async () => {
    const root = makeSeedTree()
    const state = join(root, '..', 'devboxes', 'demo', 'state')
    mkdirSync(join(state, 'fleet'), { recursive: true })
    writeFileSync(join(state, 'fleet/chassis.yml'), 'network:\n  name: user-edited\n')
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['docker', ['exec', 'devbox-demo-postgres', 'psql', '-U', 'talaria', '-d', 'talaria', '-tAc', "select 1 from information_schema.tables where table_schema='public' limit 1"]], ' 1\n')
    await runSeed(ctx, 'demo')
    expect(ctx.calls.some((c) => c.cmd === 'docker' && c.args.includes('pg_dump'))).toBe(false)
    expect(readFileSync(join(state, 'fleet/chassis.yml'), 'utf8')).toContain('user-edited')
  })

  test('primary postgres down → dies before touching the box', async () => {
    const root = makeSeedTree()
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['docker', ['inspect', 'talaria-postgres-dev']], new Error('no such object'))
    const msg = await attempt(() => runSeed(ctx, 'demo'))
    expect(msg).toContain("primary postgres (talaria-postgres-dev) isn't running")
    expect(ctx.calls.some((c) => c.args.includes('pg_dump'))).toBe(false)
  })
})

describe('box start — converge, not just start', () => {
  test('the box project comes up via `up -d`, so an edited override applies', async () => {
    const { root } = makeBox('demo')
    const ctx = fakeCtx()
    ctx.root = root
    await runStart(ctx, 'demo')
    const boxUps = ctx.calls.filter(
      (c) => c.cmd === 'docker' && c.args.includes('compose') && c.args.includes('devbox-demo') && c.args.includes('up') && c.args.includes('-d'),
    )
    expect(boxUps.length).toBeGreaterThan(0)
    // `start` would merely re-launch the existing containers — an edited
    // compose.override.yml (the documented stop-edit-start channel) never applies
    expect(ctx.calls.some((c) => c.args.includes('start'))).toBe(false)
  })
})

describe('box rm — work guards', () => {
  test('uncommitted changes → refuses, zero teardown calls', async () => {
    const { root, box } = makeBox('demo')
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['git', ['-C', join(box, 'talaria'), 'status', '--porcelain']], ' M src/x.ts\n')
    const msg = await attempt(() => runRm(ctx, 'demo', false))
    expect(msg).toContain('uncommitted changes')
    expect(ctx.calls.some((c) => c.args.includes('down'))).toBe(false)
    expect(ctx.calls.some((c) => c.args.includes('rm') && c.args[0] === 'network')).toBe(false)
  })

  test('unpushed commits → refuses and lists them', async () => {
    const { root, box } = makeBox('demo')
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['git', ['-C', join(box, 'talaria'), 'log', '--branches', '--not', '--remotes', '--oneline']], 'abc1234 did a thing\ndef5678 another\n')
    const msg = await attempt(() => runRm(ctx, 'demo', false))
    expect(msg).toContain('abc1234 did a thing')
    expect(msg).toContain('def5678 another')
    expect(ctx.calls.some((c) => c.args.includes('down'))).toBe(false)
  })

  test('clean → box down, fleet down, network rm, directory gone', async () => {
    const { root, box } = makeBox('demo')
    const ctx = fakeCtx()
    ctx.root = root
    await runRm(ctx, 'demo', false)
    const downs = ctx.calls.filter((c) => c.args.includes('down'))
    expect(downs).toHaveLength(2)
    expect(downs[0]!.args).toContain('devbox-demo') // box project, with -f/--env-file
    expect(downs[0]!.args).toContain(join(box, 'compose.env'))
    expect(downs[1]!.args).toEqual(['compose', '-p', 'devbox-demo-fleet', 'down', '-v', '--remove-orphans'])
    const netRm = ctx.calls.find((c) => c.args[0] === 'network' && c.args[1] === 'rm')!
    expect(netRm.args[2]).toBe('devbox-demo-fleet')
    expect(await Bun.file(join(box, 'box.env')).exists()).toBe(false)
  })

  test('--force skips the guards entirely', async () => {
    const { root, box } = makeBox('demo')
    const ctx = fakeCtx()
    ctx.root = root
    ctx.plant(['git', ['-C', join(box, 'talaria'), 'status', '--porcelain']], ' M x\n')
    await runRm(ctx, 'demo', true)
    expect(ctx.calls.some((c) => c.args.includes('down'))).toBe(true)
  })
})

describe('box enter', () => {
  test('no such box → dies', async () => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-empty-'))
    const ctx = fakeCtx()
    ctx.root = root
    const msg = await attempt(() => runEnter(ctx, 'nope', []))
    expect(msg).toContain("no devbox named 'nope'")
  })

  test('non-TTY → no -it, defaults to bash at /work/talaria', async () => {
    const { root } = makeBox('demo')
    const ctx = fakeCtx()
    ctx.root = root
    await runEnter(ctx, 'demo', [])
    const call = ctx.calls.find((c) => c.args[0] === 'exec')!
    expect(call.args).toEqual(['exec', '-w', '/work/talaria', 'devbox-demo', 'bash'])
  })

  test('TTY → -it, and a command passes through', async () => {
    const { root } = makeBox('demo')
    const ctx = fakeCtx({ isTTY: true })
    ctx.root = root
    await runEnter(ctx, 'demo', ['claude', '--model', 'x'])
    expect(ctx.calls.find((c) => c.args[0] === 'exec')!.args).toEqual([
      'exec', '-it', '-w', '/work/talaria', 'devbox-demo', 'claude', '--model', 'x',
    ])
  })
})

describe('box compose spec', () => {
  test('override file merges in only when present', () => {
    const { root, box } = makeBox('demo')
    const ctx = fakeCtx()
    ctx.root = root
    expect(boxComposeSpec(ctx, 'demo').files).toHaveLength(1)
    writeFileSync(join(box, 'compose.override.yml'), 'services: {}\n')
    const spec = boxComposeSpec(ctx, 'demo')
    expect(spec.files).toHaveLength(2)
    expect(spec.files[1]).toBe(join(box, 'compose.override.yml'))
    expect(spec.project).toBe('devbox-demo')
    expect(spec.envFile).toBe(join(box, 'compose.env'))
  })
})
