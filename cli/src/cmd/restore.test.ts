// restore.sh's decision table, as tests: the snapshot gate, checksum
// verification, the typed-confirm discipline (wrong word → zero destructive
// calls; non-TTY without --yes refuses), the streaming restore argv, and the
// two upload halves. Snapshots on disk are REAL (real gzip bytes, real
// checksums) so verification is exercised for true; process behavior is
// planted.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { gzipSync } from 'node:zlib'
import { runRestore, worktreeRestoreProblem } from './restore'
import { writeSums } from '../backup/lib'
import { fakeCtx } from '../testing'
import { CliError } from '../ui'

const attempt = async (fn: () => Promise<unknown>): Promise<string> => {
  try {
    await fn()
    return ''
  } catch (e) {
    if (e instanceof CliError) return e.message
    if (e instanceof Error) return `<${e.name}: ${e.message}>`
    return String(e)
  }
}

/** A real snapshot directory: genuine gzip bytes and matching checksums. */
const makeSnap = async (over: { manifest?: string; tamper?: boolean } = {}): Promise<string> => {
  const snap = mkdtempSync(join(tmpdir(), 'talaria-snap-'))
  writeFileSync(join(snap, 'db.sql.gz'), gzipSync('-- dumped\n\nPostgreSQL database dump complete\n'))
  writeFileSync(join(snap, 'uploads.tar.gz'), 'pretend tarball')
  writeFileSync(
    join(snap, 'manifest.txt'),
    over.manifest ??
      'talaria_backup=1\ncreated_at=2026-07-31T18:44:21Z\ncreated_on=here\ndatabase=127.0.0.1:5544/talaria\nstorage_mode=local\nstorage_endpoint=\nstorage_bucket=\nstorage_prefix=\nuploads_dir=\nblob_count=0\n',
  )
  await writeSums(snap, ['db.sql.gz', 'uploads.tar.gz', 'manifest.txt'])
  if (over.tamper) writeFileSync(join(snap, 'db.sql.gz'), 'tampered')
  return snap
}

const TARGET = 'postgres://talaria:talaria@127.0.0.1:5599/talaria'

describe('runRestore — gates', () => {
  test('a directory without a manifest is refused', async () => {
    const notSnap = mkdtempSync(join(tmpdir(), 'talaria-nosnap-'))
    try {
      const ctx = fakeCtx()
      const msg = await attempt(() => runRestore(ctx, notSnap, { what: 'db', yes: true, target: TARGET }))
      expect(msg).toContain('not a Talaria snapshot')
    } finally {
      rmSync(notSnap, { recursive: true, force: true })
    }
  })

  test('a tampered snapshot dies at the checksum, before any confirm or restore', async () => {
    const snap = await makeSnap({ tamper: true })
    try {
      const ctx = fakeCtx({ isTTY: true, reply: 'restore' })
      const msg = await attempt(() => runRestore(ctx, snap, { what: 'db', yes: true, target: TARGET }))
      expect(msg).toContain('checksum mismatch')
      expect(ctx.calls.length).toBe(0) // not even psql was probed
    } finally {
      rmSync(snap, { recursive: true, force: true })
    }
  })

  test('no target anywhere dies telling you the two places it looked', async () => {
    const snap = await makeSnap()
    try {
      const ctx = fakeCtx()
      ctx.root = mkdtempSync(join(tmpdir(), 'talaria-rs-'))
      try {
        const msg = await attempt(() => runRestore(ctx, snap, { what: 'db' }))
        expect(msg).toContain('no target database')
      } finally {
        rmSync(ctx.root, { recursive: true, force: true })
      }
    } finally {
      rmSync(snap, { recursive: true, force: true })
    }
  })

  test('non-interactive without --yes refuses; the typed word is required', async () => {
    const snap = await makeSnap()
    try {
      const ctx = fakeCtx({ isTTY: false })
      const msg = await attempt(() => runRestore(ctx, snap, { what: 'db', target: TARGET }))
      expect(msg).toContain('non-interactively without --yes')
      expect(ctx.calls.length).toBe(0)
    } finally {
      rmSync(snap, { recursive: true, force: true })
    }
  })

  test('the wrong typed word aborts with zero destructive calls', async () => {
    const snap = await makeSnap()
    try {
      const ctx = fakeCtx({ isTTY: true, reply: 'nope' })
      const msg = await attempt(() => runRestore(ctx, snap, { what: 'all', target: TARGET }))
      expect(msg).toBe('aborted')
      // nothing ran: no gunzip, no psql, no tar
      expect(ctx.calls.filter((c) => c.cmd === 'gunzip' || c.cmd === 'psql' || c.cmd === 'tar')).toHaveLength(0)
    } finally {
      rmSync(snap, { recursive: true, force: true })
    }
  })
})

