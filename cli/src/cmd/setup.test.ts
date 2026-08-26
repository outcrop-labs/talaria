// The PATH shim `talaria setup` installs — the pure part of setup worth
// pinning: where it lands, what it says, and that it stays executable.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, readFileSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { installShim, shimDir } from './setup'
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
