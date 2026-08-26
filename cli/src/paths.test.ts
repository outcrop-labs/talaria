// Path/port plumbing: canonicalization kills symlink and `..` spellings (the
// devbox lesson), port slots scan, name regexes hold.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, symlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { canonicalDir, NAME_RE, portSlot, repoRoot } from './paths'
// (canonicalDir mkdirs its own targets; nothing else to prepare)

describe('canonicalDir', () => test('resolves symlinks and collapses ..', () => {
  const real = mkdtempSync(join(tmpdir(), 'talaria-real-'))
  const link = join(tmpdir(), `talaria-link-${Date.now()}`)
  symlinkSync(real, link)
  expect(canonicalDir(join(link, 'sub'))).toBe(join(real, 'sub'))
  expect(canonicalDir(join(real, 'a', '..', 'b'))).toBe(join(real, 'b'))
}))

describe('repoRoot', () => {
  test('walks up to .git', () => {
    // This file is at <root>/cli/src — the repo root is three up.
    const root = repoRoot(canonicalDir(import.meta.dir))
    expect(root.endsWith('talaria')).toBe(true)
  })
})

describe('NAME_RE', () => {
  test('accepts lowercase kebab, rejects the rest', () => {
    for (const good of ['demo', 'demo-2', 'a']) expect(NAME_RE.test(good)).toBe(true)
    for (const bad of ['Demo', '-demo', 'demo_', 'demo!', '']) expect(NAME_RE.test(bad)).toBe(false)
  })
})

describe('portSlot', () => {
  test('returns the first free port, skipping taken ones', async () => {
    const taken = (p: number) => Promise.resolve(p === 5301 || p === 5302)
    expect(await portSlot(5301, 5389, taken)).toBe(5303)
  })
  test('returns null when the range is exhausted', async () => {
    expect(await portSlot(5301, 5303, () => Promise.resolve(true))).toBeNull()
  })
})