describe('runRestore — the restore itself', () => {
  test('db half streams gunzip | psql with ON_ERROR_STOP', async () => {
    const snap = await makeSnap()
    try {
      const ctx = fakeCtx({ isTTY: true, reply: 'restore' })
      await runRestore(ctx, snap, { what: 'db', target: TARGET })
      const psql = ctx.calls.find((c) => c.cmd === 'psql' && c.args.includes('/dev/null'))!
      expect(psql.args).toEqual([TARGET, '-v', 'ON_ERROR_STOP=1', '-q', '-o', '/dev/null'])
      const gunzip = ctx.calls.find((c) => c.cmd === 'gunzip')!
      expect(gunzip.args).toEqual(['-c', join(snap, 'db.sql.gz')])
      // --db-only never touches the blobs
      expect(ctx.calls.some((c) => c.cmd === 'tar' && c.args.includes('-xzf'))).toBe(false)
    } finally {
      rmSync(snap, { recursive: true, force: true })
    }
  })

  test('uploads half extracts into the configured local dir', async () => {
    const snap = await makeSnap()
    const blobs = mkdtempSync(join(tmpdir(), 'talaria-blobs-'))
    try {
      const ctx = fakeCtx({ isTTY: true, reply: 'restore' })
      ctx.env.TALARIA_UPLOADS_DIR = blobs
      await runRestore(ctx, snap, { what: 'uploads', target: TARGET })
      const tar = ctx.calls.find((c) => c.cmd === 'tar')!
      expect(tar.args).toEqual(['-xzf', join(snap, 'uploads.tar.gz'), '-C', blobs])
      // --uploads-only never streams the database
      expect(ctx.calls.some((c) => c.cmd === 'gunzip')).toBe(false)
    } finally {
      rmSync(snap, { recursive: true, force: true })
      rmSync(blobs, { recursive: true, force: true })
    }
  })

  test('a failed psql dies with the incomplete-state warning', async () => {
    const snap = await makeSnap()
    try {
      const ctx = fakeCtx({ isTTY: true, reply: 'restore' })
      ctx.plant(['psql', [TARGET, '-v', 'ON_ERROR_STOP=1', '-q', '-o', '/dev/null']], new Error('relation missing'))
      const msg = await attempt(() => runRestore(ctx, snap, { what: 'db', target: TARGET }))
      expect(msg).toContain('incomplete state')
    } finally {
      rmSync(snap, { recursive: true, force: true })
    }
  })

  test('an s3 snapshot without out-of-band creds dies at storage resolution', async () => {
    const snap = await makeSnap({ manifest: 'talaria_backup=1\nstorage_mode=s3\nstorage_endpoint=https://s3\nstorage_bucket=b\nstorage_prefix=\n' })
    try {
      const ctx = fakeCtx({ isTTY: true, reply: 'restore' })
      const msg = await attempt(() => runRestore(ctx, snap, { what: 'uploads', target: TARGET }))
      expect(msg).toContain('TALARIA_BACKUP_S3_SECRET_KEY')
    } finally {
      rmSync(snap, { recursive: true, force: true })
    }
  })

  test('an s3 snapshot with creds copies into the bucket', async () => {
    const snap = await makeSnap({ manifest: 'talaria_backup=1\nstorage_mode=s3\nstorage_endpoint=https://s3\nstorage_bucket=b\nstorage_prefix=p/\n' })
    try {
      const ctx = fakeCtx({
        isTTY: true,
        reply: 'restore',
        env: { TALARIA_BACKUP_S3_ACCESS_KEY: 'AK', TALARIA_BACKUP_S3_SECRET_KEY: 'SK' },
      })
      await runRestore(ctx, snap, { what: 'uploads', target: TARGET })
      const copy = ctx.calls.find((c) => c.cmd === 'rclone' && c.args.includes('copy'))!
      expect(copy.args).toContain('t:b/p/uploads')
      // No alias step to assert any more — the remote is pure env, so what
      // matters is that rclone will dial the endpoint the MANIFEST named.
      expect(ctx.env.RCLONE_CONFIG_T_ENDPOINT).toBe('https://s3')
      expect(ctx.calls.some((c) => c.args.includes('sync'))).toBe(false)
    } finally {
      rmSync(snap, { recursive: true, force: true })
    }
  })

  test('all extracts app-data.tar.gz when present', async () => {
    const snap = await makeSnap()
    writeFileSync(join(snap, 'app-data.tar.gz'), 'pretend app data')
    await writeSums(snap, ['db.sql.gz', 'uploads.tar.gz', 'app-data.tar.gz', 'manifest.txt'])
    const dest = mkdtempSync(join(tmpdir(), 'talaria-app-data-'))
    try {
      const ctx = fakeCtx({ isTTY: true, reply: 'restore' })
      ctx.env.TALARIA_APP_DATA_DIR = dest
      ctx.env.TALARIA_UPLOADS_DIR = dest
      await runRestore(ctx, snap, { what: 'all', target: TARGET })
      const tars = ctx.calls.filter((c) => c.cmd === 'tar' && c.args.includes('-xzf'))
      expect(tars.some((c) => c.args.includes(join(snap, 'app-data.tar.gz')) && c.args.includes(dest))).toBe(true)
    } finally {
      rmSync(snap, { recursive: true, force: true })
      rmSync(dest, { recursive: true, force: true })
    }
  })

})

