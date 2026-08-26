// backup-lib.sh's decision table, as tests: label/manifest/format helpers,
// checksum write+verify, the storage-mode sniff (both failure kinds kept
// apart), client resolution, and env lifting. Filesystem state is real (tmp
// trees); process behavior is planted. `die` throws CliError.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  bucketUploadsPath, clientFor, dbLabel, humanSize, isoSecond, liftAppEnv, manifestGet,
  pgQuery, stampOf, storageFromDb, storageFromManifest, verifySums, writeSums,
} from './lib'
import { fakeCtx } from '../testing'
import { CliError } from '../ui'

const attempt = async (fn: () => unknown): Promise<string> => {
  try {
    await fn()
    return ''
  } catch (e) {
    return e instanceof CliError ? e.message : `<unexpected throw: ${String(e)}>`
  }
}

describe('formats', () => {
  test('dbLabel strips everything up to the LAST @ (credentials gone)', () => {
    expect(dbLabel('postgres://u:p%40x@127.0.0.1:5544/talaria')).toBe('127.0.0.1:5544/talaria')
    expect(dbLabel('postgres://a@b@c/d')).toBe('c/d')
    expect(dbLabel('no-at-here')).toBe('no-at-here')
  })

  test('stampOf / isoSecond are the bash date formats', () => {
    const d = new Date('2026-07-31T18:44:21.123Z')
    expect(stampOf(d)).toBe('20260731T184421Z')
    expect(isoSecond(d)).toBe('2026-07-31T18:44:21Z')
  })

  test('humanSize is du -h shaped', () => {
    expect(humanSize(512)).toBe('512')
    expect(humanSize(2048)).toBe('2.0K')
    expect(humanSize(3 * 1024 * 1024)).toBe('3.0M')
    expect(humanSize(1024 ** 3)).toBe('1.0G')
  })

  test('manifestGet reads the first matching key', () => {
    expect(manifestGet('a=1\nb=2\na=3\n', 'a')).toBe('1')
    expect(manifestGet('a=1\n', 'missing')).toBe('')
  })
})

describe('checksums', () => {
  test('writeSums then verifySums round-trips (GNU two-space format)', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'talaria-sums-'))
    try {
      writeFileSync(join(dir, 'a'), 'alpha')
      writeFileSync(join(dir, 'b'), 'beta=with=equals')
      await writeSums(dir, ['a', 'b'])
      const sums = (await import('node:fs')).readFileSync(join(dir, 'SHA256SUMS'), 'utf8')
      expect(sums).toMatch(/^[0-9a-f]{64}  a\n[0-9a-f]{64}  b\n$/)
      await verifySums(dir) // does not throw
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  test('a tampered file fails verification', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'talaria-sums-'))
    try {
      writeFileSync(join(dir, 'a'), 'alpha')
      await writeSums(dir, ['a'])
      writeFileSync(join(dir, 'a'), 'evil')
      await expect(verifySums(dir)).rejects.toThrow('checksum mismatch: a')
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  test('a missing sums file fails', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'talaria-sums-'))
    try {
      await expect(verifySums(dir)).rejects.toThrow('no SHA256SUMS')
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })

  test('a sums line naming a path is refused, not followed', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'talaria-sums-'))
    try {
      writeFileSync(join(dir, 'a'), 'alpha')
      writeFileSync(join(dir, 'SHA256SUMS'), `${'0'.repeat(64)}  ../elsewhere\n`)
      await expect(verifySums(dir)).rejects.toThrow('unexpected path')
    } finally {
      rmSync(dir, { recursive: true, force: true })
    }
  })
})

