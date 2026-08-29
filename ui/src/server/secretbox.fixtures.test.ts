// Cross-language secretbox fixtures — the TS side of the tripwire.
//
// api/tests/fixtures/secretbox.json is the ONE artifact both languages assert
// against (Rust: api/tests/secretbox.rs). This test proves the committed file
// is exactly what the TS recipe produces TODAY: if secretbox.ts's crypto
// drifts (a dependency bump changing scrypt defaults, a rewrite of encRaw)
// without the fixture being regenerated, this fails instead of letting the two
// runtimes quietly disagree about what a token means.
//
// The generator runs production code — deriveKek from secretbox.ts — so this
// transitively pins the production derivation, not a test-only copy of it.

import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { test, expect } from 'vitest'
// Untyped .mjs at the repo root — no declaration file exists by design (the
// generator is a script, not a module other code links against).
// @ts-expect-error scripts/ is not under ui/'s tsconfig rootDirs
import { buildVectors, serialize } from '../../../scripts/gen-secretbox-vectors.mjs'

const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'api', 'tests', 'fixtures', 'secretbox.json')

test('committed secretbox vectors are what TS produces today', () => {
  const committed = readFileSync(FIXTURE, 'utf8')
  expect(committed).toBe(serialize(buildVectors()))
})

test('fixture covers every token grammar plus the DEK-wrap asymmetry', () => {
  // Shape guards: the Rust suite iterates these arrays, so a silently emptied
  // fixture would make both suites vacuously green.
  const v = buildVectors()
  const names = v.open.map((c: { name: string }) => c.name)
  expect(names).toContain('v1-kek-direct')
  expect(names).toContain('v2-versioned')
  expect(names).toContain('v2-legacy-active')
  expect(names).toContain('tampered-tag')
  expect(v.deks.length).toBeGreaterThanOrEqual(3)
  expect(v.seal.some((c: { wrap?: boolean }) => c.wrap)).toBe(true)
})