describe('runRestore — inside a worktree, only the worktree\'s own targets', () => {
  const WT_DB = 'postgres://talaria:talaria@127.0.0.1:5605/talaria'
  const wtRoot = (): string => {
    const root = mkdtempSync(join(tmpdir(), 'talaria-rs-wt-'))
    mkdirSync(join(root, 'ui'), { recursive: true })
    writeFileSync(join(root, 'ui/.env'), `TALARIA_WORKTREE=demo\nDATABASE_URL=${WT_DB}\n`)
    return root
  }
  const manifest = (mode: string, bucket = '') =>
    `talaria_backup=1\nstorage_mode=${mode}\nstorage_endpoint=http://127.0.0.1:9010\nstorage_bucket=${bucket}\nstorage_prefix=\n`

  test("a shell DATABASE_URL that is not ui/.env's, with no --target, is refused", () => {
    const ctx = fakeCtx({ env: { DATABASE_URL: 'postgres://talaria:talaria@127.0.0.1:5544/talaria' } })
    ctx.root = wtRoot()
    const msg = worktreeRestoreProblem(ctx, ctx.env, manifest('local'), { what: 'db' })
    expect(msg).toContain('127.0.0.1:5544/talaria')
    expect(msg).toContain('Pass --target')
    // an explicit --target is the operator naming it: no guess left
    expect(worktreeRestoreProblem(ctx, ctx.env, manifest('local'), { what: 'db', target: WT_DB })).toBeNull()
  })

  test("a TALARIA_ENV_FILE pointed at main's ui/.env neither hides the worktree nor picks its database", () => {
    // It used to: the marker was read from TALARIA_ENV_FILE, main's file has
    // none, and every worktree rule switched off with main's DATABASE_URL as
    // the target.
    const mainEnv = join(mkdtempSync(join(tmpdir(), 'talaria-rs-main-')), '.env')
    writeFileSync(mainEnv, 'DATABASE_URL=postgres://talaria:talaria@127.0.0.1:5544/talaria\n')
    const ctx = fakeCtx({ env: { TALARIA_ENV_FILE: mainEnv } })
    ctx.root = wtRoot()
    const msg = worktreeRestoreProblem(ctx, {}, manifest('local'), { what: 'db' })
    expect(msg).toContain('worktree "demo"')
    expect(msg).toContain(`TALARIA_ENV_FILE=${mainEnv}`)
    expect(msg).toContain('127.0.0.1:5544/talaria')
    expect(msg).not.toContain('talaria:talaria@')
    // --target names the database outright
    expect(worktreeRestoreProblem(ctx, {}, manifest('local'), { what: 'db', target: WT_DB })).toBeNull()
    // the shell wins over the file: a shell DATABASE_URL that is ui/.env's own is the target
    const shellOwn = fakeCtx({ env: { TALARIA_ENV_FILE: mainEnv, DATABASE_URL: WT_DB } })
    shellOwn.root = ctx.root
    expect(worktreeRestoreProblem(shellOwn, {}, manifest('local'), { what: 'db' })).toBeNull()
    // an env file naming the worktree's own database is no guess
    const same = join(ctx.root, 'alt.env')
    writeFileSync(same, `DATABASE_URL="${WT_DB}"\n`)
    const sameCtx = fakeCtx({ env: { TALARIA_ENV_FILE: same } })
    sameCtx.root = ctx.root
    expect(worktreeRestoreProblem(sameCtx, {}, manifest('local'), { what: 'db' })).toBeNull()
  })

  test("internal-mode blobs would land in main's object storage: refused, --db-only passes", () => {
    const ctx = fakeCtx()
    ctx.root = wtRoot()
    expect(worktreeRestoreProblem(ctx, {}, manifest('internal', 'talaria'), { what: 'all' })).toContain("main's bucket")
    expect(worktreeRestoreProblem(ctx, {}, manifest('internal', 'talaria'), { what: 'uploads' })).toContain("main's bucket")
    expect(worktreeRestoreProblem(ctx, {}, manifest('internal', 'talaria'), { what: 'db' })).toBeNull()
  })

  test('s3-mode blobs need a bucket the operator names', () => {
    const ctx = fakeCtx()
    ctx.root = wtRoot()
    expect(worktreeRestoreProblem(ctx, {}, manifest('s3', 'prod-bucket'), { what: 'uploads' })).toContain('prod-bucket')
    const named = fakeCtx({ env: { TALARIA_BACKUP_S3_BUCKET: 'drill' } })
    named.root = ctx.root
    expect(worktreeRestoreProblem(named, named.env, manifest('s3', 'prod-bucket'), { what: 'uploads' })).toBeNull()
  })

  test("local blobs and app data land in the worktree's own dirs; an inherited outside dir is refused", () => {
    const ctx = fakeCtx()
    ctx.root = wtRoot()
    expect(worktreeRestoreProblem(ctx, {}, manifest('local'), { what: 'all' })).toBeNull()
    const outside = mkdtempSync(join(tmpdir(), 'talaria-main-uploads-'))
    expect(worktreeRestoreProblem(ctx, { TALARIA_UPLOADS_DIR: outside }, manifest('local'), { what: 'uploads' })).toContain(outside)
    expect(worktreeRestoreProblem(ctx, { TALARIA_APP_DATA_DIR: outside }, manifest('local'), { what: 'all' })).toContain(outside)
    expect(worktreeRestoreProblem(ctx, { TALARIA_UPLOADS_DIR: join(ctx.root, 'x') }, manifest('local'), { what: 'uploads' })).toBeNull()
  })

  test('the primary (no marker) is unchanged: env wins, every mode passes', () => {
    const ctx = fakeCtx({ env: { DATABASE_URL: 'postgres://elsewhere/db' } })
    ctx.root = mkdtempSync(join(tmpdir(), 'talaria-rs-primary-'))
    expect(worktreeRestoreProblem(ctx, ctx.env, manifest('internal', 'talaria'), { what: 'all' })).toBeNull()
  })

  test('runRestore refuses before the checksum, the confirm or any exec', async () => {
    const snap = await makeSnap({ manifest: manifest('internal', 'talaria') })
    try {
      const ctx = fakeCtx({ isTTY: true, reply: 'restore' })
      ctx.root = wtRoot()
      const msg = await attempt(() => runRestore(ctx, snap, { what: 'all' }))
      expect(msg).toContain("main's bucket")
      expect(msg).toContain('Nothing was changed')
      expect(ctx.calls).toHaveLength(0)
    } finally {
      rmSync(snap, { recursive: true, force: true })
    }
  })

  test("the shim run from inside another checkout refuses before anything", async () => {
    const snap = await makeSnap()
    try {
      const mk = (): string => {
        const r = mkdtempSync(join(tmpdir(), 'talaria-rs-co-'))
        mkdirSync(join(r, '.git'))
        mkdirSync(join(r, 'cli/bin'), { recursive: true })
        writeFileSync(join(r, 'cli/bin/talaria.ts'), '')
        return r
      }
      const primary = mk()
      const standing = mk()
      const ctx = fakeCtx({ isTTY: true, reply: 'restore', cwd: standing })
      ctx.root = primary
      const msg = await attempt(() => runRestore(ctx, snap, { what: 'db', target: TARGET }))
      expect(msg).toContain(`cd ${standing} && bun talaria restore`)
      expect(ctx.calls).toHaveLength(0)
    } finally {
      rmSync(snap, { recursive: true, force: true })
    }
  })
})