describe('liftAppEnv', () => {
  test('file values load, shell wins, one quote layer is tolerated', () => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-env-'))
    try {
      mkdirSync(join(root, 'ui'), { recursive: true })
      writeFileSync(
        join(root, 'ui/.env'),
        'DATABASE_URL="postgres://f@h/db"\nTALARIA_S3_BUCKET=fromfile\nTALARIA_MINIO_PORT=9010\n',
      )
      const ctx = fakeCtx({ env: { TALARIA_S3_BUCKET: 'fromenv' } })
      ctx.root = root
      const env = liftAppEnv(ctx)
      expect(env.DATABASE_URL).toBe('postgres://f@h/db')
      expect(env.TALARIA_S3_BUCKET).toBe('fromenv')
      expect(env.TALARIA_MINIO_PORT).toBe('9010')
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })

  test('TALARIA_ENV_FILE points elsewhere; a missing file is not an error', () => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-env-'))
    try {
      writeFileSync(join(root, 'other.env'), 'DATABASE_URL=x\n')
      const ctx = fakeCtx({ env: { TALARIA_ENV_FILE: 'other.env' } })
      ctx.root = root
      expect(liftAppEnv(ctx).DATABASE_URL).toBe('x')
      ctx.env.TALARIA_ENV_FILE = 'gone.env'
      expect(liftAppEnv(ctx).DATABASE_URL).toBeUndefined()
    } finally {
      rmSync(root, { recursive: true, force: true })
    }
  })
})

describe('pg client', () => {
  test('host binary wins when present', async () => {
    const ctx = fakeCtx()
    expect(await clientFor(ctx, 'psql')).toEqual({ kind: 'host', bin: 'psql', pre: [] })
  })

  test('without a host binary, the argv borrows postgres:16-alpine on the host network', async () => {
    const ctx = fakeCtx()
    ctx.plant(['psql', ['--version']], new Error('not found'))
    expect(await clientFor(ctx, 'psql')).toEqual({
      kind: 'docker',
      pre: ['run', '--rm', '-i', '--network', 'host', 'postgres:16-alpine', 'psql'],
    })
  })

  test('TALARIA_PG_IMAGE overrides the fallback image', async () => {
    const ctx = fakeCtx({ env: { TALARIA_PG_IMAGE: 'postgres:17-alpine' } })
    ctx.plant(['psql', ['--version']], new Error('not found'))
    const c = await clientFor(ctx, 'psql')
    expect(c.kind === 'docker' && c.pre.includes('postgres:17-alpine')).toBe(true)
  })

  test('pgQuery runs one psql with ON_ERROR_STOP and the US field separator', async () => {
    const ctx = fakeCtx()
    ctx.plant(['psql', ['postgres://u@h/db', '-v', 'ON_ERROR_STOP=1', '-At', '-F', '\x1f', '-q', '-c', 'select 1']], 't')
    expect(await pgQuery(ctx, 'postgres://u@h/db', 'select 1')).toBe('t')
  })
})

