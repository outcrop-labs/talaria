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
import { runRestore } from './restore'
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

  test('an s3 snapshot with creds mirrors into the bucket', async () => {
    const snap = await makeSnap({ manifest: 'talaria_backup=1\nstorage_mode=s3\nstorage_endpoint=https://s3\nstorage_bucket=b\nstorage_prefix=p/\n' })
    try {
      const ctx = fakeCtx({
        isTTY: true,
        reply: 'restore',
        env: { TALARIA_BACKUP_S3_ACCESS_KEY: 'AK', TALARIA_BACKUP_S3_SECRET_KEY: 'SK' },
      })
      await runRestore(ctx, snap, { what: 'uploads', target: TARGET })
      const mirror = ctx.calls.find((c) => c.cmd === 'mc' && c.args.includes('mirror'))!
      expect(mirror.args).toContain('t/b/p/uploads')
      // the alias was set against the manifest's endpoint first
      expect(ctx.calls.some((c) => c.cmd === 'mc' && c.args.includes('alias') && c.args.includes('https://s3'))).toBe(true)
    } finally {
      rmSync(snap, { recursive: true, force: true })
    }
  })
})
