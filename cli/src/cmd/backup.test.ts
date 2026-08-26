// backup.sh's safety rails, as tests: retention (only manifest-carrying
// snapshot dirs, newest kept), the dest/keep resolution, the entry guards,
// and the partial-directory cleanup when anything throws. The full pipeline
// (dump → gzip → tar → manifest → sums → rename) is exercised live against
// the real dev DB — the file effects need real gzip/tar, which a fake ctx
// cannot produce.

import { describe, expect, test } from 'bun:test'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pruneSnapshots, runBackup } from './backup'
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

describe('pruneSnapshots', () => {
  const makeDest = (): string => {
    const dest = mkdtempSync(join(tmpdir(), 'talaria-prune-'))
    for (const name of ['20260101T000000Z', '20260102T000000Z', '20260103T000000Z']) {
      mkdirSync(join(dest, name))
      writeFileSync(join(dest, name, 'manifest.txt'), 'talaria_backup=1\n')
    }
    mkdirSync(join(dest, '20260104T000000Z')) // newer, but NOT ours
    mkdirSync(join(dest, 'not-a-snapshot')) // stray directory
    return dest
  }

  test('keeps the newest N, prunes older manifest-carrying dirs only', () => {
    const dest = makeDest()
    try {
      expect(pruneSnapshots(dest, 2)).toBe(1)
      expect(existsSync(join(dest, '20260103T000000Z'))).toBe(true)
      expect(existsSync(join(dest, '20260101T000000Z'))).toBe(false)
      // the manifest-less snapshot and the stray dir are not ours to delete
      expect(existsSync(join(dest, '20260104T000000Z'))).toBe(true)
      expect(existsSync(join(dest, 'not-a-snapshot'))).toBe(true)
    } finally {
      rmSync(dest, { recursive: true, force: true })
    }
  })

  test('keep 0 disables pruning', () => {
    const dest = makeDest()
    try {
      expect(pruneSnapshots(dest, 0)).toBe(0)
    } finally {
      rmSync(dest, { recursive: true, force: true })
    }
  })
})

describe('runBackup — guards', () => {
  test('neither docker nor pg_dump dies', async () => {
    const ctx = fakeCtx()
    ctx.plant(['docker', ['--version']], new Error('no docker'))
    ctx.plant(['pg_dump', ['--version']], new Error('no pg_dump'))
    const msg = await attempt(() => runBackup(ctx, '/tmp/x', 7))
    expect(msg).toContain('need either pg_dump on PATH or docker')
  })

  test('missing DATABASE_URL dies naming where it looked', async () => {
    const ctx = fakeCtx()
    ctx.root = mkdtempSync(join(tmpdir(), 'talaria-bk-'))
    try {
      const msg = await attempt(() => runBackup(ctx, '/tmp/x', 7))
      expect(msg).toContain('DATABASE_URL is not set')
      expect(msg).toContain('ui/.env')
    } finally {
      rmSync(ctx.root, { recursive: true, force: true })
    }
  })

  test('an existing snapshot dir for this stamp dies (no clobbering)', async () => {
    const ctx = fakeCtx()
    ctx.env.DATABASE_URL = 'postgres://u@h/db'
    // fakeCtx.now() is fixed → the stamp is deterministic
    const dest = mkdtempSync(join(tmpdir(), 'talaria-bk-'))
    mkdirSync(join(dest, '20260101T000000Z'))
    try {
      const msg = await attempt(() => runBackup(ctx, dest, 7))
      expect(msg).toContain('already exists')
    } finally {
      rmSync(dest, { recursive: true, force: true })
    }
  })

  test('a failing dump leaves no .partial behind — the trap, ported', async () => {
    const ctx = fakeCtx()
    ctx.env.DATABASE_URL = 'postgres://u@h/db'
    const dest = mkdtempSync(join(tmpdir(), 'talaria-bk-'))
    try {
      // The dump exec is planted-free (fake success, no real file written), so
      // the truncation check trips on the missing plain dump — a mid-run
      // failure after the staging dir exists.
      const msg = await attempt(() => runBackup(ctx, dest, 7))
      expect(msg).toContain('ENOENT')
      expect(existsSync(join(dest, '20260101T000000Z.partial'))).toBe(false)
      // the self-ignoring .gitignore DID land (it precedes the dump)
      expect(existsSync(join(dest, '.gitignore'))).toBe(true)
    } finally {
      rmSync(dest, { recursive: true, force: true })
    }
  })
})
