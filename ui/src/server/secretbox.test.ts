import { randomBytes } from 'node:crypto'
import type { Sql } from 'postgres'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// The KEK is derived from the environment the FIRST time it's needed and then
// memoised on globalThis, so the root secret has to be in place before the
// module is imported — hence the assignment above the import.
process.env.TALARIA_SECRET_KEY = 'test-root-secret-do-not-use-in-anger'
delete process.env.TALARIA_SECRET_KEY_FILE

const sb = await import('@/server/secretbox')

// Why a stubbed tagged template rather than a real Postgres: initSecretbox is
// three statements' worth of DB contact (select every key row, insert the first
// one) wrapped around the crypto we actually care about. A stub lets us drive
// the interesting shapes — no rows, several versions, a row wrapped under a
// DIFFERENT root secret — which a live DB would make slow and awkward to set up,
// and it lets us assert the exact SQL effects (that a first run really does
// insert version 1). Nothing here is mocked that has behaviour worth testing.
interface KeyRow {
  version: number
  wrappedDek: string
  active: boolean
}
function fakeSql(rows: KeyRow[]) {
  const inserts: unknown[][] = []
  const fn = (strings: TemplateStringsArray, ...values: unknown[]): Promise<unknown[]> => {
    const text = strings.join(' ')
    if (/insert\s+into\s+secret_keys/i.test(text)) {
      inserts.push(values)
      return Promise.resolve([])
    }
    if (/from\s+secret_keys/i.test(text)) return Promise.resolve(rows)
    throw new Error(`unexpected SQL in test: ${text}`)
  }
  return Object.assign(fn as unknown as Sql, { inserts })
}

// The module's key material lives on globalThis (so Vite HMR can't wipe it).
// Clearing it between tests is what makes each case independent.
type SecretboxGlobals = { __sbKek?: Buffer; __sbDeks?: Map<number, Buffer>; __sbActive?: number; __sbFailure?: string }
const g = globalThis as unknown as SecretboxGlobals
const reset = () => {
  delete g.__sbKek
  g.__sbDeks = new Map()
  delete g.__sbActive
  delete g.__sbFailure
}

/** A wrapped-DEK row as the DB would hold it, produced by the module itself. */
const rowFor = (version: number, dek: Buffer, active: boolean, rootMaterial?: string): KeyRow => ({
  version,
  wrappedDek: sb.wrapDekFor(dek, rootMaterial),
  active,
})

beforeEach(reset)
afterEach(() => {
  vi.restoreAllMocks()
})

describe('initSecretbox', () => {
  it('creates and installs version 1 when the table is empty', async () => {
    const sql = fakeSql([])
    await sb.initSecretbox(sql)

    expect(sql.inserts).toHaveLength(1)
    // version/active are literals in the statement; the wrapped DEK is the only
    // interpolated value.
    const [wrapped] = sql.inserts[0]!
    expect(String(wrapped)).toMatch(/^v1:/) // DEKs are wrapped KEK-direct
    expect(sb.currentKeyVersion()).toBe(1)
    expect(sb.loadedVersions()).toEqual([1])
  })

  it('loads every stored version and honours the active flag', async () => {
    const d1 = sb.newDek()
    const d2 = sb.newDek()
    const d3 = sb.newDek()
    await sb.initSecretbox(fakeSql([rowFor(1, d1, false), rowFor(2, d2, true), rowFor(3, d3, false)]))

    expect(sb.loadedVersions().sort()).toEqual([1, 2, 3])
    // Not simply "the highest version" — the DB says v2 is active.
    expect(sb.currentKeyVersion()).toBe(2)
  })

  it('falls back to the highest version when no row is marked active', async () => {
    await sb.initSecretbox(fakeSql([rowFor(1, sb.newDek(), false), rowFor(7, sb.newDek(), false)]))
    expect(sb.currentKeyVersion()).toBe(7)
  })

  it('inserts nothing when rows already exist', async () => {
    const sql = fakeSql([rowFor(1, sb.newDek(), true)])
    await sb.initSecretbox(sql)
    expect(sql.inserts).toHaveLength(0)
  })

  it('is idempotent — a second call does not disturb loaded keys', async () => {
    const d1 = sb.newDek()
    const rows = [rowFor(1, d1, true)]
    await sb.initSecretbox(fakeSql(rows))
    const token = sb.seal('hello')
    await sb.initSecretbox(fakeSql(rows))
    expect(sb.open(token)).toBe('hello')
  })

  it('survives a key it cannot unwrap: complains, keeps the others usable', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {})
    const good = sb.newDek()
    await sb.initSecretbox(
      fakeSql([
        // v1 was wrapped when the operator's root secret was something else.
        rowFor(1, sb.newDek(), false, 'a-completely-different-root-secret'),
        rowFor(2, good, true),
      ]),
    )

    expect(err).toHaveBeenCalledWith(expect.stringContaining('cannot unwrap data key v1'))
    expect(sb.loadedVersions()).toEqual([2])
    expect(sb.currentKeyVersion()).toBe(2)
    expect(sb.open(sb.seal('still works'))).toBe('still works')
  })

  it('records the reason rather than throwing when NO key can be unwrapped', async () => {
    // initSecretbox runs INSIDE the migration pass, so throwing here rejected
    // every db() call and took down boards, teams and agents — none of which
    // touch a secret. It happened once. The failure is recorded instead, and
    // surfaces on the operations that actually need a key.
    const err = vi.spyOn(console, 'error').mockImplementation(() => {})
    await sb.initSecretbox(fakeSql([rowFor(1, sb.newDek(), true, 'some-other-root')]))
    expect(err).toHaveBeenCalledWith(expect.stringContaining('none can be unwrapped'))
    // And it names the ROOT SECRET, not the migration runner that ran fine.
    expect(() => sb.seal('x')).toThrow(/root secret/)
    expect(sb.secretboxFailure()).toMatch(/none can be unwrapped/)
  })

  it('clears a recorded failure once a readable key exists', async () => {
    // Recovery path: unreadable keys, then the operator clears them (Admin →
    // Secrets or reset.sh) and the next boot mints a fresh one. A stale
    // diagnosis surviving that would make a healthy instance keep refusing.
    vi.spyOn(console, 'error').mockImplementation(() => {})
    await sb.initSecretbox(fakeSql([rowFor(1, sb.newDek(), true, 'some-other-root')]))
    expect(sb.secretboxFailure()).not.toBeNull()
    await sb.initSecretbox(fakeSql([]))
    expect(sb.secretboxFailure()).toBeNull()
    expect(sb.open(sb.seal('works again'))).toBe('works again')
  })
})

