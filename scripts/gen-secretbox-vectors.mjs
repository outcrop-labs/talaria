#!/usr/bin/env node
// Cross-language fixtures for the secretbox port (ui/src/server/secretbox.ts ⇄
// api/src/secretbox.rs). The Rust port is only trustworthy if it opens what TS
// sealed and seals what TS would — byte-for-byte, both directions — so both
// suites assert against ONE committed file, and this script is the only thing
// that writes it.
//
// Deterministic by construction: fixed root material, fixed DEKs, fixed IVs.
// Nothing here is a secret; the root is a fixture literal, not production
// material, and the point is reproducibility across languages and machines.
//
// The KEK comes from production code (`deriveKek` in secretbox.ts) — the TS
// side of the contract is executed, not re-derived. The one primitive this
// script implements itself is enc-with-explicit-IV: production's encRaw rolls
// a random IV (correct at runtime, useless for fixtures), so this is its
// recipe with the randomness pinned.
//
// Usage:
//   bun run api:vectors           # write api/tests/fixtures/secretbox.json
//   bun run api:vectors --check   # fail if the committed file is stale
//
// The DEK-wrap asymmetry gets its own vectors on purpose: a DEK is wrapped as
// the STANDARD padded base64 STRING of its bytes (secretbox.ts wrapDek), not
// the raw bytes — the single most likely silent-corruption point in any port.

import { createCipheriv } from 'node:crypto'
import { readFileSync, writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { deriveKek } from '../ui/src/server/secretbox.ts'

const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), '..', 'api', 'tests', 'fixtures', 'secretbox.json')

// ── Deterministic material ────────────────────────────────────────────────────
const ROOT = 'talaria-vector-root-v1'
const KEK = deriveKek(ROOT)
const DEKS = [
  { version: 1, hex: '000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f' },
  { version: 2, hex: '202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f' },
  { version: 3, hex: '808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f', active: true },
]
const IVS = {
  wrap1: '000102030405060708090a0b',
  wrap2: '101112131415161718191a1b',
  wrap3: '202122232425262728292a2b',
  v1: '303132333435363738393a3b',
  v2ver: '404142434445464748494a4b',
  v2legacy: '505152535455565758595a5b',
  empty: '606162636465666768696a6b',
  seal3: '707172737475767778797a7b',
  sealWrap: '808182838485868788898a8b',
}

// ── encRaw's recipe with the IV pinned (see header) ──────────────────────────
function encWithIv(key, ivHex, plaintext) {
  const iv = Buffer.from(ivHex, 'hex')
  const cipher = createCipheriv('aes-256-gcm', key, iv)
  const data = Buffer.concat([cipher.update(plaintext, 'utf8'), cipher.final()])
  return {
    iv: iv.toString('base64url'),
    tag: cipher.getAuthTag().toString('base64url'),
    data: data.toString('base64url'),
  }
}
const dek = (v) => Buffer.from(DEKS.find((d) => d.version === v).hex, 'hex')

function sealWithIv(version, ivHex, plaintext) {
  const { iv, tag, data } = encWithIv(dek(version), ivHex, plaintext)
  return ['v2', String(version), iv, tag, data].join(':')
}
function v1WithIv(ivHex, plaintext) {
  const { iv, tag, data } = encWithIv(KEK, ivHex, plaintext)
  return ['v1', iv, tag, data].join(':')
}
// The asymmetry, verbatim from secretbox.ts wrapDek: plaintext is the standard
// PADDED base64 string of the DEK bytes.
function wrapDekWithIv(version, ivHex) {
  const { iv, tag, data } = encWithIv(KEK, ivHex, dek(version).toString('base64'))
  return ['v1', iv, tag, data].join(':')
}

function tamper(token) {
  // Flip one character of the TAG part — a body a real attacker could produce.
  const parts = token.split(':')
  const tag = parts[parts.length - 2]
  const at = tag.length - 2
  parts[parts.length - 2] = tag.slice(0, at) + (tag[at] === 'A' ? 'B' : 'A') + tag.slice(at + 1)
  return parts.join(':')
}

// ── The fixture ───────────────────────────────────────────────────────────────
export function buildVectors() {
  const v2versioned = sealWithIv(2, IVS.v2ver, 'sk-ant-api03-fixture-provider-key')
  return {
    _comment:
      'Cross-language secretbox fixtures — TS (node:crypto via secretbox.ts deriveKek) ' +
      'is the producer, Rust (api/src/secretbox.rs) must byte-match. ' +
      'Regenerate with: bun run api:vectors',
    root: ROOT,
    kek_hex: KEK.toString('hex'),
    deks: DEKS.map(({ version, hex, active }) => ({
      version,
      dek_hex: hex,
      active: Boolean(active),
      wrap_iv_hex: IVS[`wrap${version}`],
      wrapped: wrapDekWithIv(version, IVS[`wrap${version}`]),
    })),
    open: [
      { name: 'v1-kek-direct', token: v1WithIv(IVS.v1, 'legacy-kek-direct-secret'), plaintext: 'legacy-kek-direct-secret' },
      { name: 'v2-versioned', token: v2versioned, plaintext: 'sk-ant-api03-fixture-provider-key' },
      {
        name: 'v2-legacy-active',
        token: (() => {
          const { iv, tag, data } = encWithIv(dek(3), IVS.v2legacy, 'país ✓ ünïcode')
          return ['v2', iv, tag, data].join(':')
        })(),
        plaintext: 'país ✓ ünïcode',
      },
      { name: 'empty-plaintext', token: sealWithIv(1, IVS.empty, ''), plaintext: '' },
      { name: 'tampered-tag', token: tamper(v2versioned), plaintext: null },
    ],
    seal: [
      {
        name: 'v2-with-active-dek',
        version: 3,
        iv_hex: IVS.seal3,
        plaintext: 'sk-fixture-seal-target',
        token: sealWithIv(3, IVS.seal3, 'sk-fixture-seal-target'),
      },
      {
        name: 'dek-wrap-base64-string-asymmetry',
        wrap: true,
        iv_hex: IVS.sealWrap,
        version: 2,
        token: wrapDekWithIv(2, IVS.sealWrap),
      },
    ],
  }
}

export function serialize(vectors) {
  return JSON.stringify(vectors, null, 2) + '\n'
}

// ── CLI ───────────────────────────────────────────────────────────────────────
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const canonical = serialize(buildVectors())
  if (process.argv.includes('--check')) {
    const committed = readFileSync(FIXTURE, 'utf8')
    if (committed !== canonical) {
      console.error(
        'api/tests/fixtures/secretbox.json is stale — regenerate with: bun run api:vectors\n' +
          '(the Rust suite and ui vitest both assert against this file; it must match the TS recipe exactly)',
      )
      process.exit(1)
    }
    console.log('secretbox vectors: committed fixture is current')
  } else {
    writeFileSync(FIXTURE, canonical)
    console.log(`wrote ${FIXTURE}`)
  }
}
