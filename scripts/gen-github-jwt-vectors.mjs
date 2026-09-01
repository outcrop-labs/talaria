#!/usr/bin/env node
// Cross-language fixtures for the GitHub app JWT port (ui/src/server/github.ts
// ⇄ api/src/github.rs). The JWT is the one bespoke construction in the token
// half: header/payload bytes are JSON.stringify literals (key order, no
// spaces), iat is backdated 60s, exp is +9m, base64url unpadded — and the
// signature must verify identically whether Node's createSign or the RustCrypto
// rsa crate made it. Both suites assert against ONE committed file; this
// script is the only thing that writes it.
//
// The keypair is fixture material, not production material: generated once,
// committed, and reused forever so the vectors are reproducible. A first run
// on a checkout missing the PEM would have nothing deterministic to sign with,
// so the script refuses rather than silently minting a second key — see the
// error at the bottom for how to regenerate deliberately.
//
// Usage:
//   bun run api:vectors           # write api/tests/fixtures/github-jwt.json
//   bun run api:vectors --check   # fail if the committed file is stale

import { createSign, createVerify } from 'node:crypto'
import { readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { appJwtAt } from '../ui/src/server/github.ts'

const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), '..', 'api', 'tests', 'fixtures', 'github-jwt.json')

// The committed PEM is the root of the fixture — read it back, never re-roll.
const pem = JSON.parse(readFileSync(FIXTURE, 'utf8')).privateKeyPem

// ── Vectors ───────────────────────────────────────────────────────────────────
// Three clock instants across the year 2038 boundary, two app ids — enough to
// pin the offsets, the key order, and the padding of both JSON bodies.
const CASES = [
  { name: 'before-2038', appId: '123456', nowSecs: 1_800_000_000 },
  { name: 'after-2038', appId: '123456', nowSecs: 2_200_000_000 },
  { name: 'long-iss', appId: 'Iv1.abc123def456', nowSecs: 1_760_000_000 },
]

function buildVectors() {
  const cases = CASES.map(({ name, appId, nowSecs }) => {
    const jwt = appJwtAt(appId, pem, nowSecs)
    // The signature must verify against the same PEM — a vector whose own
    // signature fails is a generator bug, not a port drift.
    const [h, p, s] = jwt.split('.')
    const unsigned = `${h}.${p}`
    const verified = createVerify('RSA-SHA256').update(unsigned).verify(pem, Buffer.from(s, 'base64url'))
    return { name, appId, nowSecs, jwt, verified }
  })
  return { privateKeyPem: pem, cases }
}

function serialize(vectors) {
  return JSON.stringify(vectors, null, 2) + '\n'
}

// ── CLI ───────────────────────────────────────────────────────────────────────
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const canonical = serialize(buildVectors())
  if (process.argv.includes('--check')) {
    const committed = readFileSync(FIXTURE, 'utf8')
    if (committed !== canonical) {
      console.error(
        'api/tests/fixtures/github-jwt.json is stale — regenerate with: bun run api:vectors\n' +
          '(the Rust suite asserts against this file; it must match the TS recipe exactly)',
      )
      process.exit(1)
    }
    console.log('github-jwt vectors: committed fixture is current')
  } else {
    writeFileSync(FIXTURE, canonical)
    console.log(`wrote ${FIXTURE}`)
  }
}