describe('seal / open round-trip', () => {
  beforeEach(async () => {
    await sb.initSecretbox(fakeSql([]))
  })

  it('round-trips and stamps the active version into the token', () => {
    const token = sb.seal('sk-ant-super-secret')
    expect(token.startsWith('v2:1:')).toBe(true)
    expect(token.split(':')).toHaveLength(5)
    expect(sb.open(token)).toBe('sk-ant-super-secret')
  })

  it('round-trips unicode, empty strings and long payloads', () => {
    for (const plaintext of ['', 'ünïcødé 🔐 secret', 'x'.repeat(100_000), JSON.stringify({ a: [1, 2, 3] })]) {
      expect(sb.open(sb.seal(plaintext))).toBe(plaintext)
    }
  })

  it('produces a different ciphertext every time (random IV), not an ECB-style fingerprint', () => {
    const a = sb.seal('same plaintext')
    const b = sb.seal('same plaintext')
    expect(a).not.toBe(b)
    expect(sb.open(a)).toBe(sb.open(b))
  })

  it('never leaks the plaintext into the token', () => {
    expect(sb.seal('correct horse battery staple')).not.toContain('correct')
  })
})

describe('open — token formats it must still decrypt', () => {
  it('v1 (KEK-direct, legacy)', async () => {
    await sb.initSecretbox(fakeSql([]))
    const dek = sb.newDek()
    // wrapDekFor produces exactly the v1 shape: KEK-direct AES-GCM.
    const v1 = sb.wrapDekFor(dek)
    expect(v1.split(':')).toHaveLength(4)
    expect(v1.startsWith('v1:')).toBe(true)
    expect(sb.open(v1)).toBe(dek.toString('base64'))
  })

  it('v2 unversioned (legacy, pre-versioning) — decrypts under the active DEK', async () => {
    await sb.initSecretbox(fakeSql([]))
    const [, , iv, tag, data] = sb.seal('legacy payload').split(':')
    const legacy = ['v2', iv, tag, data].join(':')
    expect(legacy.split(':')).toHaveLength(4)
    expect(sb.open(legacy)).toBe('legacy payload')
  })

  it('v2 versioned — decrypts with the named version, not the active one', async () => {
    const d1 = sb.newDek()
    const d2 = sb.newDek()
    await sb.initSecretbox(fakeSql([rowFor(1, d1, false), rowFor(2, d2, true)]))

    const oldToken = sb.sealWith(d1, 1, 'sealed under v1')
    expect(oldToken.startsWith('v2:1:')).toBe(true)
    expect(sb.currentKeyVersion()).toBe(2)
    // The whole point of versioning: a token sealed under a retired key still
    // opens, and is NOT attempted with the active key.
    expect(sb.open(oldToken)).toBe('sealed under v1')
  })

  it('a legacy unversioned token sealed under an OLD key fails once the active key moves', async () => {
    const d1 = sb.newDek()
    const d2 = sb.newDek()
    await sb.initSecretbox(fakeSql([rowFor(1, d1, true)]))
    const [, , iv, tag, data] = sb.seal('pre-rotation').split(':')
    const legacy = ['v2', iv, tag, data].join(':')

    sb.installActiveKey(d2, 2)
    // Documented consequence of the unversioned format: there is nothing in the
    // token to say which key made it, so it is only readable while v1 is active.
    expect(() => sb.open(legacy)).toThrow()
  })
})