describe('storageFromDb', () => {
  const URL_ = 'postgres://talaria:talaria@127.0.0.1:5544/talaria'
  const PROBE_SQL = `select to_regclass('public.app_settings') is not null`
  const ROW_SQL =
    `select coalesce(value->>'mode','local'), coalesce(value->>'endpoint',''), coalesce(value->>'bucket',''), ` +
    `coalesce(value->>'prefix',''), coalesce(value->>'accessKeyId','') from app_settings where key = 'storage_config'`
  const qArgs = (sql: string) => ['psql', [URL_, '-v', 'ON_ERROR_STOP=1', '-At', '-F', '\x1f', '-q', '-c', sql]]
  const probe = ['psql', qArgs(PROBE_SQL)[1]] as [string, string[]]
  const row = ['psql', qArgs(ROW_SQL)[1]] as [string, string[]]

  test('unreachable database dies naming the label — not silently "local"', async () => {
    const ctx = fakeCtx()
    ctx.plant(probe, new Error('connection refused'))
    const msg = await attempt(() => storageFromDb(ctx, URL_, {}))
    expect(msg).toContain('cannot query 127.0.0.1:5544/talaria')
  })

  test('missing app_settings table → local, and the config row is never asked for', async () => {
    const ctx = fakeCtx()
    ctx.plant(probe, '')
    const st = await storageFromDb(ctx, URL_, {})
    expect(st.mode).toBe('local')
    // exactly one QUERY ran (clientFor's --version probe doesn't count)
    expect(ctx.calls.filter((c) => c.cmd === 'psql' && c.args.includes('-c'))).toHaveLength(1)
  })

  test('no storage row → local', async () => {
    const ctx = fakeCtx()
    ctx.plant(probe, 't')
    ctx.plant(row, '')
    expect((await storageFromDb(ctx, URL_, {})).mode).toBe('local')
  })

  test('internal row resolves like storage.ts internalTarget, env-overridable', async () => {
    const ctx = fakeCtx()
    ctx.plant(probe, 't')
    ctx.plant(row, 'internal\x1f\x1f\x1f\x1f\x1f')
    const st = await storageFromDb(ctx, URL_, { TALARIA_S3_URL: 'http://10.0.0.9:9010', TALARIA_S3_BUCKET: 'box' })
    expect(st).toMatchObject({
      mode: 'internal', endpoint: 'http://10.0.0.9:9010', bucket: 'box', prefix: '',
      accessKey: 'talaria', secretKey: 'talaria-dev-secret',
    })
  })

  test('s3 row without the out-of-band secret dies with the sealed-key message', async () => {
    const ctx = fakeCtx()
    ctx.plant(probe, 't')
    ctx.plant(row, 's3\x1fhttps://s3.example\x1fkbucket\x1fpfx/\x1fAKIA')
    const msg = await attempt(() => storageFromDb(ctx, URL_, {}))
    expect(msg).toContain('TALARIA_BACKUP_S3_SECRET_KEY')
    expect(msg).toContain('sealed at rest')
  })

  test('s3 row takes the access key from the database, the secret out of band', async () => {
    const ctx = fakeCtx({ env: { TALARIA_BACKUP_S3_SECRET_KEY: 'out-of-band' } })
    ctx.plant(probe, 't')
    ctx.plant(row, 's3\x1fhttps://s3.example\x1fkbucket\x1fpfx/\x1fAKIA')
    const st = await storageFromDb(ctx, URL_, ctx.env)
    expect(st).toMatchObject({ mode: 's3', endpoint: 'https://s3.example', bucket: 'kbucket', prefix: 'pfx/', accessKey: 'AKIA', secretKey: 'out-of-band' })
    expect(bucketUploadsPath(st)).toBe('t/kbucket/pfx/uploads')
  })

  test('an unknown mode in app_settings dies rather than guessing', async () => {
    const ctx = fakeCtx()
    ctx.plant(probe, 't')
    ctx.plant(row, 'carrier-pigeon\x1f\x1f\x1f\x1f\x1f')
    const msg = await attempt(() => storageFromDb(ctx, URL_, {}))
    expect(msg).toContain('unknown storage mode "carrier-pigeon"')
  })
})

describe('storageFromManifest', () => {
  const M = (mode: string, extra = ''): string => `talaria_backup=1\nstorage_mode=${mode}\n${extra}`

  test('local and internal', () => {
    const ctx = fakeCtx()
    expect(storageFromManifest(ctx, M('local'), {}).mode).toBe('local')
    expect(storageFromManifest(ctx, M('internal'), { TALARIA_S3_URL: 'http://m:9010' }).endpoint).toBe('http://m:9010')
  })

  test('s3: manifest values, TALARIA_BACKUP_S3_* overrides, creds required', () => {
    const ctx = fakeCtx()
    const m = M('s3', 'storage_endpoint=https://old\nstorage_bucket=oldb\nstorage_prefix=old/\n')
    expect(storageFromManifest(ctx, m, {
      TALARIA_BACKUP_S3_ENDPOINT: 'https://drill', TALARIA_BACKUP_S3_BUCKET: 'drillb',
      TALARIA_BACKUP_S3_ACCESS_KEY: 'AK', TALARIA_BACKUP_S3_SECRET_KEY: 'SK',
    })).toMatchObject({ endpoint: 'https://drill', bucket: 'drillb', prefix: 'old/', accessKey: 'AK', secretKey: 'SK' })
  })

  test('a manifest without a usable mode dies', async () => {
    const ctx = fakeCtx()
    const msg = await attempt(() => storageFromManifest(ctx, M(''), {}))
    expect(msg).toContain('no usable storage_mode')
  })
})