describe('open — rejection', () => {
  beforeEach(async () => {
    await sb.initSecretbox(fakeSql([]))
  })

  it('rejects a tampered ciphertext (GCM authentication)', () => {
    const parts = sb.seal('do not modify me').split(':')
    const data = Buffer.from(parts[4]!, 'base64url')
    data[0] = data[0]! ^ 0xff
    parts[4] = data.toString('base64url')
    expect(() => sb.open(parts.join(':'))).toThrow()
  })

  it('rejects a tampered auth tag', () => {
    const parts = sb.seal('do not modify me').split(':')
    const tag = Buffer.from(parts[3]!, 'base64url')
    tag[0] = tag[0]! ^ 0xff
    parts[3] = tag.toString('base64url')
    expect(() => sb.open(parts.join(':'))).toThrow()
  })

  it('rejects a tampered IV', () => {
    const parts = sb.seal('do not modify me').split(':')
    const iv = Buffer.from(parts[2]!, 'base64url')
    iv[0] = iv[0]! ^ 0xff
    parts[2] = iv.toString('base64url')
    expect(() => sb.open(parts.join(':'))).toThrow()
  })

  it('rejects ciphertext moved onto a different token (no cross-token splicing)', () => {
    const a = sb.seal('secret A').split(':')
    const b = sb.seal('secret B').split(':')
    a[4] = b[4]! // A's iv/tag with B's data
    expect(() => sb.open(a.join(':'))).toThrow()
  })

  it('rejects an unknown version prefix', () => {
    for (const bad of ['v3:a:b:c:d', 'v0:a:b:c', 'plaintext', '', 'v2:a:b']) {
      expect(() => sb.open(bad)).toThrow(/unrecognized token/)
    }
  })

  it('rejects a v2 token naming a version that is not loaded', () => {
    const parts = sb.seal('x').split(':')
    parts[1] = '99'
    expect(() => sb.open(parts.join(':'))).toThrow(/data key v99 not loaded/)
  })

  it('rejects a v1 token wrapped under a different root secret', () => {
    const foreign = sb.wrapDekFor(sb.newDek(), 'a-different-root-secret')
    expect(() => sb.open(foreign)).toThrow()
  })
})

describe('rotation', () => {
  it('keeps old ciphertext readable across a DEK rotation', async () => {
    await sb.initSecretbox(fakeSql([rowFor(1, sb.newDek(), true)]))
    const before = sb.seal('sealed before rotation')

    sb.installActiveKey(sb.newDek(), 2)

    expect(sb.currentKeyVersion()).toBe(2)
    expect(sb.loadedVersions().sort()).toEqual([1, 2])
    expect(sb.open(before)).toBe('sealed before rotation') // v1 retained
    const after = sb.seal('sealed after rotation')
    expect(after.startsWith('v2:2:')).toBe(true)
    expect(sb.open(after)).toBe('sealed after rotation')
  })

  it('re-wraps a retained version under a KEK derived from new root material', async () => {
    const d1 = sb.newDek()
    await sb.initSecretbox(fakeSql([rowFor(1, d1, true)]))
    const sealed = sb.seal('crosses the root rotation')

    const newRoot = 'the-operators-new-root-secret'
    const rewrapped = sb.rewrapVersion(1, newRoot)
    expect(rewrapped.startsWith('v1:')).toBe(true)

    // Simulate the restart: new root in the environment, DB holds the re-wrapped row.
    reset()
    const prevRoot = process.env.TALARIA_SECRET_KEY
    process.env.TALARIA_SECRET_KEY = newRoot
    try {
      await sb.initSecretbox(fakeSql([{ version: 1, wrappedDek: rewrapped, active: true }]))
      expect(sb.open(sealed)).toBe('crosses the root rotation')
    } finally {
      process.env.TALARIA_SECRET_KEY = prevRoot
    }
  })

  it('deriveKek is deterministic per root material and differs between roots', () => {
    expect(sb.deriveKek('root-a')).toEqual(sb.deriveKek('root-a'))
    expect(sb.deriveKek('root-a')).not.toEqual(sb.deriveKek('root-b'))
    expect(sb.deriveKek('root-a')).toHaveLength(32) // AES-256
  })

  it('newDek returns 256 bits of fresh randomness', () => {
    const a = sb.newDek()
    expect(a).toHaveLength(32)
    expect(a.equals(sb.newDek())).toBe(false)
  })

  it('sealWith accepts an explicit key and version', async () => {
    await sb.initSecretbox(fakeSql([]))
    const dek = randomBytes(32)
    const token = sb.sealWith(dek, 5, 'explicit')
    expect(token.startsWith('v2:5:')).toBe(true)
    expect(() => sb.open(token)).toThrow(/data key v5 not loaded/)
    sb.installActiveKey(dek, 5)
    expect(sb.open(token)).toBe('explicit')
  })
})

describe('uninitialised module', () => {
  it('refuses to seal before initSecretbox has run', () => {
    expect(() => sb.seal('x')).toThrow(/not initialized/)
    expect(() => sb.currentKeyVersion()).toThrow(/not initialized/)
  })
})
